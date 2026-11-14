//! The window: floating icon rail on the left, one page at a time on the
//! right, a live status feed shared by every page.

use crate::widgets::{self, icons};
use crate::{pages, theme};
use adw::prelude::*;
use gtk::glib;
use sift_core::ctx::Ctx;
use sift_core::paths::Paths;
use sift_core::runner::SystemRunner;
use sift_core::sampler::{Accumulator, SamplerLock};
use sift_core::settings::{Settings, Theme};
use sift_core::status::{Collector, Snapshot};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const APP_ID: &str = "dev.sift.Sift";
const HISTORY_POINTS: usize = 90;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Recording {
    /// This window writes history (no background service running).
    App,
    /// The `sift-sampler` service writes history.
    Service,
}

#[derive(Default)]
pub struct Live {
    pub snap: Option<Snapshot>,
    pub cpu: VecDeque<f64>,
    pub mem: VecDeque<f64>,
    /// One usage history per GPU, in snapshot order.
    pub gpus: Vec<VecDeque<f64>>,
    pub rx: VecDeque<f64>,
    pub tx: VecDeque<f64>,
    pub disk_io: VecDeque<f64>,
    pub fan: VecDeque<f64>,
    pub recording: Option<Recording>,
}

fn push(q: &mut VecDeque<f64>, v: f64) {
    if q.len() >= HISTORY_POINTS {
        q.pop_front();
    }
    q.push_back(v);
}

struct LiveMsg {
    snap: Snapshot,
    recording: Recording,
}

pub struct App {
    pub window: adw::ApplicationWindow,
    pub toasts: adw::ToastOverlay,
    pub stack: gtk::Stack,
    pub paths: Paths,
    pub settings: RefCell<Settings>,
    pub live: RefCell<Live>,
    listeners: RefCell<Vec<Box<dyn Fn(&App)>>>,
    page_listeners: RefCell<Vec<Box<dyn Fn(&str)>>>,
    rail: RefCell<Vec<(String, gtk::Button)>>,
    refresh_secs: Arc<AtomicU32>,
    sample_secs: Arc<AtomicU32>,
    retention_days: Arc<AtomicU32>,
    current: RefCell<String>,
    /// Page controllers; their callbacks only hold weak references.
    pages: RefCell<Vec<Rc<dyn std::any::Any>>>,
    /// Named steps the screenshot tour can trigger (debug only).
    debug: RefCell<Vec<(String, Rc<dyn Fn()>)>>,
}

pub type AppRef = Rc<App>;

impl App {
    pub fn build(application: &adw::Application, paths: Paths) {
        let settings = Settings::load(&paths);
        let style = adw::StyleManager::default();
        style.set_color_scheme(match settings.theme {
            Theme::System => adw::ColorScheme::Default,
            Theme::Dark => adw::ColorScheme::ForceDark,
            Theme::Light => adw::ColorScheme::ForceLight,
        });
        theme::apply(style.is_dark());

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(160);
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        let toasts = adw::ToastOverlay::new();
        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title("Sift")
            .default_width(1240)
            .default_height(860)
            .content(&toasts)
            .build();
        window.add_css_class("sift-window");
        window.set_size_request(900, 620);

        let app = Rc::new(App {
            window,
            toasts: toasts.clone(),
            stack: stack.clone(),
            refresh_secs: Arc::new(AtomicU32::new(settings.live_refresh_secs)),
            sample_secs: Arc::new(AtomicU32::new(settings.sample_interval_secs)),
            retention_days: Arc::new(AtomicU32::new(settings.retention_days)),
            settings: RefCell::new(settings),
            paths,
            live: RefCell::new(Live::default()),
            listeners: RefCell::new(Vec::new()),
            page_listeners: RefCell::new(Vec::new()),
            rail: RefCell::new(Vec::new()),
            current: RefCell::new(String::new()),
            pages: RefCell::new(Vec::new()),
            debug: RefCell::new(Vec::new()),
        });

        let weak = Rc::downgrade(&app);
        style.connect_dark_notify(move |sm| {
            theme::apply(sm.is_dark());
            if let Some(app) = weak.upgrade() {
                app.notify_live();
            }
        });

        // Layout: rail | (header bar / pages)
        let root = widgets::hbox(0);
        let rail = app.build_rail();
        let handle = gtk::WindowHandle::new();
        handle.set_child(Some(&rail));
        root.append(&handle);
        let main = widgets::vbox(0);
        main.set_hexpand(true);
        let header = adw::HeaderBar::new();
        header.set_show_title(false);
        header.add_css_class("flat");
        main.append(&header);
        main.append(&stack);
        root.append(&main);
        toasts.set_child(Some(&root));

        stack.add_named(&pages::monitor::build(&app), Some("monitor"));
        stack.add_named(&pages::clean::build(&app), Some("clean"));
        stack.add_named(&pages::optimize::build(&app), Some("optimize"));
        stack.add_named(&pages::apps::build(&app), Some("apps"));
        stack.add_named(&pages::analyze::build(&app), Some("analyze"));
        stack.add_named(&pages::dupes::build(&app), Some("dupes"));
        stack.add_named(&pages::ports::build(&app), Some("ports"));
        stack.add_named(&pages::settings::build(&app), Some("settings"));

        // The window owns the App: callbacks only hold weak references, so
        // without this the App (and with it the live feed and navigation)
        // would be dropped as soon as build() returns.
        let owner = app.clone();
        app.window.connect_close_request(move |_| {
            let _ = &owner;
            glib::Propagation::Proceed
        });

        app.start_live_feed();
        let first = std::env::var("SIFT_PAGE").unwrap_or_else(|_| "monitor".into());
        app.navigate(&first);
        app.window.present();

        if let Some(dir) = std::env::var_os("SIFT_SCREENSHOT_DIR") {
            screenshot_tour(&app, std::path::PathBuf::from(dir));
        }
    }

