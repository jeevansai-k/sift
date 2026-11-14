//! Apps: everything installed, with an uninstall that previews exactly what
//! will run and which leftovers would go.

use crate::app::AppRef;
use crate::widgets::{self, hbox, icons, label, vbox};
use crate::worker;
use adw::prelude::*;
use gtk::pango;
use sift_core::apps::{self, App, Source, UninstallPlan, UninstallReport};
use sift_core::format::{bytes, tilde};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

struct Page {
    app: Weak<crate::app::App>,
    list: gtk::ListBox,
    all: RefCell<Vec<App>>,
    search: gtk::SearchEntry,
    source: gtk::DropDown,
    sort: gtk::DropDown,
    count: gtk::Label,
    loaded: Cell<bool>,
}

const SOURCES: &[&str] = &["All sources", "APT", "Flatpak", "Snap", "AppImage", "Local"];
const SORTS: &[&str] = &["Largest first", "Name", "Source"];

pub fn build(app: &AppRef) -> gtk::Widget {
    let root = vbox(14);
    root.add_css_class("page");
    root.add_css_class("accent-apps");
    let head = hbox(12);
    head.append(&widgets::page_header("Apps", "Shed what you've outgrown."));
    head.append(&widgets::spacer());
    root.append(&head);
    let tools = hbox(10);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search apps"));
    search.set_hexpand(true);
    tools.append(&search);
    let source = gtk::DropDown::from_strings(SOURCES);
    tools.append(&source);
    let sort = gtk::DropDown::from_strings(SORTS);
    tools.append(&sort);
    let refresh = icons::button("refresh", None);
    refresh.add_css_class("ghost");
    refresh.add_css_class("pill");
    refresh.set_tooltip_text(Some("Reload"));
    tools.append(&refresh);
    root.append(&tools);
    let count = label("", &["mono", "small", "dim"]);
    root.append(&count);
    let list = gtk::ListBox::new();
    list.add_css_class("list");
    list.set_selection_mode(gtk::SelectionMode::None);
    root.append(&widgets::scrolled(&list));

    let page = Rc::new(Page {
        app: Rc::downgrade(app),
        list,
        all: RefCell::new(Vec::new()),
        search: search.clone(),
        source: source.clone(),
        sort: sort.clone(),
        count,
        loaded: Cell::new(false),
    });
    app.keep(page.clone());
    let p = Rc::downgrade(&page);
    search.connect_search_changed(move |_| {
        if let Some(p) = p.upgrade() {
            p.render();
        }
    });
    for dd in [&source, &sort] {
        let p = Rc::downgrade(&page);
        dd.connect_selected_notify(move |_| {
            if let Some(p) = p.upgrade() {
                p.render();
            }
        });
    }
    let p = Rc::downgrade(&page);
    refresh.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.load();
        }
    });
    let p = Rc::downgrade(&page);
    app.on_page(move |name| {
        if name == "apps" {
            if let Some(p) = p.upgrade() {
                if !p.loaded.replace(true) {
                    p.load();
                }
            }
        }
    });
    root.upcast()
}

