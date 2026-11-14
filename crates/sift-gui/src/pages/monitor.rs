//! Monitor (Burrow's Home): Overview · History · Activity · Doctor.

use super::{card_heading, set_badge};
use crate::app::{AppRef, Recording};
use crate::theme::{self, Rgb};
use crate::widgets::{self, charts, hbox, label, vbox};
use gtk::pango;
use gtk::prelude::*;
use sift_core::format::{self, bytes, rate};
use sift_core::status::Snapshot;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Line colors for GPU 1, 2, 3 in the GPU card.
const GPU_COLORS: [Rgb; 3] = [theme::CORAL, theme::AZURE, theme::TEAL];

struct Metric {
    root: gtk::Box,
    badge: gtk::Label,
    value: gtk::Label,
    unit: gtk::Label,
    chart: gtk::DrawingArea,
    foot: gtk::Label,
}

fn metric(icon: &str, title: &str, color: Rgb) -> Metric {
    let root = widgets::card();
    root.set_hexpand(true);
    let (head, badge) = card_heading(icon, title, color);
    root.append(&head);
    let vrow = hbox(6);
    let value = label("—", &["big-number"]);
    let unit = label("", &["unit"]);
    unit.set_valign(gtk::Align::End);
    unit.set_margin_bottom(8);
    vrow.append(&value);
    vrow.append(&unit);
    root.append(&vrow);
    let chart = gtk::DrawingArea::new();
    chart.set_content_height(54);
    chart.set_hexpand(true);
    root.append(&chart);
    let foot = label("", &["mono", "small", "dim"]);
    foot.set_ellipsize(pango::EllipsizeMode::End);
    root.append(&foot);
    Metric { root, badge, value, unit, chart, foot }
}

pub fn build(app: &AppRef) -> gtk::Widget {
    let root = vbox(0);
    root.add_css_class("accent-monitor");

    let head = hbox(12);
    head.set_margin_start(22);
    head.set_margin_end(26);
    head.set_margin_bottom(10);
    let seg = hbox(2);
    seg.add_css_class("seg");
    head.append(&seg);
    head.append(&widgets::spacer());
    let rec = widgets::chip("", "");
    head.append(&rec);
    root.append(&head);
    let hair = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    hair.add_css_class("hair");
    root.append(&hair);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_vexpand(true);
    stack.add_named(&overview(app), Some("overview"));
    let history = super::history::build(app);
    stack.add_named(&history.widget, Some("history"));
    let activity = super::activity::build(app);
    stack.add_named(&activity.widget, Some("activity"));
    let doctor = super::doctor::build(app);
    stack.add_named(&doctor.widget, Some("doctor"));
    root.append(&stack);

    let sections = [("overview", "Overview"), ("history", "History"), ("activity", "Activity"), ("doctor", "Doctor")];
    let mut first: Option<gtk::ToggleButton> = None;
    for (id, title) in sections {
        let b = gtk::ToggleButton::with_label(title);
        b.add_css_class("seg-btn");
        match &first {
            Some(f) => b.set_group(Some(f)),
            None => {
                b.set_active(true);
                first = Some(b.clone());
            }
        }
        let stack = stack.clone();
        let (h, a, d) = (history.clone(), activity.clone(), doctor.clone());
        b.connect_toggled(move |b| {
            if b.is_active() {
                stack.set_visible_child_name(id);
                match id {
                    "history" => h.refresh(),
                    "activity" => a.refresh(),
                    "doctor" => d.ensure_run(),
                    _ => {}
                }
            }
        });
        seg.append(&b);
        let a = Rc::downgrade(app);
        let b2 = b.clone();
        app.debug_action(&format!("monitor.{id}"), move || {
            if let Some(a) = a.upgrade() {
                a.navigate("monitor");
            }
            b2.set_active(true);
        });
    }
    // Re-entering Monitor refreshes whichever section is showing.
    let stack2 = stack.clone();
    let (h, a) = (history.clone(), activity.clone());
    app.on_page(move |p| {
        if p == "monitor" {
            match stack2.visible_child_name().as_deref() {
                Some("history") => h.refresh(),
                Some("activity") => a.refresh(),
                _ => {}
            }
        }
    });

    app.on_live(move |app| {
        let live = app.live.borrow();
        match live.recording {
            Some(Recording::App) => {
                rec.set_text("● recording while open");
                rec.set_tooltip_text(Some("History is written while this window is open. Turn on background recording in Settings to keep it going."));
            }
            Some(Recording::Service) => {
                rec.set_text("● recording in background");
                rec.set_tooltip_text(Some("The sift-sampler service records history."));
            }
            None => rec.set_text(""),
        }
    });
    root.upcast()
}