    fn build_rail(self: &Rc<Self>) -> gtk::Box {
        let rail = widgets::vbox(8);
        rail.add_css_class("rail");
        rail.set_vexpand(true);

        let logo = gtk::Button::new();
        logo.add_css_class("rail-btn");
        logo.add_css_class("rail-logo");
        logo.add_css_class("accent-monitor");
        logo.set_child(Some(&logo_mark(30)));
        logo.set_tooltip_text(Some("Monitor"));
        let me = Rc::downgrade(self);
        logo.connect_clicked(move |_| {
            if let Some(a) = me.upgrade() {
                a.navigate("monitor");
            }
        });
        rail.append(&logo);
        self.rail.borrow_mut().push(("monitor".into(), logo));
        let sep = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        sep.add_css_class("rail-sep");
        rail.append(&sep);

        let glyph = |id: &str| match id {
            "clean" => "clean",
            "optimize" => "optimize",
            "apps" => "apps",
            "analyze" => "analyze",
            "dupes" => "dupes",
            "ports" => "ports",
            _ => "dot",
        };
        for (id, title, _) in theme::TOOLS {
            let b = gtk::Button::new();
            b.add_css_class("rail-btn");
            b.add_css_class(&format!("accent-{id}"));
            b.set_child(Some(&icons::icon(glyph(id), 22)));
            b.set_tooltip_text(Some(title));
            let me = Rc::downgrade(self);
            let id_s = id.to_string();
            b.connect_clicked(move |_| {
                if let Some(a) = me.upgrade() {
                    a.navigate(&id_s);
                }
            });
            rail.append(&b);
            self.rail.borrow_mut().push((id.to_string(), b));
        }
        let fill = widgets::vbox(0);
        fill.set_vexpand(true);
        rail.append(&fill);
        let s = gtk::Button::new();
        s.add_css_class("rail-btn");
        s.add_css_class("accent-settings");
        s.set_child(Some(&icons::icon("settings", 22)));
        s.set_tooltip_text(Some("Settings"));
        let me = Rc::downgrade(self);
        s.connect_clicked(move |_| {
            if let Some(a) = me.upgrade() {
                a.navigate("settings");
            }
        });
        rail.append(&s);
        self.rail.borrow_mut().push(("settings".into(), s));
        rail
    }

    pub fn navigate(&self, page: &str) {
        let page = if self.stack.child_by_name(page).is_some() { page } else { "monitor" };
        self.stack.set_visible_child_name(page);
        for (id, b) in self.rail.borrow().iter() {
            if id == page {
                b.add_css_class("active");
            } else {
                b.remove_css_class("active");
            }
            if let Some(c) = b.child() {
                c.queue_draw();
            }
        }
        *self.current.borrow_mut() = page.to_string();
        let listeners = std::mem::take(&mut *self.page_listeners.borrow_mut());
        for f in &listeners {
            f(page);
        }
        let mut cur = self.page_listeners.borrow_mut();
        let added = std::mem::take(&mut *cur);
        *cur = listeners;
        cur.extend(added);
    }

    /// Keep a page controller alive for the life of the window.
    /// Register a step for SIFT_SCREENSHOT_PAGES (e.g. `clean.scan`).
    pub fn debug_action(&self, name: &str, f: impl Fn() + 'static) {
        if std::env::var_os("SIFT_SCREENSHOT_DIR").is_some() {
            self.debug.borrow_mut().push((name.to_string(), Rc::new(f)));
        }
    }

    pub fn keep(&self, page: Rc<dyn std::any::Any>) {
        self.pages.borrow_mut().push(page);
    }

    pub fn current_page(&self) -> String {
        self.current.borrow().clone()
    }

