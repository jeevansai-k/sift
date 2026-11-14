//! Activity: what Sift has done, with the exact paths for every removal.

use crate::app::AppRef;
use crate::widgets::{self, hbox, icons, label, vbox};
use crate::worker;
use gtk::pango;
use gtk::prelude::*;
use sift_core::db::{Db, Operation, Removal};
use sift_core::format::{self, bytes};
use std::rc::Rc;

pub struct ActivityView {
    pub widget: gtk::Widget,
    app: std::rc::Weak<crate::app::App>,
    list: gtk::ListBox,
    total: gtk::Label,
}

pub fn build(app: &AppRef) -> Rc<ActivityView> {
    let page = vbox(14);
    page.add_css_class("page");
    page.set_margin_top(14);
    let top = hbox(10);
    let total = label("", &["section-title"]);
    top.append(&total);
    top.append(&widgets::spacer());
    let refresh = icons::button("refresh", Some("Refresh"));
    refresh.add_css_class("ghost");
    refresh.add_css_class("small-btn");
    top.append(&refresh);
    page.append(&top);
    let list = gtk::ListBox::new();
    list.add_css_class("list");
    list.set_selection_mode(gtk::SelectionMode::None);
    page.append(&list);
    let view = Rc::new(ActivityView { widget: widgets::scrolled(&page).upcast(), app: Rc::downgrade(app), list, total });
    let v = Rc::downgrade(&view);
    refresh.connect_clicked(move |_| {
        if let Some(v) = v.upgrade() {
            v.refresh();
        }
    });
    view
}

fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "clean" => "clean",
        "purge" => "code",
        "installer" => "download",
        "optimize" => "optimize",
        "uninstall" => "apps",
        "duplicates" => "dupes",
        _ => "dot",
    }
}

impl ActivityView {
    pub fn refresh(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let path = app.paths.db_file();
        let me = Rc::downgrade(self);
        worker::run(
            move |send| {
                let r = Db::open(&path).and_then(|db| Ok((db.operations(200)?, db.reclaimed_total())));
                send(r.map_err(|e| e.message));
            },
            move |r: Result<(Vec<Operation>, i64), String>| {
                let Some(me) = me.upgrade() else { return };
                widgets::clear_list(&me.list);
                match r {
                    Ok((ops, total)) => {
                        me.total.set_text(&format!("{} handled by Sift so far", bytes(total.max(0) as u64)));
                        if ops.is_empty() {
                            me.list.append(&widgets::empty_state("history", "Nothing yet", "Cleanups, uninstalls and maintenance runs will show up here, with every file they touched."));
                        }
                        for op in ops {
                            me.list.append(&op_row(&me, op));
                        }
                    }
                    Err(e) => me.total.set_text(&format!("Activity unavailable: {e}")),
                }
            },
        );
    }
}

fn op_row(view: &Rc<ActivityView>, op: Operation) -> gtk::Widget {
    let outer = vbox(4);
    outer.add_css_class("row-box");
    let line = hbox(12);
    line.append(&icons::icon(kind_icon(&op.kind), 20));
    let text = vbox(2);
    text.set_hexpand(true);
    let mut title = op.kind.clone();
    if let Some(f) = title.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    text.append(&label(&format!("{title}{}", if op.dry_run { " (preview)" } else { "" }), &["bold"]));
    let summary = label(&op.summary, &["small", "dim"]);
    summary.set_ellipsize(pango::EllipsizeMode::End);
    text.append(&summary);
    line.append(&text);
    let kind = match op.status.as_str() {
        "done" => "good",
        "partial" | "cancelled" => "warn",
        "failed" => "bad",
        _ => "info",
    };
    line.append(&widgets::chip(&op.status, kind));
    let when = label(&format::ago(op.started, sift_core::now_unix()), &["mono", "small", "faint"]);
    when.set_tooltip_text(Some(&format::datetime(op.started)));
    when.set_width_chars(13);
    when.set_xalign(1.0);
    line.append(&when);
    outer.append(&line);

    let details = vbox(2);
    details.set_margin_start(32);
    let revealer = gtk::Revealer::new();
    revealer.set_child(Some(&details));
    outer.append(&revealer);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&outer));
    let click = gtk::GestureClick::new();
    let app = view.app.clone();
    let loaded = std::cell::Cell::new(false);
    let id = op.id;
    click.connect_released(move |_, _, _, _| {
        revealer.set_reveal_child(!revealer.reveals_child());
        if loaded.replace(true) {
            return;
        }
        let Some(app) = app.upgrade() else { return };
        let path = app.paths.db_file();
        let d = details.clone();
        let home = app.paths.home.clone();
        worker::run(
            move |send| send(Db::open(&path).and_then(|db| db.removals(Some(id), 500)).map_err(|e| e.message)),
            move |r: Result<Vec<Removal>, String>| {
                widgets::clear(&d);
                match r {
                    Ok(rows) if rows.is_empty() => d.append(&label("No individual files recorded.", &["small", "dim"])),
                    Ok(rows) => {
                        for rm in rows {
                            let l = hbox(10);
                            let mark = match rm.status.as_str() {
                                "trashed" | "removed" | "done" => "check",
                                "refused" | "skipped" => "info",
                                _ => "x",
                            };
                            l.append(&icons::icon(mark, 14));
                            let p = label(&format::tilde(std::path::Path::new(&rm.path), &home), &["mono", "tiny"]);
                            p.set_ellipsize(pango::EllipsizeMode::Middle);
                            p.set_hexpand(true);
                            p.set_tooltip_text(Some(&rm.path));
                            l.append(&p);
                            let what = if rm.reason.is_empty() { format!("{} · {}", rm.action, rm.status) } else { format!("{} · {}", rm.status, rm.reason) };
                            l.append(&label(&what, &["tiny", "faint"]));
                            l.append(&label(&if rm.bytes > 0 { bytes(rm.bytes as u64) } else { String::new() }, &["mono", "tiny", "dim"]));
                            d.append(&l);
                        }
                    }
                    Err(e) => d.append(&label(&e, &["small", "dim"])),
                }
            },
        );
    });
    row.add_controller(click);
    row.upcast()
}
