//! Clean: one hub for caches & system junk, project build artifacts and old
//! installers. Every flow is scan → "X found" → review → confirm → result,
//! and only what you ticked in review is removed.

use crate::app::AppRef;
use crate::theme::{self, Rgb};
use crate::widgets::review::{ReviewList, ReviewRow};
use crate::widgets::{self, charts, hbox, icons, label, vbox};
use crate::worker;
use gtk::glib;
use gtk::prelude::*;
use sift_core::cancel::CancelToken;
use sift_core::clean::{self, CleanItem, ItemKind};
use sift_core::format::{bytes, tilde};
use sift_core::removal::{Event, Summary};
use sift_core::{installer, purge};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Caches,
    Purge,
    Installers,
}

impl Kind {
    fn title(self) -> &'static str {
        match self {
            Kind::Caches => "System & app caches",
            Kind::Purge => "Project build artifacts",
            Kind::Installers => "Leftover installers",
        }
    }
    fn blurb(self) -> &'static str {
        match self {
            Kind::Caches => "Browser, developer and app caches, thumbnails, old logs — plus admin-only junk like the APT cache and system journal.",
            Kind::Purge => "node_modules, target/, build/, __pycache__ and friends in your projects. Rebuilt the next time you build.",
            Kind::Installers => ".deb, .rpm, AppImages, ISOs and archives sitting in Downloads and Desktop.",
        }
    }
    fn accent(self) -> &'static str {
        match self {
            Kind::Caches => "clean",
            Kind::Purge => "purge",
            Kind::Installers => "installer",
        }
    }
    fn color(self) -> Rgb {
        theme::accent_for(self.accent())
    }
    fn icon(self) -> &'static str {
        match self {
            Kind::Caches => "clean",
            Kind::Purge => "code",
            Kind::Installers => "download",
        }
    }
}

enum Scanned {
    Caches(Vec<CleanItem>),
    Purge(Vec<purge::Artifact>, Vec<PathBuf>),
    Installers(Vec<installer::InstallerFile>),
}

enum ScanMsg {
    Found(u64, String),
    Done(Scanned, bool),
}

enum RunMsg {
    Ev(Event),
    Done(Summary),
}

struct Page {
    app: Weak<crate::app::App>,
    root: gtk::Box,
    stack: gtk::Stack,
    kind: Cell<Kind>,
    cancel: RefCell<Option<CancelToken>>,
    scanned: RefCell<Option<Scanned>>,
    phase: Rc<Cell<f64>>,
    animating: Rc<Cell<bool>>,
    crumbs: Vec<gtk::Label>,
    // scan
    scan_orb: gtk::DrawingArea,
    scan_detail: gtk::Label,
    found_so_far: Cell<u64>,
    // found
    found_orb: gtk::DrawingArea,
    found_title: gtk::Label,
    found_sub: gtk::Label,
    found_review: gtk::Button,
    // review
    review: Rc<ReviewList>,
    sel_label: gtk::Label,
    permanent: gtk::Switch,
    go_btn: gtk::Button,
    // run
    run_bar: gtk::ProgressBar,
    run_label: gtk::Label,
    problems: RefCell<Vec<(String, String)>>,
    // done
    done_orb: gtk::DrawingArea,
    done_title: gtk::Label,
    done_sub: gtk::Label,
    done_details: gtk::Box,
    empty_trash: gtk::Button,
    // hub
    trash_label: gtk::Label,
    hub_empty_trash: gtk::Button,
}

fn crumb_bar(stack_back: impl Fn() + 'static) -> (gtk::Box, gtk::Label) {
    let row = hbox(6);
    let back = gtk::Button::new();
    back.add_css_class("crumb-btn");
    let inner = hbox(4);
    inner.append(&icons::icon("back", 16));
    inner.append(&gtk::Label::new(Some("Cleanup")));
    back.set_child(Some(&inner));
    back.connect_clicked(move |_| stack_back());
    row.append(&back);
    row.append(&label("·", &["crumb"]));
    let cur = label("", &["crumb", "faint"]);
    row.append(&cur);
    (row, cur)
}

