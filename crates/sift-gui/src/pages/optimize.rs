//! Optimize: safe maintenance with the exact commands visible before running.

use crate::app::AppRef;
use crate::theme;
use crate::widgets::{self, hbox, icons, label, vbox};
use crate::worker;
use gtk::prelude::*;
use sift_core::optimize::{self, OptTask, TaskResult};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

struct Row {
    check: gtk::CheckButton,
    status: gtk::Box,
    output: gtk::Label,
}

struct Page {
    app: Weak<crate::app::App>,
    list: gtk::Box,
    tasks: RefCell<Vec<OptTask>>,
    rows: RefCell<HashMap<String, Row>>,
    run_btn: gtk::Button,
    summary: gtk::Label,
    loaded: std::cell::Cell<bool>,
}

pub fn build(app: &AppRef) -> gtk::Widget {
    let root = vbox(16);
    root.add_css_class("page");
    root.add_css_class("accent-optimize");
    let head = hbox(12);
    head.append(&widgets::page_header("Optimize", "Small turns, a smoother run."));
    head.append(&widgets::spacer());
    let refresh = icons::button("refresh", None);
    refresh.add_css_class("ghost");
    refresh.add_css_class("pill");
    refresh.set_valign(gtk::Align::Center);
    refresh.set_tooltip_text(Some("Check again"));
    head.append(&refresh);
    root.append(&head);
    let list = vbox(10);
    root.append(&widgets::scrolled(&list));
    let bar = hbox(14);
    bar.add_css_class("bottom-bar");
    let summary = label("", &["dim"]);
    summary.set_hexpand(true);
    summary.set_wrap(true);
    bar.append(&summary);
    let run_btn = widgets::pill("Run selected", &["accent-btn"]);
    bar.append(&run_btn);
    root.append(&bar);

    let page = Rc::new(Page {
        app: Rc::downgrade(app),
        list,
        tasks: RefCell::new(Vec::new()),
        rows: RefCell::new(HashMap::new()),
        run_btn: run_btn.clone(),
        summary,
        loaded: std::cell::Cell::new(false),
    });
    app.keep(page.clone());
    let p = Rc::downgrade(&page);
    refresh.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.load();
        }
    });
    let p = Rc::downgrade(&page);
    run_btn.connect_clicked(move |_| {
        if let Some(p) = p.upgrade() {
            p.confirm_run();
        }
    });
    let p = Rc::downgrade(&page);
    app.on_page(move |name| {
        if name == "optimize" {
            if let Some(p) = p.upgrade() {
                if !p.loaded.replace(true) {
                    p.load();
                }
            }
        }
    });
    root.upcast()
}

impl Page {
    fn load(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let ctx = app.ctx();
        widgets::clear(&self.list);
        let spinner = gtk::Spinner::new();
        spinner.start();
        spinner.set_margin_top(40);
        self.list.append(&spinner);
        self.summary.set_text("Checking what can be tuned…");
        self.run_btn.set_sensitive(false);
        let me = Rc::downgrade(self);
        worker::run(move |send| send(optimize::plan(&ctx)), move |tasks: Vec<OptTask>| {
            if let Some(me) = me.upgrade() {
                me.show(tasks);
            }
        });
    }

    fn show(self: &Rc<Self>, tasks: Vec<OptTask>) {
        widgets::clear(&self.list);
        self.rows.borrow_mut().clear();
        let mut header_shown = (false, false);
        for t in &tasks {
            if !t.needs_root && !header_shown.0 {
                self.list.append(&label("As you", &["section-title"]));
                header_shown.0 = true;
            }
            if t.needs_root && !header_shown.1 {
                let h = label("As administrator — one password prompt for all of them", &["section-title"]);
                h.set_margin_top(10);
                self.list.append(&h);
                header_shown.1 = true;
            }
            let card = widgets::card();
            let line = hbox(14);
            let check = gtk::CheckButton::new();
            check.set_active(t.default_on);
            check.set_valign(gtk::Align::Start);
            line.append(&check);
            let text = vbox(4);
            text.set_hexpand(true);
            let title_row = hbox(8);
            title_row.append(&label(&t.title, &["bold"]));
            if t.needs_root {
                title_row.append(&widgets::chip("admin", "info"));
            }
            text.append(&title_row);
            text.append(&widgets::wrapped(&t.description, &["small", "dim"]));
            let cmd = widgets::wrapped(&t.commands.join("  ;  "), &["mono", "tiny", "faint"]);
            cmd.set_selectable(true);
            text.append(&cmd);
            if !t.preview.is_empty() {
                let exp = gtk::Expander::new(Some(&format!("{} item{}", t.preview.len(), if t.preview.len() == 1 { "" } else { "s" })));
                let pv = vbox(2);
                for p in t.preview.iter().take(200) {
                    pv.append(&label(p, &["mono", "tiny", "dim"]));
                }
                exp.set_child(Some(&pv));
                text.append(&exp);
            }
            let output = widgets::wrapped("", &["mono", "tiny", "dim"]);
            output.set_visible(false);
            text.append(&output);
            line.append(&text);
            let status = hbox(0);
            status.set_valign(gtk::Align::Center);
            line.append(&status);
            card.append(&line);
            self.list.append(&card);
            let me = Rc::downgrade(self);
            check.connect_toggled(move |_| {
                if let Some(me) = me.upgrade() {
                    me.update_summary();
                }
            });
            self.rows.borrow_mut().insert(t.id.clone(), Row { check, status, output });
        }
        if tasks.is_empty() {
            self.list.append(&widgets::empty_state("optimize", "Nothing to tune", "None of the maintenance tools Sift uses were found on this system."));
        }
        *self.tasks.borrow_mut() = tasks;
        self.update_summary();
    }

