//! Ports: who's listening, and on which addresses.

use crate::app::AppRef;
use crate::widgets::{self, hbox, icons, label, vbox};
use crate::worker;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use sift_core::ports::{self, Port};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

struct Page {
    app: Weak<crate::app::App>,
    holder: gtk::Box,
    filter: gtk::SearchEntry,
    all: RefCell<Vec<Port>>,
    count: gtk::Label,
    root: gtk::Box,
}

pub fn build(app: &AppRef) -> gtk::Widget {
    let root = vbox(14);
    root.add_css_class("page");
    root.add_css_class("accent-ports");
    let head = hbox(12);
    head.append(&widgets::page_header("Ports", "See who's listening."));
    head.append(&widgets::spacer());
    let filter = gtk::SearchEntry::new();
    filter.set_placeholder_text(Some("Filter by port or process"));
    filter.set_valign(gtk::Align::Center);
    head.append(&filter);
    let refresh = icons::button("refresh", None);
    refresh.add_css_class("ghost");
    refresh.add_css_class("pill");
    refresh.set_valign(gtk::Align::Center);
    head.append(&refresh);
    root.append(&head);
    let count = label("", &["mono", "small", "dim"]);
    root.append(&count);
    let holder = vbox(0);
    let card = widgets::card();
    card.append(&holder);
    root.append(&widgets::scrolled(&card));
    root.append(&widgets::wrapped(
        "“local” ports only accept connections from this computer; “network” ports are reachable from other machines (subject to your firewall). Processes owned by other users or the system show without a PID unless Sift runs as that user.",
        &["small", "faint"],
    ));
    let page = Rc::new(Page { app: Rc::downgrade(app), holder, filter: filter.clone(), all: RefCell::new(Vec::new()), count, root: root.clone() });
    app.keep(page.clone());
    let p = Rc::downgrade(&page);
    refresh.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.load();
        }
    });
    let p = Rc::downgrade(&page);
    filter.connect_search_changed(move |_| {
        if let Some(p) = p.upgrade() {
            p.render();
        }
    });
    let p = Rc::downgrade(&page);
    app.on_page(move |name| {
        if name == "ports" {
            if let Some(p) = p.upgrade() {
                p.load();
            }
        }
    });
    let p = Rc::downgrade(&page);
    glib::timeout_add_seconds_local(5, move || {
        let Some(p) = p.upgrade() else { return glib::ControlFlow::Break };
        if p.root.is_mapped() {
            p.load();
        }
        glib::ControlFlow::Continue
    });
    root.upcast()
}

impl Page {
    fn load(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        worker::run(move |send| send(ports::list()), move |list: Vec<Port>| {
            if let Some(me) = me.upgrade() {
                *me.all.borrow_mut() = list;
                me.render();
            }
        });
    }

    fn render(self: &Rc<Self>) {
        widgets::clear(&self.holder);
        let q = self.filter.text().to_lowercase();
        let all = self.all.borrow();
        let shown: Vec<&Port> = all
            .iter()
            .filter(|p| q.is_empty() || p.port.to_string().contains(&q) || p.process.as_deref().unwrap_or("").to_lowercase().contains(&q))
            .collect();
        let exposed = shown.iter().filter(|p| p.exposure == "network").count();
        self.count.set_text(&format!("{} listening · {exposed} reachable from the network", shown.len()));
        let grid = gtk::Grid::new();
        grid.set_column_spacing(18);
        grid.set_row_spacing(8);
        for (i, h) in ["PROTO", "PORT", "ADDRESS", "", "PROCESS", "PID", "USER", ""].iter().enumerate() {
            grid.attach(&label(h, &["table-head"]), i as i32, 0, 1, 1);
        }
        let uid = unsafe { libc::geteuid() };
        for (r, p) in shown.iter().enumerate() {
            let r = r as i32 + 1;
            grid.attach(&widgets::chip(&p.proto, ""), 0, r, 1, 1);
            grid.attach(&label(&p.port.to_string(), &["mono", "bold"]), 1, r, 1, 1);
            grid.attach(&label(&p.address, &["mono", "small", "dim"]), 2, r, 1, 1);
            grid.attach(&widgets::chip(&p.exposure, if p.exposure == "network" { "warn" } else { "good" }), 3, r, 1, 1);
            let proc_name = label(p.process.as_deref().unwrap_or("—"), &[]);
            proc_name.set_ellipsize(pango::EllipsizeMode::End);
            proc_name.set_hexpand(true);
            if let Some(c) = &p.command {
                proc_name.set_tooltip_text(Some(c));
            }
            grid.attach(&proc_name, 4, r, 1, 1);
            grid.attach(&label(&p.pid.map(|x| x.to_string()).unwrap_or_else(|| "—".into()), &["mono", "small", "dim"]), 5, r, 1, 1);
            grid.attach(&label(&p.user, &["small", "dim"]), 6, r, 1, 1);
            if let (Some(pid), true) = (p.pid, p.uid == uid) {
                let b = widgets::pill("Stop", &["danger", "small-btn"]);
                let me = Rc::downgrade(self);
                let name = p.process.clone().unwrap_or_default();
                let port = p.port;
                b.connect_clicked(move |_| {
                    if let Some(me) = me.upgrade() {
                        me.confirm_stop(pid, &name, port);
                    }
                });
                grid.attach(&b, 7, r, 1, 1);
            }
        }
        if shown.is_empty() {
            self.holder.append(&label("Nothing is listening.", &["dim"]));
        } else {
            self.holder.append(&grid);
        }
    }

    fn confirm_stop(self: &Rc<Self>, pid: u32, name: &str, port: u16) {
        let Some(app) = self.app.upgrade() else { return };
        let me = Rc::downgrade(self);
        widgets::confirm(
            &app.window,
            &format!("Stop {name}?"),
            &format!("Sends {name} (PID {pid}, listening on port {port}) a request to quit. Unsaved work in that program may be lost."),
            "Stop process",
            true,
            None,
            move || {
                let Some(me) = me.upgrade() else { return };
                let msg = match ports::stop_process(pid, false) {
                    Ok(()) => format!("Asked PID {pid} to stop"),
                    Err(e) => format!("Couldn't stop PID {pid}: {}", e.message),
                };
                if let Some(app) = me.app.upgrade() {
                    app.toast(&msg);
                }
                let me2 = Rc::downgrade(&me);
                glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                    if let Some(m) = me2.upgrade() {
                        m.load();
                    }
                });
            },
        );
    }
}