    /// Called whenever a page becomes visible.
    pub fn on_page(&self, f: impl Fn(&str) + 'static) {
        self.page_listeners.borrow_mut().push(Box::new(f));
    }

    /// Called on every live status tick (after `live` is updated).
    pub fn on_live(&self, f: impl Fn(&App) + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    fn notify_live(&self) {
        let ls = std::mem::take(&mut *self.listeners.borrow_mut());
        for f in &ls {
            f(self);
        }
        let mut cur = self.listeners.borrow_mut();
        let added = std::mem::take(&mut *cur);
        *cur = ls;
        cur.extend(added);
    }

    pub fn toast(&self, text: &str) {
        let t = adw::Toast::new(text);
        t.set_timeout(4);
        self.toasts.add_toast(t);
    }

    /// Engine context with the current settings (cloned for worker threads).
    pub fn ctx(&self) -> Ctx {
        Ctx {
            paths: self.paths.clone(),
            settings: self.settings.borrow().clone(),
            runner: Arc::new(SystemRunner),
            source: "gui".into(),
        }
    }

    /// Change settings, save them, and apply what can be applied live.
    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let mut s = self.settings.borrow().clone();
        f(&mut s);
        let s = s.clamped();
        if let Err(e) = s.save(&self.paths) {
            self.toast(&format!("Couldn't save settings: {e}"));
        }
        self.refresh_secs.store(s.live_refresh_secs, Ordering::Relaxed);
        self.sample_secs.store(s.sample_interval_secs, Ordering::Relaxed);
        self.retention_days.store(s.retention_days, Ordering::Relaxed);
        let scheme = match s.theme {
            Theme::System => adw::ColorScheme::Default,
            Theme::Dark => adw::ColorScheme::ForceDark,
            Theme::Light => adw::ColorScheme::ForceLight,
        };
        *self.settings.borrow_mut() = s;
        adw::StyleManager::default().set_color_scheme(scheme);
    }

    /// Background thread: collect a snapshot every refresh interval; write a
    /// history row every sample interval unless the service already does.
    fn start_live_feed(self: &Rc<Self>) {
        let (tx, rx) = async_channel::bounded::<LiveMsg>(2);
        let paths = self.paths.clone();
        let refresh = self.refresh_secs.clone();
        let sample = self.sample_secs.clone();
        let retention = self.retention_days.clone();
        std::thread::spawn(move || {
            let debug = std::env::var_os("SIFT_DEBUG").is_some();
            if debug {
                eprintln!("[sift] live feed thread started");
            }
            let mut collector = Collector::new();
            let mut lock: Option<SamplerLock> = None;
            let mut db: Option<sift_core::db::Db> = None;
            let mut acc = Accumulator::default();
            let mut last_write = Instant::now();
            let mut last_lock_try = Instant::now() - Duration::from_secs(120);
            let mut last_prune = Instant::now() - Duration::from_secs(7200);
            loop {
                // Take over recording when the service isn't running.
                if lock.is_none() && last_lock_try.elapsed() > Duration::from_secs(60) {
                    last_lock_try = Instant::now();
                    if let Some(l) = SamplerLock::try_acquire(&paths) {
                        db = sift_core::db::Db::open(&paths.db_file()).ok();
                        lock = Some(l);
                    }
                }
                if debug {
                    eprintln!("[sift] collecting (lock held: {})", lock.is_some());
                }
                let snap = collector.collect();
                if debug {
                    eprintln!("[sift] collected: cpu {:.1}%", snap.cpu.usage);
                }
                let recording = if lock.is_some() { Recording::App } else { Recording::Service };
                if let Some(d) = db.as_mut() {
                    acc.add(&snap);
                    if last_write.elapsed() >= Duration::from_secs(sample.load(Ordering::Relaxed) as u64) {
                        if let Some((row, procs)) = acc.take() {
                            let _ = d.insert_sample(&row, &procs);
                        }
                        last_write = Instant::now();
                    }
                    if last_prune.elapsed() > Duration::from_secs(3600) {
                        let _ = d.prune(retention.load(Ordering::Relaxed), sift_core::now_unix());
                        last_prune = Instant::now();
                    }
                }
                if tx.send_blocking(LiveMsg { snap, recording }).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_secs(refresh.load(Ordering::Relaxed).max(1) as u64));
            }
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                if std::env::var_os("SIFT_DEBUG").is_some() {
                    eprintln!("[sift] main loop received snapshot");
                }
                let Some(app) = weak.upgrade() else { break };
                {
                    let mut live = app.live.borrow_mut();
                    let s = &msg.snap;
                    push(&mut live.cpu, s.cpu.usage);
                    push(&mut live.mem, s.memory.used_pct);
                    if live.gpus.len() != s.gpus.len() {
                        live.gpus = vec![VecDeque::new(); s.gpus.len()];
                    }
                    for (q, g) in live.gpus.iter_mut().zip(s.gpus.iter()) {
                        push(q, g.usage.unwrap_or(0.0));
                    }
                    push(&mut live.rx, s.net_rx_bps);
                    push(&mut live.tx, s.net_tx_bps);
                    push(&mut live.disk_io, s.disk_io.read_bps + s.disk_io.write_bps);
                    push(&mut live.fan, s.thermal.fans.first().map(|f| f.rpm as f64).unwrap_or(0.0));
                    live.recording = Some(msg.recording);
                    live.snap = Some(msg.snap);
                }
                app.notify_live();
            }
        });
    }
}