    fn selected(&self) -> Vec<String> {
        self.rows.borrow().iter().filter(|(_, r)| r.check.is_active()).map(|(id, _)| id.clone()).collect()
    }

    fn update_summary(&self) {
        let sel = self.selected();
        let tasks = self.tasks.borrow();
        let admin = tasks.iter().filter(|t| t.needs_root && sel.contains(&t.id)).count();
        self.run_btn.set_sensitive(!sel.is_empty());
        self.summary.set_text(&match (sel.len(), admin) {
            (0, _) => "Tick the tasks you want to run.".to_string(),
            (n, 0) => format!("{n} task{} selected. Nothing is deleted except broken launchers (to the Trash).", if n == 1 { "" } else { "s" }),
            (n, a) => format!("{n} tasks selected, {a} need your password once."),
        });
    }

    fn confirm_run(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let ids = self.selected();
        let tasks = self.tasks.borrow();
        let admin: Vec<String> = tasks.iter().filter(|t| t.needs_root && ids.contains(&t.id)).flat_map(|t| t.commands.clone()).collect();
        drop(tasks);
        if admin.is_empty() {
            self.run(ids, false);
            return;
        }
        let me = Rc::downgrade(self);
        let extra = widgets::command_list(&admin);
        widgets::confirm(
            &app.window,
            "Run admin maintenance?",
            "These commands run as administrator after one password prompt:",
            "Run",
            false,
            Some(&extra),
            move || {
                if let Some(me) = me.upgrade() {
                    me.run(ids.clone(), true);
                }
            },
        );
    }

    fn run(self: &Rc<Self>, ids: Vec<String>, allow_root: bool) {
        let Some(app) = self.app.upgrade() else { return };
        let ctx = app.ctx();
        for (id, r) in self.rows.borrow().iter() {
            widgets::clear(&r.status);
            r.output.set_visible(false);
            if ids.contains(id) {
                let s = gtk::Spinner::new();
                s.start();
                r.status.append(&s);
            }
        }
        self.run_btn.set_sensitive(false);
        self.summary.set_text("Running…");
        let me = Rc::downgrade(self);
        worker::run(
            move |send| {
                let results = optimize::run(&ctx, &ids, false, allow_root, &mut |_, r| {
                    if let Some(r) = r {
                        send((Some(r.clone()), None));
                    }
                });
                send((None, Some(results)));
            },
            move |(one, all): (Option<TaskResult>, Option<Vec<TaskResult>>)| {
                let Some(me) = me.upgrade() else { return };
                if let Some(r) = one {
                    if let Some(row) = me.rows.borrow().get(&r.id) {
                        widgets::clear(&row.status);
                        let (icon, color) = if r.ok { ("check", theme::GREEN) } else { ("x", theme::RED) };
                        row.status.append(&super::colored_icon(icon, 20, color));
                        if !r.output.trim().is_empty() {
                            row.output.set_text(r.output.trim());
                            row.output.set_visible(true);
                        }
                    }
                }
                if let Some(all) = all {
                    let failed = all.iter().filter(|r| !r.ok).count();
                    me.run_btn.set_sensitive(true);
                    me.summary.set_text(&if failed == 0 {
                        format!("All {} tasks finished.", all.len())
                    } else {
                        format!("{} of {} tasks finished; {failed} had problems (details under each task).", all.len() - failed, all.len())
                    });
                    if let Some(app) = me.app.upgrade() {
                        app.toast(if failed == 0 { "Maintenance finished" } else { "Maintenance finished with problems" });
                    }
                }
            },
        );
    }
}