fn orb_area(size: i32, phase: Rc<Cell<f64>>, color: Rc<Cell<u32>>) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::new();
    a.set_content_width(size);
    a.set_content_height(size);
    a.set_halign(gtk::Align::Center);
    a.set_draw_func(move |_, cr, w, h| charts::orb(cr, w as f64, h as f64, Rgb(color.get()), phase.get()));
    a
}

pub fn build(app: &AppRef) -> gtk::Widget {
    let root = vbox(0);
    root.add_css_class("page");
    root.add_css_class("accent-clean");
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_vexpand(true);
    root.append(&stack);

    let phase = Rc::new(Cell::new(0.0));
    let orb_color = Rc::new(Cell::new(theme::TEAL.0));
    let back_to_hub = {
        let s = stack.clone();
        move || s.set_visible_child_name("hub")
    };

    // ---------- hub
    let hub = vbox(22);
    hub.append(&widgets::page_header("Clean", "Pick what to clear."));
    let cards = gtk::Grid::new();
    cards.set_column_spacing(16);
    cards.set_column_homogeneous(true);
    hub.append(&cards);
    let trash_row = hbox(12);
    trash_row.add_css_class("card");
    trash_row.append(&icons::icon("trash", 22));
    let trash_label = label("Trash", &[]);
    trash_label.set_hexpand(true);
    trash_row.append(&trash_label);
    let hub_empty_trash = widgets::pill("Empty Trash…", &["danger", "small-btn"]);
    hub_empty_trash.set_valign(gtk::Align::Center);
    trash_row.append(&hub_empty_trash);
    hub.append(&trash_row);
    let safety = widgets::wrapped(
        "Sift only removes what you tick in the review. Files go to the Trash (restore them from your file manager) unless you choose to delete permanently. Admin cleanups ask for your password once and show the exact commands first.",
        &["small", "faint"],
    );
    hub.append(&safety);
    stack.add_named(&widgets::scrolled(&hub), Some("hub"));

    // ---------- scanning
    let scan = vbox(18);
    scan.set_valign(gtk::Align::Center);
    let (cb1, crumb1) = crumb_bar(back_to_hub.clone());
    let scan_orb = orb_area(200, phase.clone(), orb_color.clone());
    let scan_title = label("Scanning…", &["orb-title"]);
    scan_title.set_halign(gtk::Align::Center);
    let scan_detail = label("", &["mono", "dim"]);
    scan_detail.set_halign(gtk::Align::Center);
    scan_detail.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    scan_detail.set_max_width_chars(70);
    let stop_scan = icons::button("stop", Some("Stop"));
    stop_scan.add_css_class("pill");
    stop_scan.add_css_class("ghost");
    stop_scan.set_halign(gtk::Align::Center);
    let scan_wrap = vbox(0);
    scan_wrap.append(&cb1);
    scan.append(&scan_orb);
    scan.append(&scan_title);
    scan.append(&scan_detail);
    scan.append(&stop_scan);
    scan.set_vexpand(true);
    scan_wrap.append(&scan);
    stack.add_named(&scan_wrap, Some("scan"));

    // ---------- found
    let found_wrap = vbox(0);
    let (cb2, crumb2) = crumb_bar(back_to_hub.clone());
    found_wrap.append(&cb2);
    let found = vbox(18);
    found.set_valign(gtk::Align::Center);
    found.set_vexpand(true);
    let found_orb = orb_area(200, phase.clone(), orb_color.clone());
    let found_title = label("", &["orb-title"]);
    found_title.set_halign(gtk::Align::Center);
    let found_sub = label("", &["dim"]);
    found_sub.set_halign(gtk::Align::Center);
    let frow = hbox(12);
    frow.set_halign(gtk::Align::Center);
    let found_review = widgets::pill("Review results", &["primary"]);
    let rescan = widgets::pill("Rescan", &["ghost"]);
    let back_btn = widgets::pill("Back", &["flat-link"]);
    frow.append(&found_review);
    frow.append(&rescan);
    frow.append(&back_btn);
    found.append(&found_orb);
    found.append(&found_title);
    found.append(&found_sub);
    found.append(&frow);
    found_wrap.append(&found);
    stack.add_named(&found_wrap, Some("found"));

    // ---------- review
    let review_wrap = vbox(10);
    let (cb3, crumb3) = crumb_bar(back_to_hub.clone());
    review_wrap.append(&cb3);
    let review = ReviewList::new();
    review_wrap.append(&widgets::scrolled(&review.widget));
    let bar = hbox(14);
    bar.add_css_class("bottom-bar");
    let sel_label = label("", &["mono"]);
    sel_label.set_hexpand(true);
    bar.append(&sel_label);
    let perm_box = hbox(8);
    perm_box.append(&label("Delete permanently", &["small", "dim"]));
    let permanent = gtk::Switch::new();
    permanent.set_valign(gtk::Align::Center);
    perm_box.append(&permanent);
    bar.append(&perm_box);
    let go_btn = widgets::pill("Clean selected", &["accent-btn"]);
    bar.append(&go_btn);
    review_wrap.append(&bar);
    stack.add_named(&review_wrap, Some("review"));

    // ---------- running
    let run = vbox(18);
    run.set_valign(gtk::Align::Center);
    run.set_vexpand(true);
    let run_title = label("Cleaning…", &["orb-title"]);
    run_title.set_halign(gtk::Align::Center);
    let run_bar = gtk::ProgressBar::new();
    run_bar.set_margin_start(120);
    run_bar.set_margin_end(120);
    let run_label = label("", &["mono", "dim"]);
    run_label.set_halign(gtk::Align::Center);
    run_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    run_label.set_max_width_chars(80);
    let stop_run = icons::button("stop", Some("Stop"));
    stop_run.add_css_class("pill");
    stop_run.add_css_class("ghost");
    stop_run.set_halign(gtk::Align::Center);
    run.append(&run_title);
    run.append(&run_bar);
    run.append(&run_label);
    run.append(&stop_run);
    stack.add_named(&run, Some("run"));

    // ---------- done
    let done_wrap = vbox(0);
    let (cb4, crumb4) = crumb_bar(back_to_hub.clone());
    done_wrap.append(&cb4);
    let done = vbox(16);
    done.set_valign(gtk::Align::Center);
    done.set_vexpand(true);
    let done_orb = orb_area(150, phase.clone(), orb_color.clone());
    let done_title = label("", &["orb-title"]);
    done_title.set_halign(gtk::Align::Center);
    done_title.set_wrap(true);
    done_title.set_justify(gtk::Justification::Center);
    let done_sub = label("", &["dim"]);
    done_sub.set_halign(gtk::Align::Center);
    let done_details = vbox(4);
    done_details.set_margin_start(80);
    done_details.set_margin_end(80);
    let drow = hbox(12);
    drow.set_halign(gtk::Align::Center);
    let empty_trash = widgets::pill("Empty Trash…", &["danger"]);
    let finish = widgets::pill("Done", &["primary"]);
    let again = widgets::pill("Scan again", &["ghost"]);
    drow.append(&finish);
    drow.append(&again);
    drow.append(&empty_trash);
    done.append(&done_orb);
    done.append(&done_title);
    done.append(&done_sub);
    done.append(&drow);
    done.append(&done_details);
    done_wrap.append(&widgets::scrolled(&done));
    stack.add_named(&done_wrap, Some("done"));

    let page = Rc::new(Page {
        app: Rc::downgrade(app),
        root: root.clone(),
        stack: stack.clone(),
        kind: Cell::new(Kind::Caches),
        cancel: RefCell::new(None),
        scanned: RefCell::new(None),
        phase,
        animating: Rc::new(Cell::new(false)),
        crumbs: vec![crumb1, crumb2, crumb3, crumb4],
        scan_orb,
        scan_detail,
        found_so_far: Cell::new(0),
        found_orb,
        found_title,
        found_sub,
        found_review: found_review.clone(),
        review: review.clone(),
        sel_label,
        permanent: permanent.clone(),
        go_btn: go_btn.clone(),
        run_bar,
        run_label,
        problems: RefCell::new(Vec::new()),
        done_orb,
        done_title,
        done_sub,
        done_details,
        empty_trash: empty_trash.clone(),
        trash_label,
        hub_empty_trash: hub_empty_trash.clone(),
    });
    app.keep(page.clone());
    {
        let p = Rc::downgrade(&page);
        let a = Rc::downgrade(app);
        let step = move |k: Option<Kind>| {
            if let (Some(p), Some(a)) = (p.upgrade(), a.upgrade()) {
                a.navigate("clean");
                match k {
                    Some(k) => p.start_scan(k),
                    None => p.show_review(),
                }
            }
        };
        let s1 = step.clone();
        app.debug_action("clean.scan", move || s1(Some(Kind::Caches)));
        let s2 = step.clone();
        app.debug_action("clean.purge", move || s2(Some(Kind::Purge)));
        let s3 = step.clone();
        app.debug_action("clean.installers", move || s3(Some(Kind::Installers)));
        app.debug_action("clean.review", move || step(None));
        // Applying is only wired up for sandboxed homes (SIFT_HOME).
        if app.paths.sandboxed {
            let p = Rc::downgrade(&page);
            app.debug_action("clean.apply", move || {
                if let Some(p) = p.upgrade() {
                    let ids: HashSet<String> = p.review.selected_ids().into_iter().collect();
                    p.run(ids, false, false, false);
                }
            });
        }
    }
    // hub cards
    for (i, kind) in [Kind::Caches, Kind::Purge, Kind::Installers].into_iter().enumerate() {
        let card = widgets::card();
        card.add_css_class("hoverable");
        card.set_vexpand(false);
        card.append(&super::colored_icon(kind.icon(), 34, kind.color()));
        card.append(&label(kind.title(), &["section-title"]));
        let blurb = widgets::wrapped(kind.blurb(), &["small", "dim"]);
        blurb.set_vexpand(true);
        blurb.set_valign(gtk::Align::Start);
        card.append(&blurb);
        let b = widgets::pill("Scan", &["accent-btn"]);
        b.add_css_class(&format!("accent-{}", kind.accent()));
        b.set_halign(gtk::Align::Start);
        let holder = vbox(0);
        holder.add_css_class(&format!("accent-{}", kind.accent()));
        holder.append(&b);
        card.append(&holder);
        let p = Rc::downgrade(&page);
        b.connect_clicked(move |_| {
            if let Some(p) = p.upgrade() {
                p.start_scan(kind);
            }
        });
        cards.attach(&card, i as i32, 0, 1, 1);
    }

    let p = Rc::downgrade(&page);
    stop_scan.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            if let Some(c) = p.cancel.borrow().as_ref() {
                c.cancel();
            }
        }
    });
    let p = Rc::downgrade(&page);
    stop_run.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            if let Some(c) = p.cancel.borrow().as_ref() {
                c.cancel();
            }
        }
    });
    let p = Rc::downgrade(&page);
    found_review.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.show_review();
        }
    });
    let p = Rc::downgrade(&page);
    rescan.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.start_scan(p.kind.get());
        }
    });
    let p = Rc::downgrade(&page);
    again.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.start_scan(p.kind.get());
        }
    });
    let s = stack.clone();
    back_btn.connect_clicked(move |_| s.set_visible_child_name("hub"));
    let p = Rc::downgrade(&page);
    finish.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.stack.set_visible_child_name("hub");
            p.refresh_trash();
        }
    });
    let p = Rc::downgrade(&page);
    review.connect_changed(move || {
        if let Some(p) = p.upgrade() {
            p.update_selection();
        }
    });
    let p = Rc::downgrade(&page);
    permanent.connect_active_notify(move |_| {
        if let Some(p) = p.upgrade() {
            p.update_selection();
        }
    });
    let p = Rc::downgrade(&page);
    go_btn.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.confirm_and_run();
        }
    });
    for b in [empty_trash, hub_empty_trash] {
        let p = Rc::downgrade(&page);
        b.connect_clicked(move |_| {
            if let Some(p) = p.upgrade() {
                p.confirm_empty_trash();
            }
        });
    }
    let p = Rc::downgrade(&page);
    app.on_page(move |name| {
        if name == "clean" {
            if let Some(p) = p.upgrade() {
                p.refresh_trash();
            }
        }
    });
    root.upcast()
}

