//! Analyze: map a folder as a treemap, drill in, open it in Files. Read-only.

use crate::app::AppRef;
use crate::widgets::treemap::Treemap;
use crate::widgets::{self, hbox, icons, label, vbox};
use crate::worker;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use sift_core::analyze::{self, Progress, Tree};
use sift_core::cancel::CancelToken;
use sift_core::format::bytes;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;

struct Page {
    app: Weak<crate::app::App>,
    root_path: RefCell<PathBuf>,
    tree: RefCell<Option<Rc<Tree>>>,
    node: Cell<usize>,
    map: Rc<Treemap>,
    side: gtk::ListBox,
    crumbs: gtk::Box,
    status: gtk::Label,
    hover: gtk::Label,
    scan_btn: gtk::Button,
    cancel: RefCell<Option<CancelToken>>,
    body: gtk::Stack,
    progress_label: gtk::Label,
}

pub fn build(app: &AppRef) -> gtk::Widget {
    let root = vbox(14);
    root.add_css_class("page");
    root.add_css_class("accent-analyze");
    let head = hbox(12);
    head.append(&widgets::page_header("Analyze", "Map every chamber below."));
    head.append(&widgets::spacer());
    let home_btn = icons::button("home", Some("Home"));
    let root_btn = icons::button("disk", Some("Whole disk"));
    let pick_btn = icons::button("folder", Some("Choose…"));
    for b in [&home_btn, &root_btn, &pick_btn] {
        b.add_css_class("ghost");
        b.add_css_class("pill");
        b.set_valign(gtk::Align::Center);
        head.append(b);
    }
    let scan_btn = widgets::pill("Scan", &["accent-btn"]);
    scan_btn.set_valign(gtk::Align::Center);
    head.append(&scan_btn);
    root.append(&head);

    let crumbs = hbox(2);
    let status = label("", &["mono", "small", "dim"]);
    let toprow = hbox(10);
    crumbs.set_hexpand(true);
    toprow.append(&crumbs);
    toprow.append(&status);
    root.append(&toprow);

    let body = gtk::Stack::new();
    body.set_vexpand(true);
    body.add_named(
        &widgets::empty_state("analyze", "Pick a folder to map", "Sift measures every folder (staying on one disk, never following links) and draws them by size. Nothing is changed."),
        Some("empty"),
    );
    let scanning = vbox(12);
    scanning.set_valign(gtk::Align::Center);
    let sp = gtk::Spinner::new();
    sp.set_size_request(40, 40);
    sp.start();
    scanning.append(&sp);
    let progress_label = label("", &["mono", "dim"]);
    progress_label.set_halign(gtk::Align::Center);
    progress_label.set_ellipsize(pango::EllipsizeMode::Middle);
    progress_label.set_max_width_chars(90);
    scanning.append(&progress_label);
    body.add_named(&scanning, Some("scanning"));

    let split = hbox(16);
    let map = Treemap::new();
    let mapbox = vbox(6);
    mapbox.set_hexpand(true);
    mapbox.append(&map.area);
    let hover = label(" ", &["hover-info"]);
    hover.set_ellipsize(pango::EllipsizeMode::Middle);
    mapbox.append(&hover);
    split.append(&mapbox);
    let sidebox = vbox(8);
    sidebox.set_size_request(330, -1);
    let open_btn = icons::button("external", Some("Open in Files"));
    open_btn.add_css_class("ghost");
    open_btn.add_css_class("small-btn");
    open_btn.set_halign(gtk::Align::Start);
    sidebox.append(&open_btn);
    let side = gtk::ListBox::new();
    side.add_css_class("list");
    side.set_selection_mode(gtk::SelectionMode::None);
    sidebox.append(&widgets::scrolled(&side));
    split.append(&sidebox);
    body.add_named(&split, Some("map"));
    root.append(&body);

    let page = Rc::new(Page {
        app: Rc::downgrade(app),
        root_path: RefCell::new(app.paths.home.clone()),
        tree: RefCell::new(None),
        node: Cell::new(0),
        map: map.clone(),
        side,
        crumbs,
        status,
        hover,
        scan_btn: scan_btn.clone(),
        cancel: RefCell::new(None),
        body,
        progress_label,
    });
    app.keep(page.clone());
    {
        let p = Rc::downgrade(&page);
        let a = Rc::downgrade(app);
        app.debug_action("analyze.scan", move || {
            if let (Some(p), Some(a)) = (p.upgrade(), a.upgrade()) {
                a.navigate("analyze");
                let target = std::env::var("SIFT_ANALYZE_PATH").map(PathBuf::from).unwrap_or_else(|_| a.paths.home.clone());
                *p.root_path.borrow_mut() = target;
                p.scan();
            }
        });
    }
    page.status.set_text(&format!("Folder: {}", app.paths.home.display()));

    let p = Rc::downgrade(&page);
    map.connect_open(move |idx| {
        if let Some(p) = p.upgrade() {
            p.open_node(idx);
        }
    });
    let p = Rc::downgrade(&page);
    map.connect_hover(move |idx| {
        if let Some(p) = p.upgrade() {
            let tree = p.tree.borrow();
            match (idx, tree.as_ref()) {
                (Some(i), Some(t)) => {
                    let n = &t.nodes[i];
                    let parent = t.nodes[p.node.get()].bytes.max(1);
                    p.hover.set_text(&format!("{}{} — {} ({:.1}%) · {} files", n.name, if n.is_dir { "/" } else { "" }, bytes(n.bytes), n.bytes as f64 / parent as f64 * 100.0, n.files));
                }
                _ => p.hover.set_text(" "),
            }
        }
    });
    let p = Rc::downgrade(&page);
    scan_btn.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            if p.cancel.borrow().is_some() {
                if let Some(c) = p.cancel.borrow().as_ref() {
                    c.cancel();
                }
            } else {
                p.scan();
            }
        }
    });
    let p = Rc::downgrade(&page);
    home_btn.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            if let Some(app) = p.app.upgrade() {
                *p.root_path.borrow_mut() = app.paths.home.clone();
                p.scan();
            }
        }
    });
    let p = Rc::downgrade(&page);
    root_btn.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            *p.root_path.borrow_mut() = PathBuf::from("/");
            p.scan();
        }
    });
    let p = Rc::downgrade(&page);
    pick_btn.connect_clicked(move |_| {
        let Some(p) = p.upgrade() else { return };
        let Some(app) = p.app.upgrade() else { return };
        let dialog = gtk::FileDialog::builder().title("Choose a folder to analyze").modal(true).build();
        let pw = Rc::downgrade(&p);
        dialog.select_folder(Some(&app.window), gtk::gio::Cancellable::NONE, move |res| {
            if let (Ok(file), Some(p)) = (res, pw.upgrade()) {
                if let Some(path) = file.path() {
                    *p.root_path.borrow_mut() = path;
                    p.scan();
                }
            }
        });
    });
    let p = Rc::downgrade(&page);
    open_btn.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.open_in_files();
        }
    });
    root.upcast()
}

