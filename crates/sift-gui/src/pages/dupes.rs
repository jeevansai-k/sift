//! Duplicates: identical files across folders you choose. Keeps at least
//! one copy of everything, re-checks content before moving anything.

use crate::app::AppRef;
use crate::widgets::review::{ReviewList, ReviewRow};
use crate::widgets::{self, hbox, icons, label, vbox};
use crate::worker;
use gtk::glib;
use gtk::prelude::*;
use sift_core::cancel::CancelToken;
use sift_core::dupes::{self, DupeGroup, DupeReport, Keep, Options, Progress};
use sift_core::format::{bytes, tilde};
use sift_core::removal::Summary;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::atomic::Ordering;
use std::sync::Arc;

const MIN_SIZES: &[(&str, u64)] = &[("≥ 100 KB", 100_000), ("≥ 1 MB", 1_000_000), ("≥ 10 MB", 10_000_000), ("≥ 100 MB", 100_000_000)];

struct Page {
    app: Weak<crate::app::App>,
    folders: RefCell<Vec<PathBuf>>,
    folder_box: gtk::Box,
    min_size: gtk::DropDown,
    hidden: gtk::CheckButton,
    scan_btn: gtk::Button,
    status: gtk::Label,
    stack: gtk::Stack,
    review: Rc<ReviewList>,
    report: RefCell<Option<DupeReport>>,
    sel_label: gtk::Label,
    go_btn: gtk::Button,
    cancel: RefCell<Option<CancelToken>>,
}