impl Page {
    fn set_kind(&self, kind: Kind) {
        self.kind.set(kind);
        for k in ["clean", "purge", "installer"] {
            self.root.remove_css_class(&format!("accent-{k}"));
        }
        self.root.add_css_class(&format!("accent-{}", kind.accent()));
        for c in &self.crumbs {
            c.set_text(kind.title());
        }
        let color = kind.color();
        for orb in [&self.scan_orb, &self.found_orb, &self.done_orb] {
            let phase = self.phase.clone();
            orb.set_draw_func(move |_, cr, w, h| charts::orb(cr, w as f64, h as f64, color, phase.get()));
        }
    }

    fn animate(self: &Rc<Self>, on: bool) {
        self.animating.set(on);
        if !on {
            return;
        }
        let me = Rc::downgrade(self);
        glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
            let Some(me) = me.upgrade() else { return glib::ControlFlow::Break };
            if !me.animating.get() {
                return glib::ControlFlow::Break;
            }
            me.phase.set((me.phase.get() + 0.012) % 1.0);
            me.scan_orb.queue_draw();
            glib::ControlFlow::Continue
        });
    }

    fn refresh_trash(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let ctx = app.ctx();
        let me = Rc::downgrade(self);
        worker::run(
            move |send| {
                let t = ctx.trash();
                send((t.size().bytes, t.count()))
            },
            move |(b, n): (u64, usize)| {
                if let Some(me) = me.upgrade() {
                    if n == 0 {
                        me.trash_label.set_text("Trash is empty");
                        me.hub_empty_trash.set_sensitive(false);
                    } else {
                        me.trash_label.set_text(&format!("Trash holds {} in {n} item{} — that space is freed only when you empty it", bytes(b), if n == 1 { "" } else { "s" }));
                        me.hub_empty_trash.set_sensitive(true);
                    }
                }
            },
        );
    }

    fn start_scan(self: &Rc<Self>, kind: Kind) {
        let Some(app) = self.app.upgrade() else { return };
        self.set_kind(kind);
        self.found_so_far.set(0);
        self.scan_detail.set_text("Looking around…");
        self.stack.set_visible_child_name("scan");
        self.animate(true);
        let cancel = CancelToken::new();
        *self.cancel.borrow_mut() = Some(cancel.clone());
        let ctx = app.ctx();
        let me = Rc::downgrade(self);
        worker::run(
            move |send| {
                let scanned = match kind {
                    Kind::Caches => {
                        let s = clean::scan(&ctx, &clean::ScanOptions { include_system: true }, &cancel, &mut |i| {
                            send(ScanMsg::Found(if i.approx || i.blocked.is_some() { 0 } else { i.bytes }, i.label.clone()))
                        });
                        Scanned::Caches(s.items)
                    }
                    Kind::Purge => {
                        let roots = ctx.settings.purge_root_paths(&ctx.paths);
                        let s = purge::scan(&ctx, &roots, &cancel, &mut |a| {
                            send(ScanMsg::Found(a.bytes, tilde(&a.path, &ctx.paths.home)))
                        });
                        Scanned::Purge(s.items, roots)
                    }
                    Kind::Installers => {
                        let s = installer::scan(&ctx, &cancel, &mut |f| send(ScanMsg::Found(f.bytes, f.name.clone())));
                        Scanned::Installers(s.items)
                    }
                };
                send(ScanMsg::Done(scanned, cancel.is_cancelled()));
            },
            move |msg| {
                let Some(me) = me.upgrade() else { return };
                match msg {
                    ScanMsg::Found(b, what) => {
                        me.found_so_far.set(me.found_so_far.get() + b);
                        me.scan_detail.set_text(&format!("{} found · {what}", bytes(me.found_so_far.get())));
                    }
                    ScanMsg::Done(s, cancelled) => {
                        me.animate(false);
                        me.show_found(s, cancelled);
                    }
                }
            },
        );
    }

    fn show_found(self: &Rc<Self>, s: Scanned, cancelled: bool) {
        let (total, count, pre) = match &s {
            Scanned::Caches(items) => {
                let ok: Vec<&CleanItem> = items.iter().filter(|i| i.blocked.is_none() && !i.approx).collect();
                (ok.iter().map(|i| i.bytes).sum::<u64>(), items.len(), items.iter().filter(|i| i.selected).count())
            }
            Scanned::Purge(a, _) => (a.iter().map(|x| x.bytes).sum(), a.len(), a.iter().filter(|x| x.selected).count()),
            Scanned::Installers(f) => (f.iter().map(|x| x.bytes).sum(), f.len(), f.iter().filter(|x| x.selected).count()),
        };
        *self.scanned.borrow_mut() = Some(s);
        if count == 0 {
            self.found_title.set_text("All clean");
            self.found_sub.set_text(if cancelled { "Scan stopped before anything was found." } else { "Nothing to clear here right now." });
            self.found_review.set_visible(false);
        } else {
            self.found_title.set_text(&format!("{} found", bytes(total)));
            self.found_sub.set_text(&format!(
                "{count} item{} · {pre} pre-selected{}",
                if count == 1 { "" } else { "s" },
                if cancelled { " · scan stopped early" } else { "" }
            ));
            self.found_review.set_visible(true);
        }
        self.stack.set_visible_child_name("found");
        self.found_orb.queue_draw();
    }

    fn show_review(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let home = app.paths.home.clone();
        let rows: Vec<ReviewRow> = match self.scanned.borrow().as_ref() {
            Some(Scanned::Caches(items)) => items
                .iter()
                .map(|i| ReviewRow {
                    id: i.id.clone(),
                    group: i.group.title().into(),
                    group_order: i.group as u32,
                    title: i.label.clone(),
                    subtitle: i.path.as_ref().map(|p| tilde(p, &home)).or(i.command.clone()).unwrap_or_default(),
                    bytes: i.bytes,
                    approx: i.approx,
                    selected: i.selected,
                    blocked: i.blocked.clone(),
                    note: i.note.clone(),
                    badge: match i.kind {
                        ItemKind::Root => Some(("admin".into(), "info".into())),
                        ItemKind::EmptyTrash => Some(("permanent".into(), "bad".into())),
                        ItemKind::Path => None,
                    },
                })
                .collect(),
            Some(Scanned::Purge(items, _)) => items
                .iter()
                .map(|a| ReviewRow {
                    id: a.id.clone(),
                    group: tilde(&a.project, &home),
                    group_order: 0,
                    title: format!("{} — {}", a.name, a.kind),
                    subtitle: tilde(&a.path, &home),
                    bytes: a.bytes,
                    approx: false,
                    selected: a.selected,
                    blocked: None,
                    note: if a.recent { "Project changed recently — not pre-selected".into() } else { String::new() },
                    badge: Some((sift_core::format::ago(a.project_mtime, sift_core::now_unix()), String::new())),
                })
                .collect(),
            Some(Scanned::Installers(files)) => files
                .iter()
                .map(|f| ReviewRow {
                    id: f.id.clone(),
                    group: f.category.title().into(),
                    group_order: f.category as u32,
                    title: f.name.clone(),
                    subtitle: tilde(&f.path, &home),
                    bytes: f.bytes,
                    approx: false,
                    selected: f.selected,
                    blocked: None,
                    note: f.note.clone().unwrap_or_default(),
                    badge: Some((format!("{}d old", f.age_days), String::new())),
                })
                .collect(),
            None => Vec::new(),
        };
        self.permanent.set_active(false);
        self.review.set_rows(rows);
        self.stack.set_visible_child_name("review");
    }

    fn update_selection(&self) {
        let (b, n) = self.review.selection_size();
        self.sel_label.set_text(&format!("Selected {} in {n} item{}", bytes(b), if n == 1 { "" } else { "s" }));
        self.go_btn.set_sensitive(n > 0);
        self.go_btn.set_label(if self.permanent.is_active() { "Delete selected" } else { "Clean selected" });
    }

    fn confirm_and_run(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let ids: HashSet<String> = self.review.selected_ids().into_iter().collect();
        if ids.is_empty() {
            return;
        }
        let permanent = self.permanent.is_active();
        let (bytes_sel, n) = self.review.selection_size();
        let mut admin_cmds: Vec<String> = Vec::new();
        let mut empties_trash = false;
        if let Some(Scanned::Caches(items)) = self.scanned.borrow().as_ref() {
            for i in items.iter().filter(|i| ids.contains(&i.id)) {
                match i.kind {
                    ItemKind::Root => admin_cmds.push(i.command.clone().unwrap_or_default()),
                    ItemKind::EmptyTrash => empties_trash = true,
                    ItemKind::Path => {}
                }
            }
        }
        let heading = if permanent { format!("Permanently delete {n} items?") } else { format!("Clean {n} items?") };
        let mut body = if permanent {
            format!("{} will be deleted for good. This can't be undone.", bytes(bytes_sel))
        } else {
            format!("{} will move to the Trash. You can restore anything from your file manager until you empty it.", bytes(bytes_sel))
        };
        if empties_trash {
            body.push_str("\n\nEmptying the Trash permanently deletes everything already in it.");
        }
        let extra = if admin_cmds.is_empty() {
            None
        } else {
            body.push_str("\n\nThese admin commands will run after one password prompt:");
            Some(widgets::command_list(&admin_cmds))
        };
        let me = Rc::downgrade(self);
        let ids2 = ids.clone();
        let allow_root = !admin_cmds.is_empty();
        widgets::confirm(&app.window, &heading, &body, if permanent { "Delete" } else { "Clean" }, true, extra.as_ref(), move || {
            if let Some(me) = me.upgrade() {
                me.run(ids2.clone(), permanent, empties_trash, allow_root);
            }
        });
    }

    fn run(self: &Rc<Self>, ids: HashSet<String>, permanent: bool, empty_trash: bool, allow_root: bool) {
        let Some(app) = self.app.upgrade() else { return };
        let ctx = app.ctx();
        let cancel = CancelToken::new();
        *self.cancel.borrow_mut() = Some(cancel.clone());
        self.problems.borrow_mut().clear();
        self.run_bar.set_fraction(0.0);
        self.run_label.set_text("Starting…");
        self.stack.set_visible_child_name("run");
        enum Job {
            Caches(Vec<CleanItem>),
            Purge(Vec<PathBuf>, Vec<PathBuf>),
            Installers(Vec<PathBuf>),
        }
        let job = match self.scanned.borrow().as_ref() {
            Some(Scanned::Caches(items)) => Job::Caches(items.iter().filter(|i| ids.contains(&i.id)).cloned().collect()),
            Some(Scanned::Purge(items, roots)) => {
                Job::Purge(items.iter().filter(|a| ids.contains(&a.id)).map(|a| a.path.clone()).collect(), roots.clone())
            }
            Some(Scanned::Installers(files)) => Job::Installers(files.iter().filter(|f| ids.contains(&f.id)).map(|f| f.path.clone()).collect()),
            None => return,
        };
        let me = Rc::downgrade(self);
        worker::run(
            move |send| {
                let mut sink = |e: &Event| send(RunMsg::Ev(e.clone()));
                let summary = match job {
                    Job::Caches(items) => {
                        let opts = clean::ApplyOptions { dry_run: false, permanent, allow_empty_trash: empty_trash, allow_root };
                        clean::apply(&ctx, &items, &opts, &cancel, &mut sink)
                    }
                    Job::Purge(paths, roots) => purge::apply(&ctx, &roots, &paths, false, permanent, &cancel, &mut sink),
                    Job::Installers(paths) => installer::apply(&ctx, &paths, false, permanent, &cancel, &mut sink),
                };
                send(RunMsg::Done(summary));
            },
            move |msg| {
                let Some(me) = me.upgrade() else { return };
                match msg {
                    RunMsg::Ev(Event::Progress { done, total, current }) => {
                        if total > 0 {
                            me.run_bar.set_fraction(done as f64 / total as f64);
                        }
                        me.run_label.set_text(&current);
                    }
                    RunMsg::Ev(Event::Failed { path, reason }) => me.problems.borrow_mut().push((path, format!("failed: {reason}"))),
                    RunMsg::Ev(Event::Refused { path, reason }) => me.problems.borrow_mut().push((path, format!("refused: {reason}"))),
                    RunMsg::Ev(Event::Skipped { path, reason }) => me.problems.borrow_mut().push((path, format!("skipped: {reason}"))),
                    RunMsg::Ev(_) => {}
                    RunMsg::Done(s) => me.show_done(s),
                }
            },
        );
    }

    fn show_done(self: &Rc<Self>, s: Summary) {
        let home = self.app.upgrade().map(|a| a.paths.home.clone()).unwrap_or_default();
        self.done_title.set_text(&s.headline());
        let mut parts = vec![format!("{} removed", s.removed)];
        if s.failed > 0 {
            parts.push(format!("{} failed", s.failed));
        }
        if s.refused > 0 {
            parts.push(format!("{} refused by safety checks", s.refused));
        }
        if s.skipped > 0 {
            parts.push(format!("{} skipped", s.skipped));
        }
        let mut sub = parts.join(" · ");
        if s.trashed_bytes > 0 {
            sub.push_str("\nSpace in the Trash is freed when you empty it.");
        }
        self.done_sub.set_text(&sub);
        self.done_sub.set_justify(gtk::Justification::Center);
        self.empty_trash.set_visible(s.trashed_bytes > 0);
        widgets::clear(&self.done_details);
        let problems = self.problems.borrow();
        if !problems.is_empty() {
            let exp = gtk::Expander::new(Some(&format!("{} items not removed", problems.len())));
            let v = vbox(2);
            for (p, why) in problems.iter().take(300) {
                let row = hbox(10);
                let l = label(&tilde(std::path::Path::new(p), &home), &["mono", "tiny"]);
                l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                l.set_hexpand(true);
                row.append(&l);
                row.append(&label(why, &["tiny", "faint"]));
                v.append(&row);
            }
            exp.set_child(Some(&v));
            self.done_details.append(&exp);
        }
        self.stack.set_visible_child_name("done");
        self.done_orb.queue_draw();
    }

    fn confirm_empty_trash(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let ctx = app.ctx();
        let size = ctx.trash().size().bytes;
        let me = Rc::downgrade(self);
        widgets::confirm(
            &app.window,
            "Empty the Trash?",
            &format!("This permanently deletes everything in the Trash ({}). It can't be undone.", bytes(size)),
            "Empty Trash",
            true,
            None,
            move || {
                let ctx = ctx.clone();
                let me = me.clone();
                worker::run(
                    move |send| {
                        let item = CleanItem {
                            id: "trash".into(),
                            target: "trash".into(),
                            label: "Empty Trash".into(),
                            group: clean::Group::Trash,
                            kind: ItemKind::EmptyTrash,
                            path: None,
                            bytes: 0,
                            approx: false,
                            files: 0,
                            selected: true,
                            blocked: None,
                            note: String::new(),
                            command: None,
                            root: None,
                        };
                        let opts = clean::ApplyOptions { allow_empty_trash: true, ..Default::default() };
                        send(clean::apply(&ctx, &[item], &opts, &CancelToken::new(), &mut |_| {}))
                    },
                    move |s: Summary| {
                        if let Some(me) = me.upgrade() {
                            if let Some(app) = me.app.upgrade() {
                                app.toast(&format!("Trash emptied — {} freed", bytes(s.freed_bytes)));
                            }
                            me.empty_trash.set_visible(false);
                            me.refresh_trash();
                        }
                    },
                );
            },
        );
    }
}