fn overview(app: &AppRef) -> gtk::Widget {
    let page = vbox(18);
    page.add_css_class("page");
    page.set_margin_top(14);

    // Hero: ring + score + host
    let hero = hbox(22);
    let ring = gtk::DrawingArea::new();
    ring.set_content_width(116);
    ring.set_content_height(116);
    let score = Rc::new(Cell::new(0u8));
    let s2 = score.clone();
    ring.set_draw_func(move |_, cr, w, _| {
        let v = s2.get();
        let text = if v == 0 { String::new() } else { v.to_string() };
        charts::ring(cr, w as f64, v as f64 / 100.0, theme::health_color(v), &text, "Geist Mono Bold 22");
    });
    hero.append(&ring);
    let hv = vbox(2);
    hv.set_valign(gtk::Align::Center);
    let srow = hbox(10);
    let big = label("—", &["huge-number"]);
    let lab = gtk::Label::new(None);
    lab.add_css_class("section-title");
    lab.set_valign(gtk::Align::End);
    lab.set_margin_bottom(14);
    srow.append(&big);
    srow.append(&lab);
    hv.append(&srow);
    let msg = label("Reading your system…", &["dim"]);
    hv.append(&msg);
    hero.append(&hv);
    hero.append(&widgets::spacer());
    let host = vbox(4);
    host.set_valign(gtk::Align::Center);
    let host1 = label("", &["mono", "dim"]);
    host1.set_xalign(1.0);
    let host2 = label("", &["mono", "dim"]);
    host2.set_xalign(1.0);
    host.append(&host1);
    host.append(&host2);
    hero.append(&host);
    page.append(&hero);
    let hair = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    hair.add_css_class("hair");
    page.append(&hair);

    // Metric cards
    let grid = gtk::Grid::new();
    grid.set_row_spacing(16);
    grid.set_column_spacing(16);
    grid.set_column_homogeneous(true);
    let cpu = metric("cpu", "CPU", theme::GREEN);
    let mem = metric("memory", "Memory", theme::GOLD);
    let gpu = metric("gpu", "GPU", theme::CORAL);
    let disk = metric("disk", "Disk", theme::AZURE);
    let net = metric("network", "Network", theme::TEAL);
    let fan = metric("fan", "Fans", theme::LILAC);
    let bat = metric("battery", "Battery", theme::GOLD);
    grid.attach(&cpu.root, 0, 0, 1, 1);
    grid.attach(&mem.root, 1, 0, 1, 1);
    grid.attach(&gpu.root, 2, 0, 1, 1);
    grid.attach(&disk.root, 0, 1, 1, 1);
    grid.attach(&net.root, 1, 1, 1, 1);
    grid.attach(&fan.root, 2, 1, 1, 1);
    grid.attach(&bat.root, 0, 2, 3, 1);
    fan.root.set_visible(false);
    bat.root.set_visible(false);
    page.append(&grid);

    // Chart draw funcs read the live buffers.
    let w = Rc::downgrade(app);
    cpu.chart.set_draw_func(move |_, cr, width, height| {
        if let Some(app) = w.upgrade() {
            if let Some(s) = app.live.borrow().snap.as_ref() {
                charts::bars(cr, width as f64, height as f64, &s.cpu.per_core, theme::GREEN);
            }
        }
    });
    let w = Rc::downgrade(app);
    mem.chart.set_draw_func(move |_, cr, width, height| {
        if let Some(app) = w.upgrade() {
            let d: Vec<f64> = app.live.borrow().mem.iter().cloned().collect();
            charts::sparkline(cr, width as f64, height as f64, &d, 100.0, theme::GOLD);
        }
    });
    let w = Rc::downgrade(app);
    gpu.chart.set_draw_func(move |_, cr, width, height| {
        if let Some(app) = w.upgrade() {
            let series: Vec<(Vec<f64>, Rgb)> = app
                .live
                .borrow()
                .gpus
                .iter()
                .enumerate()
                .map(|(i, q)| (q.iter().cloned().collect(), GPU_COLORS[i % GPU_COLORS.len()]))
                .collect();
            charts::multi(cr, width as f64, height as f64, &series, 10.0);
        }
    });
    let w = Rc::downgrade(app);
    disk.chart.set_draw_func(move |_, cr, width, height| {
        if let Some(app) = w.upgrade() {
            if let Some(d) = app.live.borrow().snap.as_ref().and_then(|s| s.root_disk().cloned()) {
                let h = 10.0;
                cr.translate(0.0, (height as f64 - h) / 2.0);
                charts::gauge(cr, width as f64, h, d.used_pct / 100.0, theme::GOLD, if d.used_pct > 90.0 { theme::RED } else { theme::ORANGE });
            }
        }
    });
    let w = Rc::downgrade(app);
    net.chart.set_draw_func(move |_, cr, width, height| {
        if let Some(app) = w.upgrade() {
            let live = app.live.borrow();
            let rx: Vec<f64> = live.rx.iter().cloned().collect();
            let tx: Vec<f64> = live.tx.iter().cloned().collect();
            charts::dual(cr, width as f64, height as f64, &rx, &tx, theme::BLUE, theme::GREEN);
        }
    });
    let w = Rc::downgrade(app);
    fan.chart.set_draw_func(move |_, cr, width, height| {
        if let Some(app) = w.upgrade() {
            let d: Vec<f64> = app.live.borrow().fan.iter().cloned().collect();
            charts::sparkline(cr, width as f64, height as f64, &d, 0.0, theme::LILAC);
        }
    });
    let w = Rc::downgrade(app);
    bat.chart.set_draw_func(move |_, cr, width, height| {
        if let Some(app) = w.upgrade() {
            if let Some(b) = app.live.borrow().snap.as_ref().and_then(|s| s.batteries.first().cloned()) {
                let h = 10.0;
                cr.translate(0.0, (height as f64 - h) / 2.0);
                let c = if b.percent < 20.0 { theme::RED } else { theme::GREEN };
                charts::gauge(cr, width as f64, h, b.percent / 100.0, c, c);
            }
        }
    });

    // Processes
    let procs_card = widgets::card();
    let ph = hbox(10);
    let (heading, _) = card_heading("monitor", "Top processes", theme::GOLD);
    heading.set_hexpand(true);
    ph.append(&heading);
    let sort_seg = hbox(2);
    sort_seg.add_css_class("seg");
    let by_cpu = gtk::ToggleButton::with_label("CPU");
    by_cpu.add_css_class("seg-btn");
    by_cpu.set_active(true);
    let by_mem = gtk::ToggleButton::with_label("Memory");
    by_mem.add_css_class("seg-btn");
    by_mem.set_group(Some(&by_cpu));
    sort_seg.append(&by_cpu);
    sort_seg.append(&by_mem);
    ph.append(&sort_seg);
    procs_card.append(&ph);
    let table_holder = vbox(0);
    procs_card.append(&table_holder);
    page.append(&procs_card);
    let sort_mem = Rc::new(Cell::new(false));
    {
        let sm = sort_mem.clone();
        let w = Rc::downgrade(app);
        let th = table_holder.clone();
        by_mem.connect_toggled(move |b| {
            sm.set(b.is_active());
            if let Some(app) = w.upgrade() {
                if let Some(s) = app.live.borrow().snap.as_ref() {
                    fill_procs(&th, s, sm.get());
                }
            }
        });
    }

    let cards = Rc::new(RefCell::new((cpu, mem, gpu, disk, net, fan, bat)));
    let ring2 = ring.clone();
    app.on_live(move |app| {
        let live = app.live.borrow();
        let Some(s) = live.snap.as_ref() else { return };
        score.set(s.health.score);
        ring2.queue_draw();
        big.set_text(&s.health.score.to_string());
        lab.set_markup(&format!(
            "<span foreground='{}'>{}</span>",
            theme::health_color(s.health.score).css(),
            gtk::glib::markup_escape_text(&s.health.label)
        ));
        msg.set_text(&s.health.message);
        let ram = bytes(s.memory.total);
        host1.set_text(&format!("{} · {} · {}", s.model, ram, s.os));
        host2.set_text(&format!("{} · up {} · kernel {}", s.host, format::duration(s.uptime_secs), s.kernel));

        let c = cards.borrow();
        let (cpu, mem, gpu, disk, net, fan, bat) = &*c;
        // CPU
        cpu.value.set_text(&format!("{:.1}", s.cpu.usage));
        cpu.unit.set_text("%");
        match s.cpu.temp_c {
            Some(t) => set_badge(&cpu.badge, &format!("{t:.0}°C"), if t >= 85.0 { "bad" } else if t >= 70.0 { "warn" } else { "" }),
            None => set_badge(&cpu.badge, "", ""),
        }
        let freq = s.cpu.freq_mhz.map(|f| format!(" · {:.1} GHz", f / 1000.0)).unwrap_or_default();
        cpu.foot.set_text(&format!("load {:.2} · {:.2} · {:.2} · {} threads{freq}", s.cpu.load1, s.cpu.load5, s.cpu.load15, s.cpu.logical));
        cpu.chart.queue_draw();
        // Memory
        mem.value.set_text(&format!("{:.0}", s.memory.used_pct));
        mem.unit.set_text("%");
        let pk = match s.memory.pressure.as_str() {
            "critical" => "bad",
            "warning" => "warn",
            _ => "",
        };
        set_badge(&mem.badge, &s.memory.pressure, pk);
        mem.foot.set_text(&format!("{} / {} · swap {}", bytes(s.memory.used), bytes(s.memory.total), bytes(s.memory.swap_used)));
        mem.chart.queue_draw();
        // GPU: one line per card; the big number is the busiest one.
        if s.gpus.is_empty() {
            gpu.value.set_text("—");
            gpu.unit.set_text("");
            gpu.foot.set_text("No GPU detected");
        } else {
            match s.gpu_usage() {
                Some(u) => {
                    gpu.value.set_text(&format!("{u:.0}"));
                    gpu.unit.set_text("%");
                }
                None => {
                    let asleep = s.gpus.iter().all(|g| g.state == "sleeping");
                    gpu.value.set_text(if asleep { "zZ" } else { "—" });
                    gpu.unit.set_text(if asleep { "asleep" } else { "" });
                }
            }
            match s.gpus.iter().filter_map(|g| g.temp_c).reduce(f64::max) {
                Some(t) => set_badge(&gpu.badge, &format!("{t:.0}°C"), if t >= 85.0 { "bad" } else if t >= 75.0 { "warn" } else { "" }),
                None => set_badge(&gpu.badge, "", ""),
            }
            let parts: Vec<String> = s
                .gpus
                .iter()
                .enumerate()
                .map(|(i, g)| {
                    let color = GPU_COLORS[i % GPU_COLORS.len()].css();
                    let usage = match (g.usage, g.state.as_str()) {
                        (_, "sleeping") => "asleep".to_string(),
                        (Some(u), _) if u < 1.0 && g.pstate.as_deref() == Some("P8") => "idle".to_string(),
                        (Some(u), _) => format!("{u:.0}%"),
                        (None, _) => "—".to_string(),
                    };
                    let mut extra = Vec::new();
                    if let (Some(u), Some(t)) = (g.mem_used, g.mem_total) {
                        if t > 0 {
                            extra.push(format!("{} / {}", bytes(u), bytes(t)));
                        }
                    }
                    if let Some(p) = g.power_w {
                        extra.push(format!("{p:.0} W"));
                    }
                    if g.drives_display {
                        extra.push("display".into());
                    }
                    let extra = if extra.is_empty() { String::new() } else { format!(" ({})", extra.join(", ")) };
                    format!(
                        "<span foreground='{color}'>●</span> {}",
                        gtk::glib::markup_escape_text(&format!("{} {usage}{extra}", sift_core::status::gpu::short_name(&g.name)))
                    )
                })
                .collect();
            gpu.foot.set_markup(&parts.join("  "));
            gpu.foot.set_tooltip_text(Some(&s.gpus.iter().map(|g| g.name.clone()).collect::<Vec<_>>().join("\n")));
        }
        gpu.chart.queue_draw();
        // Disk
        if let Some(d) = s.root_disk() {
            let (n, u) = format::bytes_parts(d.free);
            disk.value.set_text(&n);
            disk.unit.set_text(&format!("{u} free"));
            set_badge(&disk.badge, &bytes(d.total), "");
            disk.foot.set_text(&format!(
                "{:.0}% used · R {} · W {}",
                d.used_pct,
                rate(s.disk_io.read_bps),
                rate(s.disk_io.write_bps)
            ));
        }
        disk.chart.queue_draw();
        // Network
        let total = s.net_rx_bps + s.net_tx_bps;
        let (n, u) = format::bytes_parts(total as u64);
        net.value.set_text(&n);
        net.unit.set_text(&format!("{u}/s"));
        let iface = s.primary_net();
        set_badge(&net.badge, iface.map(|i| if i.wireless { "Wi-Fi" } else { "Ethernet" }).unwrap_or(""), "info");
        net.foot.set_text(&format!(
            "↓ {}  ↑ {}{}",
            rate(s.net_rx_bps),
            rate(s.net_tx_bps),
            iface.map(|i| format!(" · {}{}", i.name, i.ip.as_ref().map(|ip| format!(" · {ip}")).unwrap_or_default())).unwrap_or_default()
        ));
        net.chart.queue_draw();
        // Fans
        if let Some(f) = s.thermal.fans.first() {
            fan.root.set_visible(true);
            fan.value.set_text(&f.rpm.to_string());
            fan.unit.set_text("RPM");
            set_badge(&fan.badge, &format!("{} fan{}", s.thermal.fans.len(), if s.thermal.fans.len() == 1 { "" } else { "s" }), "");
            let all: Vec<String> = s.thermal.fans.iter().map(|f| format!("{} {}", f.label, f.rpm)).collect();
            fan.foot.set_text(&all.join(" · "));
            fan.chart.queue_draw();
        }
        // Battery
        if let Some(b) = s.batteries.first() {
            bat.root.set_visible(true);
            bat.value.set_text(&format!("{:.0}", b.percent));
            bat.unit.set_text(&format!("% · {}", b.status.to_lowercase()));
            if let Some(h) = b.health_pct {
                set_badge(&bat.badge, &format!("{h:.0}% health"), if h < 70.0 { "warn" } else { "good" });
            }
            let mut parts = Vec::new();
            if let Some(t) = b.time_left_secs {
                parts.push(format!("{} {}", format::duration(t), if b.status == "Charging" { "to full" } else { "left" }));
            }
            if let Some(c) = b.cycles {
                parts.push(format!("{c} cycles"));
            }
            if let Some(p) = b.power_w {
                parts.push(format!("{p:.1} W"));
            }
            if s.ac_online == Some(true) {
                parts.push("on AC power".into());
            }
            bat.foot.set_text(&parts.join(" · "));
            bat.chart.queue_draw();
        }
        drop(c);
        if app.current_page() == "monitor" {
            fill_procs(&table_holder, s, sort_mem.get());
        }
    });

    widgets::scrolled(&page).upcast()
}