fn app_icon(a: &App) -> gtk::Widget {
    let img = match a.icon.as_deref() {
        Some(i) if i.starts_with('/') && std::path::Path::new(i).exists() => gtk::Image::from_file(i),
        Some(i) => gtk::Image::from_icon_name(i),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    img.set_pixel_size(36);
    img.upcast()
}

impl Page {
    fn load(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let ctx = app.ctx();
        widgets::clear_list(&self.list);
        let sp = gtk::Spinner::new();
        sp.start();
        sp.set_margin_top(40);
        self.list.append(&sp);
        self.count.set_text("Reading installed apps…");
        let me = Rc::downgrade(self);
        worker::run(move |send| send(apps::inventory(&ctx)), move |list: Vec<App>| {
            if let Some(me) = me.upgrade() {
                *me.all.borrow_mut() = list;
                me.render();
            }
        });
    }

    fn render(self: &Rc<Self>) {
        widgets::clear_list(&self.list);
        let q = self.search.text().to_lowercase();
        let src = self.source.selected() as usize;
        let all = self.all.borrow();
        let mut shown: Vec<&App> = all
            .iter()
            .filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q) || a.package.to_lowercase().contains(&q))
            .filter(|a| match src {
                1 => a.source == Source::Apt,
                2 => a.source == Source::Flatpak,
                3 => a.source == Source::Snap,
                4 => a.source == Source::AppImage,
                5 => a.source == Source::Local,
                _ => true,
            })
            .collect();
        match self.sort.selected() {
            0 => shown.sort_by(|a, b| b.size.cmp(&a.size)),
            1 => shown.sort_by_key(|a| a.name.to_lowercase()),
            _ => shown.sort_by(|a, b| a.source.cmp(&b.source).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))),
        }
        let total: u64 = shown.iter().map(|a| a.size).sum();
        self.count.set_text(&format!("{} apps · {}", shown.len(), bytes(total)));
        if shown.is_empty() && !all.is_empty() {
            self.list.append(&widgets::empty_state("search", "No matches", "Try a different search or source."));
        }
        for a in shown {
            let line = hbox(14);
            line.add_css_class("row-box");
            line.append(&app_icon(a));
            let text = vbox(2);
            text.set_hexpand(true);
            let name = label(&a.name, &["bold"]);
            name.set_ellipsize(pango::EllipsizeMode::End);
            text.append(&name);
            let meta = hbox(8);
            meta.append(&widgets::chip(a.source.label(), ""));
            let detail = if a.version.is_empty() { a.package.clone() } else { format!("{} · {}", a.package, a.version) };
            let d = label(&detail, &["mono", "tiny", "faint"]);
            d.set_ellipsize(pango::EllipsizeMode::End);
            meta.append(&d);
            text.append(&meta);
            line.append(&text);
            let size = label(&if a.size > 0 { bytes(a.size) } else { "—".into() }, &["mono"]);
            size.set_width_chars(9);
            size.set_xalign(1.0);
            line.append(&size);
            let b = widgets::pill("Uninstall…", &["ghost", "small-btn"]);
            b.set_valign(gtk::Align::Center);
            b.set_sensitive(a.removable);
            if let Some(r) = &a.protected_reason {
                b.set_tooltip_text(Some(r));
            }
            let me = Rc::downgrade(self);
            let id = a.id.clone();
            b.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    me.preview(&id);
                }
            });
            line.append(&b);
            let row = gtk::ListBoxRow::new();
            row.set_activatable(false);
            row.set_child(Some(&line));
            self.list.append(&row);
        }
    }

    fn preview(self: &Rc<Self>, id: &str) {
        let Some(app) = self.app.upgrade() else { return };
        let all = self.all.borrow().clone();
        let Some(target) = all.iter().find(|a| a.id == id).cloned() else { return };
        let ctx = app.ctx();
        app.toast(&format!("Checking what removing {} involves…", target.name));
        let me = Rc::downgrade(self);
        worker::run(move |send| send(apps::plan_uninstall(&ctx, &target, &all)), move |plan: UninstallPlan| {
            if let Some(me) = me.upgrade() {
                me.show_plan(plan);
            }
        });
    }

    fn show_plan(self: &Rc<Self>, plan: UninstallPlan) {
        let Some(app) = self.app.upgrade() else { return };
        let home = app.paths.home.clone();
        let body = vbox(10);
        if let Some(b) = &plan.blocked {
            body.append(&widgets::wrapped(&format!("Sift won't remove this: {b}"), &["bold"]));
        }
        body.append(&label(if plan.needs_root { "Runs as administrator (password prompt):" } else { "Runs:" }, &["small", "dim"]));
        body.append(&widgets::command_list(&plan.commands));
        if !plan.also_removes.is_empty() {
            body.append(&widgets::wrapped(&format!("APT will also remove: {}", plan.also_removes.join(", ")), &["small"]));
        }
        if !plan.trash_files.is_empty() {
            for f in &plan.trash_files {
                body.append(&label(&format!("→ Trash: {}", tilde(f, &home)), &["mono", "tiny", "dim"]));
            }
        }
        let checks: Rc<RefCell<Vec<(PathBuf, gtk::CheckButton)>>> = Rc::new(RefCell::new(Vec::new()));
        if !plan.leftovers.is_empty() {
            body.append(&label("Leftovers (moved to the Trash after the app is removed):", &["small", "dim"]));
            for l in &plan.leftovers {
                let row = hbox(8);
                let cb = gtk::CheckButton::new();
                cb.set_active(l.selected);
                row.append(&cb);
                let t = vbox(0);
                let p = label(&tilde(&l.path, &home), &["mono", "small"]);
                p.set_ellipsize(pango::EllipsizeMode::Middle);
                t.append(&p);
                t.append(&label(&format!("{} · {}", l.reason, bytes(l.bytes)), &["tiny", "faint"]));
                t.set_hexpand(true);
                row.append(&t);
                body.append(&row);
                checks.borrow_mut().push((l.path.clone(), cb));
            }
        } else {
            body.append(&label("No leftover folders found.", &["small", "faint"]));
        }
        let d = adw::MessageDialog::new(Some(&app.window), Some(&format!("Uninstall {}?", plan.app.name)), None);
        d.set_extra_child(Some(&widgets::scrolled(&body)));
        d.set_default_size(560, 520);
        d.add_response("cancel", "Cancel");
        if plan.blocked.is_none() {
            d.add_response("ok", "Uninstall");
            d.set_response_appearance("ok", adw::ResponseAppearance::Destructive);
        }
        d.set_default_response(Some("cancel"));
        d.set_close_response("cancel");
        let me = Rc::downgrade(self);
        let plan = Rc::new(plan);
        d.connect_response(None, move |_, r| {
            if r != "ok" {
                return;
            }
            let chosen: Vec<PathBuf> = checks.borrow().iter().filter(|(_, c)| c.is_active()).map(|(p, _)| p.clone()).collect();
            if let Some(me) = me.upgrade() {
                me.run(plan.clone(), chosen);
            }
        });
        d.present();
    }

    fn run(self: &Rc<Self>, plan: Rc<UninstallPlan>, leftovers: Vec<PathBuf>) {
        let Some(app) = self.app.upgrade() else { return };
        let ctx = app.ctx();
        app.toast(&format!("Removing {}…", plan.app.name));
        let plan_owned: UninstallPlan = (*plan).clone();
        let me = Rc::downgrade(self);
        worker::run(
            move |send| send(apps::uninstall(&ctx, &plan_owned, &leftovers, true, &mut |_| {})),
            move |r: UninstallReport| {
                let Some(me) = me.upgrade() else { return };
                if let Some(app) = me.app.upgrade() {
                    let extra = if r.trashed_bytes > 0 { format!(" · {} of leftovers in the Trash", bytes(r.trashed_bytes)) } else { String::new() };
                    app.toast(&format!("{}{extra}", r.message));
                }
                if r.ok {
                    me.load();
                }
            },
        );
    }
}
