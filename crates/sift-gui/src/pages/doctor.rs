//! Doctor: health checks with a plain-language fix for each finding.

use crate::app::AppRef;
use crate::theme;
use crate::widgets::{self, hbox, icons, label, vbox};
use crate::worker;
use gtk::prelude::*;
use sift_core::doctor::{self, Check, Level};
use std::cell::Cell;
use std::rc::Rc;

pub struct DoctorView {
    pub widget: gtk::Widget,
    app: std::rc::Weak<crate::app::App>,
    list: gtk::Box,
    status: gtk::Label,
    run_btn: gtk::Button,
    ran: Cell<bool>,
}

pub fn build(app: &AppRef) -> Rc<DoctorView> {
    let page = vbox(14);
    page.add_css_class("page");
    page.set_margin_top(14);
    let top = hbox(10);
    let status = label("Checks haven't run yet.", &["dim"]);
    top.append(&status);
    top.append(&widgets::spacer());
    let run_btn = icons::button("refresh", Some("Run checks"));
    run_btn.add_css_class("ghost");
    run_btn.add_css_class("small-btn");
    top.append(&run_btn);
    page.append(&top);
    let list = vbox(10);
    page.append(&list);
    let view = Rc::new(DoctorView {
        widget: widgets::scrolled(&page).upcast(),
        app: Rc::downgrade(app),
        list,
        status,
        run_btn: run_btn.clone(),
        ran: Cell::new(false),
    });
    let v = Rc::downgrade(&view);
    run_btn.connect_clicked(move |_| {
        if let Some(v) = v.upgrade() {
            v.run();
        }
    });
    view
}

impl DoctorView {
    pub fn ensure_run(self: &Rc<Self>) {
        if !self.ran.replace(true) {
            self.run();
        }
    }

    pub fn run(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let ctx = app.ctx();
        let snap = app.live.borrow().snap.clone();
        self.status.set_text("Checking…");
        self.run_btn.set_sensitive(false);
        let me = Rc::downgrade(self);
        worker::run(
            move |send| send(doctor::run(&ctx, snap.as_ref())),
            move |checks: Vec<Check>| {
                let Some(me) = me.upgrade() else { return };
                me.run_btn.set_sensitive(true);
                widgets::clear(&me.list);
                let problems = checks.iter().filter(|c| matches!(c.level, Level::Fail | Level::Warn)).count();
                me.status.set_text(&if problems == 0 {
                    format!("All {} checks look fine.", checks.len())
                } else {
                    format!("{problems} of {} checks need attention.", checks.len())
                });
                for c in checks {
                    me.list.append(&check_card(&me, c));
                }
            },
        );
    }
}

fn check_card(view: &Rc<DoctorView>, c: Check) -> gtk::Widget {
    let card = widgets::card();
    let line = hbox(14);
    let (icon, color) = match c.level {
        Level::Fail => ("x", theme::RED),
        Level::Warn => ("warn", theme::GOLD),
        Level::Info => ("info", theme::BLUE),
        Level::Ok => ("check", theme::GREEN),
    };
    line.append(&super::colored_icon(icon, 22, color));
    let text = vbox(3);
    text.set_hexpand(true);
    text.append(&label(&c.title, &["bold"]));
    text.append(&widgets::wrapped(&c.summary, &["dim"]));
    if let (Some(h), true) = (&c.hint, c.level != Level::Ok) {
        text.append(&widgets::wrapped(h, &["small", "faint"]));
    }
    line.append(&text);
    if let (Some(a), true) = (&c.action, c.level != Level::Ok) {
        let title = match a.as_str() {
            "clean" => "Open Clean",
            "optimize" => "Open Optimize",
            "settings" => "Open Settings",
            "apps" => "Open Apps",
            _ => "Open",
        };
        let b = widgets::pill(title, &["ghost", "small-btn"]);
        b.set_valign(gtk::Align::Center);
        let app = view.app.clone();
        let target = a.clone();
        b.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.navigate(&target);
            }
        });
        line.append(&b);
    }
    card.append(&line);
    card.upcast()
}
