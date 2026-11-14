//! History: long-range charts over the local SQLite history.

use super::card_heading;
use crate::app::AppRef;
use crate::theme::{self, Rgb};
use crate::widgets::{self, charts, hbox, label, vbox};
use crate::worker;
use gtk::prelude::*;
use sift_core::db::{Db, ProcUsage, Sample, UsageMetric};
use sift_core::format::{bytes, rate};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const RANGES: &[(&str, i64)] = &[("5m", 300), ("1h", 3600), ("6h", 21_600), ("24h", 86_400), ("7d", 604_800), ("30d", 2_592_000), ("90d", 7_776_000)];

struct Data {
    samples: Vec<Sample>,
    top: Vec<ProcUsage>,
    stats: (i64, Option<i64>, Option<i64>),
}

pub struct HistoryView {
    pub widget: gtk::Widget,
    app: std::rc::Weak<crate::app::App>,
    range: Cell<i64>,
    data: Rc<RefCell<Option<Data>>>,
    charts: Vec<gtk::DrawingArea>,
    status: gtk::Label,
    top_holder: gtk::Box,
}

type ChartSpec = (&'static str, &'static str, Rgb);
const CHARTS: &[ChartSpec] = &[
    ("cpu", "CPU", theme::GREEN),
    ("memory", "Memory", theme::GOLD),
    ("disk", "Free disk space", theme::AZURE),
    ("network", "Network", theme::TEAL),
    ("cpu", "Temperature", theme::CORAL),
    ("gpu", "GPU", theme::ORANGE),
];

pub fn build(app: &AppRef) -> Rc<HistoryView> {
    let page = vbox(16);
    page.add_css_class("page");
    page.set_margin_top(14);
    let top = hbox(12);
    let seg = hbox(2);
    seg.add_css_class("seg");
    top.append(&seg);
    top.append(&widgets::spacer());
    let status = label("", &["mono", "small", "dim"]);
    top.append(&status);
    page.append(&top);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(16);
    grid.set_column_spacing(16);
    grid.set_column_homogeneous(true);
    let data: Rc<RefCell<Option<Data>>> = Rc::new(RefCell::new(None));
    let mut areas = Vec::new();
    for (i, (icon, title, color)) in CHARTS.iter().enumerate() {
        let card = widgets::card();
        let (head, _) = card_heading(icon, title, *color);
        card.append(&head);
        let area = gtk::DrawingArea::new();
        area.set_content_height(170);
        area.set_hexpand(true);
        let d = data.clone();
        let idx = i;
        let color = *color;
        area.set_draw_func(move |_, cr, w, h| {
            let data = d.borrow();
            let Some(data) = data.as_ref() else { return };
            let ts: Vec<i64> = data.samples.iter().map(|s| s.ts).collect();
            let some = |f: &dyn Fn(&Sample) -> Option<f64>| data.samples.iter().map(f).collect::<Vec<_>>();
            let (w, h) = (w as f64, h as f64);
            match idx {
                0 => {
                    let v = some(&|s| Some(s.cpu));
                    charts::line_chart(cr, w, h, &ts, &[charts::Series { values: &v, color }], Some(100.0), &|x| format!("{x:.0}%"));
                }
                1 => {
                    let v = some(&|s| Some(s.mem_pct));
                    charts::line_chart(cr, w, h, &ts, &[charts::Series { values: &v, color }], Some(100.0), &|x| format!("{x:.0}%"));
                }
                2 => {
                    let v = some(&|s| Some(s.disk_free as f64));
                    charts::line_chart(cr, w, h, &ts, &[charts::Series { values: &v, color }], None, &|x| bytes(x as u64));
                }
                3 => {
                    let rx = some(&|s| Some(s.net_rx));
                    let tx = some(&|s| Some(s.net_tx));
                    charts::line_chart(
                        cr,
                        w,
                        h,
                        &ts,
                        &[charts::Series { values: &rx, color: theme::BLUE }, charts::Series { values: &tx, color: theme::GREEN }],
                        None,
                        &|x| rate(x),
                    );
                }
                4 => {
                    let v = some(&|s| s.cpu_temp);
                    let g = some(&|s| s.gpu_temp);
                    charts::line_chart(
                        cr,
                        w,
                        h,
                        &ts,
                        &[charts::Series { values: &v, color }, charts::Series { values: &g, color: theme::ORANGE }],
                        None,
                        &|x| format!("{x:.0}°"),
                    );
                }
                _ => {
                    let v = some(&|s| s.gpu);
                    charts::line_chart(cr, w, h, &ts, &[charts::Series { values: &v, color }], Some(100.0), &|x| format!("{x:.0}%"));
                }
            }
        });
        card.append(&area);
        grid.attach(&card, (i % 2) as i32, (i / 2) as i32, 1, 1);
        areas.push(area);
    }
    page.append(&grid);

    let tcard = widgets::card();
    let (head, _) = card_heading("monitor", "Busiest processes in this range", theme::GOLD);
    tcard.append(&head);
    let top_holder = vbox(0);
    tcard.append(&top_holder);
    page.append(&tcard);

    let view = Rc::new(HistoryView {
        widget: widgets::scrolled(&page).upcast(),
        app: Rc::downgrade(app),
        range: Cell::new(3600),
        data,
        charts: areas,
        status,
        top_holder,
    });

    let mut first: Option<gtk::ToggleButton> = None;
    for (name, secs) in RANGES {
        let b = gtk::ToggleButton::with_label(name);
        b.add_css_class("seg-btn");
        match &first {
            Some(f) => b.set_group(Some(f)),
            None => first = Some(b.clone()),
        }
        if *secs == 3600 {
            b.set_active(true);
        }
        let v = Rc::downgrade(&view);
        let secs = *secs;
        b.connect_toggled(move |b| {
            if b.is_active() {
                if let Some(v) = v.upgrade() {
                    v.range.set(secs);
                    v.refresh();
                }
            }
        });
        seg.append(&b);
    }

    // Keep it fresh while visible.
    let v = Rc::downgrade(&view);
    gtk::glib::timeout_add_seconds_local(30, move || {
        let Some(v) = v.upgrade() else { return gtk::glib::ControlFlow::Break };
        if v.widget.is_mapped() {
            v.refresh();
        }
        gtk::glib::ControlFlow::Continue
    });
    view
}