pub fn build(app: &AppRef) -> gtk::Widget {
    let root = vbox(14);
    root.add_css_class("page");
    root.add_css_class("accent-dupes");
    let head = hbox(12);
    head.append(&widgets::page_header("Duplicates", "Find what you've stashed twice."));
    head.append(&widgets::spacer());
    root.append(&head);

    let opts = hbox(10);
    let folder_box = hbox(6);
    folder_box.set_hexpand(true);
    opts.append(&folder_box);
    let add = icons::button("plus", Some("Add folder"));
    add.add_css_class("ghost");
    add.add_css_class("small-btn");
    opts.append(&add);
    let min_size = gtk::DropDown::from_strings(&MIN_SIZES.iter().map(|m| m.0).collect::<Vec<_>>());
    let cur = app.settings.borrow().dupes_min_size;
    let idx = MIN_SIZES.iter().position(|m| m.1 == cur).unwrap_or(1);
    min_size.set_selected(idx as u32);
    opts.append(&min_size);
    let hidden = gtk::CheckButton::with_label("Include hidden");
    opts.append(&hidden);
    let scan_btn = widgets::pill("Find duplicates", &["accent-btn"]);
    opts.append(&scan_btn);
    root.append(&opts);
    let status = label("", &["mono", "small", "dim"]);
    root.append(&status);

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(
        &widgets::empty_state("dupes", "Look for duplicate files", "Choose folders and Sift compares file contents (not just names). You review every group; one copy of each file is always kept."),
        Some("empty"),
    );
    let results = vbox(10);
    let tools = hbox(10);
    let select_extra = widgets::pill("Select extra copies", &["ghost", "small-btn"]);
    select_extra.set_tooltip_text(Some("Keeps the oldest copy of each file and selects the rest"));
    let select_none = widgets::pill("Select none", &["flat-link", "small-btn"]);
    tools.append(&select_extra);
    tools.append(&select_none);
    results.append(&tools);
    let review = ReviewList::new();
    results.append(&widgets::scrolled(&review.widget));
    let bar = hbox(14);
    bar.add_css_class("bottom-bar");
    let sel_label = label("", &["mono"]);
    sel_label.set_hexpand(true);
    bar.append(&sel_label);
    let go_btn = widgets::pill("Move selected to Trash", &["accent-btn"]);
    bar.append(&go_btn);
    results.append(&bar);
    stack.add_named(&results, Some("results"));
    root.append(&stack);

    let defaults: Vec<PathBuf> = ["Downloads", "Documents", "Pictures", "Desktop", "Videos", "Music"]
        .iter()
        .map(|d| app.paths.home.join(d))
        .filter(|p| p.is_dir())
        .collect();
    let page = Rc::new(Page {
        app: Rc::downgrade(app),
        folders: RefCell::new(defaults),
        folder_box,
        min_size,
        hidden,
        scan_btn: scan_btn.clone(),
        status,
        stack,
        review: review.clone(),
        report: RefCell::new(None),
        sel_label,
        go_btn: go_btn.clone(),
        cancel: RefCell::new(None),
    });
    app.keep(page.clone());
    {
        let p = Rc::downgrade(&page);
        let a = Rc::downgrade(app);
        app.debug_action("dupes.scan", move || {
            if let (Some(p), Some(a)) = (p.upgrade(), a.upgrade()) {
                a.navigate("dupes");
                p.scan();
            }
        });
    }
    page.render_folders();

    let p = Rc::downgrade(&page);
    add.connect_clicked(move |_| {
        let Some(p) = p.upgrade() else { return };
        let Some(app) = p.app.upgrade() else { return };
        let dialog = gtk::FileDialog::builder().title("Add a folder").modal(true).build();
        let pw = Rc::downgrade(&p);
        dialog.select_folder(Some(&app.window), gtk::gio::Cancellable::NONE, move |res| {
            if let (Ok(f), Some(p)) = (res, pw.upgrade()) {
                if let Some(path) = f.path() {
                    if !p.folders.borrow().contains(&path) {
                        p.folders.borrow_mut().push(path);
                    }
                    p.render_folders();
                }
            }
        });
    });
    let p = Rc::downgrade(&page);
    scan_btn.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            if let Some(c) = p.cancel.borrow().as_ref() {
                c.cancel();
                return;
            }
            p.scan();
        }
    });
    let p = Rc::downgrade(&page);
    review.connect_changed(move || {
        if let Some(p) = p.upgrade() {
            let (b, n) = p.review.selection_size();
            p.sel_label.set_text(&format!("Selected {} in {n} file{}", bytes(b), if n == 1 { "" } else { "s" }));
            p.go_btn.set_sensitive(n > 0);
        }
    });
    let p = Rc::downgrade(&page);
    select_extra.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            let ids: HashSet<String> = p
                .report
                .borrow()
                .as_ref()
                .map(|r| r.groups.iter().flat_map(|g| dupes::select_extras(g, Keep::Oldest)).map(|p| p.display().to_string()).collect())
                .unwrap_or_default();
            p.review.set_selected(&ids);
        }
    });
    let p = Rc::downgrade(&page);
    select_none.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.review.set_selected(&HashSet::new());
        }
    });
    let p = Rc::downgrade(&page);
    go_btn.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.confirm_remove();
        }
    });
    root.upcast()
}