impl Page {
    fn scan(self: &Rc<Self>) {
        let root = self.root_path.borrow().clone();
        let cancel = CancelToken::new();
        *self.cancel.borrow_mut() = Some(cancel.clone());
        self.scan_btn.set_label("Stop");
        self.body.set_visible_child_name("scanning");
        self.progress_label.set_text(&format!("Measuring {}…", root.display()));
        self.status.set_text("");
        let progress = Arc::new(Progress::default());
        // Poll progress while the walk runs.
        let pw = Rc::downgrade(self);
        let prog = progress.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
            let Some(p) = pw.upgrade() else { return glib::ControlFlow::Break };
            if p.cancel.borrow().is_none() {
                return glib::ControlFlow::Break;
            }
            let (files, dirs, b, cur) = prog.snapshot();
            p.progress_label.set_text(&format!("{} in {files} files, {dirs} folders\n{cur}", bytes(b)));
            glib::ControlFlow::Continue
        });
        let me = Rc::downgrade(self);
        worker::run(
            move |send| send(analyze::scan(&root, &progress, &cancel).map_err(|e| e.message)),
            move |res: Result<Tree, String>| {
                let Some(me) = me.upgrade() else { return };
                *me.cancel.borrow_mut() = None;
                me.scan_btn.set_label("Rescan");
                match res {
                    Ok(t) => {
                        let t = Rc::new(t);
                        let errs = if t.errors > 0 { format!(" · {} unreadable", t.errors) } else { String::new() };
                        let partial = if t.partial { " · stopped early" } else { "" };
                        me.status.set_text(&format!("{} in {} files{errs}{partial}", bytes(t.nodes[0].bytes), t.nodes[0].files));
                        *me.tree.borrow_mut() = Some(t);
                        me.body.set_visible_child_name("map");
                        me.open_node(0);
                    }
                    Err(e) => {
                        me.body.set_visible_child_name("empty");
                        if let Some(app) = me.app.upgrade() {
                            app.toast(&format!("Couldn't analyze: {e}"));
                        }
                    }
                }
            },
        );
    }

    fn open_node(self: &Rc<Self>, idx: usize) {
        let tree = self.tree.borrow().clone();
        let Some(tree) = tree else { return };
        let n = &tree.nodes[idx];
        if !n.is_dir || n.children.is_empty() {
            // Files and folded groups: nothing to drill into.
            return;
        }
        self.node.set(idx);
        self.map.show(tree.clone(), idx);
        // breadcrumbs
        widgets::clear(&self.crumbs);
        let chain = tree.chain(idx);
        for (i, c) in chain.iter().enumerate() {
            if i > 0 {
                self.crumbs.append(&label("/", &["crumb", "faint"]));
            }
            let name = if i == 0 { tree.root_path.display().to_string() } else { tree.nodes[*c].name.clone() };
            let b = gtk::Button::with_label(&name);
            b.add_css_class("crumb-btn");
            let me = Rc::downgrade(self);
            let target = *c;
            b.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    me.open_node(target);
                }
            });
            self.crumbs.append(&b);
        }
        // side list
        widgets::clear_list(&self.side);
        let total = n.bytes.max(1);
        for c in tree.sorted_children(idx).into_iter().take(200) {
            let child = &tree.nodes[c];
            let row = vbox(4);
            row.add_css_class("row-box");
            let line = hbox(8);
            line.append(&icons::icon(if child.is_dir { "folder" } else if child.is_rest { "dupes" } else { "dot" }, 14));
            let name = label(&child.name, &[]);
            name.set_ellipsize(pango::EllipsizeMode::Middle);
            name.set_hexpand(true);
            line.append(&name);
            line.append(&label(&bytes(child.bytes), &["mono", "small", "dim"]));
            row.append(&line);
            let frac = child.bytes as f64 / total as f64;
            let bar = gtk::ProgressBar::new();
            bar.set_fraction(frac);
            row.append(&bar);
            let r = gtk::ListBoxRow::new();
            r.set_child(Some(&row));
            r.set_activatable(child.is_dir && !child.children.is_empty());
            let me = Rc::downgrade(self);
            let gesture = gtk::GestureClick::new();
            gesture.connect_released(move |_, _, _, _| {
                if let Some(me) = me.upgrade() {
                    me.open_node(c);
                }
            });
            r.add_controller(gesture);
            self.side.append(&r);
        }
    }

    fn open_in_files(&self) {
        let tree = self.tree.borrow();
        let path = tree.as_ref().and_then(|t| t.path_of(self.node.get())).unwrap_or_else(|| self.root_path.borrow().clone());
        let Some(app) = self.app.upgrade() else { return };
        let launcher = gtk::FileLauncher::new(Some(&gtk::gio::File::for_path(&path)));
        let weak = self.app.clone();
        launcher.launch(Some(&app.window), gtk::gio::Cancellable::NONE, move |r| {
            if let (Err(e), Some(app)) = (r, weak.upgrade()) {
                app.toast(&format!("Couldn't open the folder: {e}"));
            }
        });
    }
}