impl HistoryView {
    pub fn refresh(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let path = app.paths.db_file();
        let span = self.range.get();
        let me = Rc::downgrade(self);
        worker::run(
            move |send| {
                let now = sift_core::now_unix();
                let from = now - span;
                let res = Db::open(&path).map(|db| Data {
                    samples: db.samples(from, now, 240).unwrap_or_default(),
                    top: db.process_usage(from, now, 12, UsageMetric::PeakCpu).unwrap_or_default(),
                    stats: db.sample_stats(),
                });
                send(res.map_err(|e| e.message));
            },
            move |res: Result<Data, String>| {
                let Some(me) = me.upgrade() else { return };
                match res {
                    Ok(d) => {
                        let (count, _first, last) = d.stats;
                        let recording = me.app.upgrade().and_then(|a| {
                            let r = a.live.borrow().recording;
                            r
                        });
                        let who = match recording {
                            Some(crate::app::Recording::Service) => "recording in background",
                            Some(crate::app::Recording::App) => "recording while Sift is open",
                            None => "",
                        };
                        let newest = last.map(|l| sift_core::format::ago(l, sift_core::now_unix())).unwrap_or_else(|| "never".into());
                        me.status.set_text(&format!("{count} samples · newest {newest} · {who}"));
                        fill_top(&me.top_holder, &d.top);
                        *me.data.borrow_mut() = Some(d);
                        for c in &me.charts {
                            c.queue_draw();
                        }
                    }
                    Err(e) => me.status.set_text(&format!("History unavailable: {e}")),
                }
            },
        );
    }
}

fn fill_top(holder: &gtk::Box, top: &[ProcUsage]) {
    widgets::clear(holder);
    if top.is_empty() {
        holder.append(&label("No process history in this range yet.", &["dim"]));
        return;
    }
    let grid = gtk::Grid::new();
    grid.set_column_spacing(20);
    grid.set_row_spacing(6);
    for (i, h) in ["PROCESS", "PEAK CPU", "AVG CPU", "PEAK MEMORY", "SEEN"].iter().enumerate() {
        let l = label(h, &["table-head"]);
        if i > 0 {
            l.set_xalign(1.0);
        } else {
            l.set_hexpand(true);
        }
        grid.attach(&l, i as i32, 0, 1, 1);
    }
    for (r, p) in top.iter().enumerate() {
        let r = r as i32 + 1;
        let n = label(&p.name, &[]);
        n.set_hexpand(true);
        grid.attach(&n, 0, r, 1, 1);
        for (i, c) in [format!("{:.1}%", p.peak_cpu), format!("{:.1}%", p.avg_cpu), bytes(p.peak_mem as u64), format!("{}×", p.samples)].iter().enumerate() {
            let l = label(c, &["mono", "small", "dim"]);
            l.set_xalign(1.0);
            grid.attach(&l, i as i32 + 1, r, 1, 1);
        }
    }
    holder.append(&grid);
}