impl Page {
    fn render_folders(self: &Rc<Self>) {
        widgets::clear(&self.folder_box);
        let home = self.app.upgrade().map(|a| a.paths.home.clone()).unwrap_or_default();
        for f in self.folders.borrow().iter() {
            let chip = hbox(4);
            chip.add_css_class("chip");
            chip.append(&label(&tilde(f, &home), &[]));
            let x = gtk::Button::new();
            x.add_css_class("flat-link");
            x.set_child(Some(&icons::icon("x", 12)));
            let me = Rc::downgrade(self);
            let path = f.clone();
            x.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    me.folders.borrow_mut().retain(|p| *p != path);
                    me.render_folders();
                }
            });
            chip.append(&x);
            self.folder_box.append(&chip);
        }
    }

    fn scan(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let roots = self.folders.borrow().clone();
        if roots.is_empty() {
            app.toast("Add at least one folder first");
            return;
        }
        let ctx = app.ctx();
        let opts = Options { min_size: MIN_SIZES[self.min_size.selected() as usize % MIN_SIZES.len()].1, include_hidden: self.hidden.is_active() };
        let cancel = CancelToken::new();
        *self.cancel.borrow_mut() = Some(cancel.clone());
        self.scan_btn.set_label("Stop");
        let progress = Arc::new(Progress::default());
        let pw = Rc::downgrade(self);
        let prog = progress.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            let Some(p) = pw.upgrade() else { return glib::ControlFlow::Break };
            if p.cancel.borrow().is_none() {
                return glib::ControlFlow::Break;
            }
            let seen = prog.files_seen.load(Ordering::Relaxed);
            let hashed = prog.hashed.load(Ordering::Relaxed);
            let to_hash = prog.to_hash.load(Ordering::Relaxed);
            p.status.set_text(&if to_hash == 0 { format!("Looking at {seen} files…") } else { format!("Comparing contents: {hashed} of {to_hash} candidates…") });
            glib::ControlFlow::Continue
        });
        let me = Rc::downgrade(self);
        worker::run(move |send| send(dupes::find(&ctx, &roots, &opts, &progress, &cancel)), move |r: DupeReport| {
            let Some(me) = me.upgrade() else { return };
            *me.cancel.borrow_mut() = None;
            me.scan_btn.set_label("Find duplicates");
            me.show(r);
        });
    }

    fn show(self: &Rc<Self>, r: DupeReport) {
        let home = self.app.upgrade().map(|a| a.paths.home.clone()).unwrap_or_default();
        self.status.set_text(&format!(
            "{} groups · {} reclaimable · {} files looked at{}",
            r.groups.len(),
            bytes(r.wasted),
            r.scanned_files,
            if r.partial { " · stopped early" } else { "" }
        ));
        if r.groups.is_empty() {
            self.stack.set_visible_child_name("empty");
            if let Some(app) = self.app.upgrade() {
                app.toast("No duplicates found");
            }
            *self.report.borrow_mut() = Some(r);
            return;
        }
        let mut rows = Vec::new();
        for (gi, g) in r.groups.iter().take(400).enumerate() {
            for f in &g.files {
                rows.push(ReviewRow {
                    id: f.path.display().to_string(),
                    group: format!("{} copies · {} each · {} reclaimable", g.files.len(), bytes(g.size), bytes(g.wasted)),
                    group_order: gi as u32,
                    title: f.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    subtitle: tilde(&f.path, &home),
                    bytes: g.size,
                    approx: false,
                    selected: false,
                    blocked: None,
                    note: String::new(),
                    badge: Some((sift_core::format::datetime(f.mtime), String::new())),
                });
            }
        }
        self.review.set_rows(rows);
        *self.report.borrow_mut() = Some(r);
        self.stack.set_visible_child_name("results");
    }

    fn confirm_remove(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let selected: HashSet<PathBuf> = self.review.selected_ids().into_iter().map(PathBuf::from).collect();
        let (b, n) = self.review.selection_size();
        let groups: Vec<DupeGroup> = self.report.borrow().as_ref().map(|r| r.groups.clone()).unwrap_or_default();
        let roots = self.folders.borrow().clone();
        let me = Rc::downgrade(self);
        widgets::confirm(
            &app.window,
            &format!("Move {n} duplicate files to the Trash?"),
            &format!("{} — each file is re-checked first, and Sift refuses to remove the last copy of anything.", bytes(b)),
            "Move to Trash",
            true,
            None,
            move || {
                let Some(me) = me.upgrade() else { return };
                let Some(app) = me.app.upgrade() else { return };
                let ctx = app.ctx();
                let (sel, groups, roots) = (selected.clone(), groups.clone(), roots.clone());
                me.go_btn.set_sensitive(false);
                let mw = Rc::downgrade(&me);
                worker::run(
                    move |send| send(dupes::remove(&ctx, &roots, &groups, &sel, false, &CancelToken::new(), &mut |_| {})),
                    move |s: Summary| {
                        let Some(me) = mw.upgrade() else { return };
                        if let Some(app) = me.app.upgrade() {
                            let extra = if s.refused > 0 { format!(" · {} refused (changed or last copy)", s.refused) } else { String::new() };
                            app.toast(&format!("{}{extra}", s.headline()));
                        }
                        me.scan();
                    },
                );
            },
        );
    }
}