/// Sift's mark: a sieve — a bowl with three falling grains.
pub fn logo_mark(px: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(px);
    area.set_content_height(px);
    area.set_can_target(false);
    area.set_draw_func(|_, cr, w, h| {
        let s = w.min(h) as f64 / 32.0;
        cr.scale(s, s);
        let pal = theme::palette();
        theme::set_source(cr, pal.ink(1.0));
        // bowl
        cr.move_to(4.0, 9.0);
        cr.line_to(28.0, 9.0);
        cr.curve_to(28.0, 16.5, 22.5, 21.0, 16.0, 21.0);
        cr.curve_to(9.5, 21.0, 4.0, 16.5, 4.0, 9.0);
        cr.close_path();
        let _ = cr.fill();
        // mesh holes
        let (r, g, b, _) = pal.base.rgba(1.0);
        cr.set_source_rgba(r, g, b, 1.0);
        for (x, y) in [(10.5, 12.5), (16.0, 12.5), (21.5, 12.5), (13.25, 16.3), (18.75, 16.3)] {
            cr.arc(x, y, 1.25, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.fill();
        }
        // grains
        let (r, g, b, _) = theme::GOLD.rgba(1.0);
        cr.set_source_rgba(r, g, b, 1.0);
        for (x, y, rad) in [(13.0, 25.0, 1.5), (18.5, 27.0, 1.3), (15.5, 29.6, 1.1)] {
            cr.arc(x, y, rad, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.fill();
        }
    });
    area
}

/// Debug aid: render each page to a PNG (SIFT_SCREENSHOT_DIR), then quit.
fn screenshot_tour(app: &Rc<App>, dir: std::path::PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    // A background window's frame clock may not tick, so transitions would
    // never finish; switch pages instantly instead.
    if let Some(s) = gtk::Settings::default() {
        s.set_gtk_enable_animations(false);
    }
    app.stack.set_transition_type(gtk::StackTransitionType::None);
    let pages: Vec<String> = std::env::var("SIFT_SCREENSHOT_PAGES")
        .unwrap_or_else(|_| "monitor,clean,optimize,apps,analyze,dupes,ports,settings".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let delay = std::env::var("SIFT_SCREENSHOT_DELAY").ok().and_then(|v| v.parse().ok()).unwrap_or(5u64);
    let app = app.clone();
    let idx = Rc::new(std::cell::Cell::new(0usize));
    glib::timeout_add_seconds_local(1, move || {
        let i = idx.get();
        // Each page gets `delay` seconds to load before capture.
        let page_i = i / delay as usize;
        if page_i >= pages.len() {
            app.window.close();
            return glib::ControlFlow::Break;
        }
        let page = &pages[page_i];
        if i % delay as usize == 0 {
            let action = app.debug.borrow().iter().find(|(n, _)| n == page).map(|(_, f)| f.clone());
            match action {
                Some(f) => f(),
                None => app.navigate(page),
            }
            app.window.present();
        } else if i % delay as usize == delay as usize - 1 {
            let path = dir.join(format!("{page}.png"));
            if !save_png(app.toasts.upcast_ref::<gtk::Widget>(), &path) {
                eprintln!("screenshot failed for {page}");
            }
        }
        idx.set(i + 1);
        glib::ControlFlow::Continue
    });
}

pub fn save_png(widget: &gtk::Widget, path: &std::path::Path) -> bool {
    let (w, h) = (widget.width() as f64, widget.height() as f64);
    if w < 1.0 || h < 1.0 {
        return false;
    }
    // A covered window may not get frames from the compositor, so run
    // layout ourselves before painting.
    widget.allocate(w as i32, h as i32, -1, None);
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    // Paint the window background first so the PNG isn't transparent.
    let base = theme::palette().base;
    let (r, g, b, _) = base.rgba(1.0);
    snapshot.append_color(&gtk::gdk::RGBA::new(r as f32, g as f32, b as f32, 1.0), &gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32));
    paintable.snapshot(&snapshot, w, h);
    let Some(node) = snapshot.to_node() else { return false };
    let Some(native) = widget.native() else { return false };
    let Some(renderer) = native.renderer() else { return false };
    let texture = renderer.render_texture(&node, None);
    texture.save_to_png(path).is_ok()
}