fn fill_procs(holder: &gtk::Box, s: &Snapshot, by_mem: bool) {
    widgets::clear(holder);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(18);
    grid.set_row_spacing(6);
    let heads = ["NAME", "PID", "CPU", "MEMORY", "USER"];
    for (i, h) in heads.iter().enumerate() {
        let l = label(h, &["table-head"]);
        if i > 0 {
            l.set_xalign(1.0);
        }
        if i == 0 {
            l.set_hexpand(true);
        }
        grid.attach(&l, i as i32, 0, 1, 1);
    }
    let mut procs: Vec<_> = s.top_processes.iter().collect();
    if by_mem {
        procs.sort_by(|a, b| b.mem_bytes.cmp(&a.mem_bytes));
    } else {
        procs.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
    }
    for (row, p) in procs.iter().take(12).enumerate() {
        let r = row as i32 + 1;
        let name = label(&p.name, &[]);
        name.set_ellipsize(pango::EllipsizeMode::End);
        name.set_tooltip_text(Some(&p.command));
        name.set_hexpand(true);
        grid.attach(&name, 0, r, 1, 1);
        let cells = [
            p.pid.to_string(),
            format!("{:.1}%", p.cpu),
            bytes(p.mem_bytes),
            p.user.clone(),
        ];
        for (i, c) in cells.iter().enumerate() {
            let l = label(c, &["mono", "small"]);
            l.set_xalign(1.0);
            if i == 1 && p.cpu >= 80.0 {
                l.add_css_class("accent");
            }
            if i != 1 {
                l.add_css_class("dim");
            }
            grid.attach(&l, i as i32 + 1, r, 1, 1);
        }
    }
    holder.append(&grid);
}
