//! Cairo drawing for cards and charts: sparklines, per-core bars, gauges,
//! the health ring, history line charts and the "found" orb.

use crate::theme::{self, Rgb};
use gtk::cairo::{Context, LinearGradient, LineCap, RadialGradient};
use gtk::pango;
use std::f64::consts::PI;

fn src(cr: &Context, c: Rgb, a: f64) {
    let (r, g, b, a) = c.rgba(a);
    cr.set_source_rgba(r, g, b, a);
}

/// Filled sparkline of `data` scaled to `max` (auto when `max <= 0`).
pub fn sparkline(cr: &Context, w: f64, h: f64, data: &[f64], max: f64, color: Rgb) {
    if data.len() < 2 {
        return;
    }
    let max = if max > 0.0 { max } else { data.iter().cloned().fold(0.0, f64::max).max(1e-9) * 1.15 };
    let n = data.len();
    let x = |i: usize| i as f64 / (n - 1) as f64 * w;
    let y = |v: f64| h - (v / max).clamp(0.0, 1.0) * (h - 3.0) - 1.5;
    cr.move_to(x(0), y(data[0]));
    for (i, v) in data.iter().enumerate().skip(1) {
        cr.line_to(x(i), y(*v));
    }
    let path = cr.copy_path();
    cr.line_to(w, h);
    cr.line_to(0.0, h);
    cr.close_path();
    let grad = LinearGradient::new(0.0, 0.0, 0.0, h);
    let (r, g, b, _) = color.rgba(1.0);
    grad.add_color_stop_rgba(0.0, r, g, b, 0.28);
    grad.add_color_stop_rgba(1.0, r, g, b, 0.0);
    let _ = cr.set_source(&grad);
    let _ = cr.fill();
    if let Ok(p) = path {
        cr.new_path();
        cr.append_path(&p);
        src(cr, color, 1.0);
        cr.set_line_width(1.8);
        cr.set_line_cap(LineCap::Round);
        let _ = cr.stroke();
    }
}

/// Two series sharing one scale (network down/up).
pub fn dual(cr: &Context, w: f64, h: f64, a: &[f64], b: &[f64], ca: Rgb, cb: Rgb) {
    let max = a.iter().chain(b.iter()).cloned().fold(0.0, f64::max).max(1.0) * 1.15;
    for (data, c) in [(a, ca), (b, cb)] {
        if data.len() < 2 {
            continue;
        }
        let n = data.len();
        cr.move_to(0.0, h - data[0] / max * (h - 3.0) - 1.5);
        for (i, v) in data.iter().enumerate().skip(1) {
            cr.line_to(i as f64 / (n - 1) as f64 * w, h - v / max * (h - 3.0) - 1.5);
        }
        src(cr, c, 1.0);
        cr.set_line_width(1.8);
        let _ = cr.stroke();
    }
}

/// Several series on one auto-scaled chart (never scaled below `floor`),
/// each a line with a soft fill — used for one line per GPU.
pub fn multi(cr: &Context, w: f64, h: f64, series: &[(Vec<f64>, Rgb)], floor: f64) {
    let peak = series.iter().flat_map(|(d, _)| d.iter()).cloned().fold(0.0, f64::max);
    let max = (peak * 1.2).max(floor).max(1e-9);
    for (data, color) in series {
        if data.len() < 2 {
            continue;
        }
        let n = data.len();
        let x = |i: usize| i as f64 / (n - 1) as f64 * w;
        let y = |v: f64| h - (v / max).clamp(0.0, 1.0) * (h - 3.0) - 1.5;
        cr.move_to(x(0), y(data[0]));
        for (i, v) in data.iter().enumerate().skip(1) {
            cr.line_to(x(i), y(*v));
        }
        let path = cr.copy_path();
        cr.line_to(w, h);
        cr.line_to(0.0, h);
        cr.close_path();
        let grad = LinearGradient::new(0.0, 0.0, 0.0, h);
        let (r, g, b, _) = color.rgba(1.0);
        grad.add_color_stop_rgba(0.0, r, g, b, 0.20);
        grad.add_color_stop_rgba(1.0, r, g, b, 0.0);
        let _ = cr.set_source(&grad);
        let _ = cr.fill();
        if let Ok(p) = path {
            cr.new_path();
            cr.append_path(&p);
            src(cr, *color, 1.0);
            cr.set_line_width(1.8);
            cr.set_line_cap(LineCap::Round);
            let _ = cr.stroke();
        }
    }
}

/// Per-core vertical bars (0–100).
pub fn bars(cr: &Context, w: f64, h: f64, values: &[f64], color: Rgb) {
    if values.is_empty() {
        return;
    }
    let n = values.len() as f64;
    let gap = if n > 24.0 { 2.0 } else { 4.0 };
    let bw = ((w - gap * (n - 1.0)) / n).max(1.0);
    let pal = theme::palette();
    for (i, v) in values.iter().enumerate() {
        let x = i as f64 * (bw + gap);
        theme::set_source(cr, pal.track());
        rounded(cr, x, 0.0, bw, h, bw.min(4.0) / 2.0);
        let _ = cr.fill();
        let bh = (v / 100.0).clamp(0.02, 1.0) * h;
        src(cr, color, 0.92);
        rounded(cr, x, h - bh, bw, bh, bw.min(4.0) / 2.0);
        let _ = cr.fill();
    }
}

/// Horizontal gauge with a warm gradient (disk usage).
pub fn gauge(cr: &Context, w: f64, h: f64, frac: f64, from: Rgb, to: Rgb) {
    let pal = theme::palette();
    let r = h / 2.0;
    theme::set_source(cr, pal.track());
    rounded(cr, 0.0, 0.0, w, h, r);
    let _ = cr.fill();
    let fw = (frac.clamp(0.0, 1.0) * w).max(h);
    let grad = LinearGradient::new(0.0, 0.0, w, 0.0);
    let (r1, g1, b1, _) = from.rgba(1.0);
    let (r2, g2, b2, _) = to.rgba(1.0);
    grad.add_color_stop_rgba(0.0, r1, g1, b1, 1.0);
    grad.add_color_stop_rgba(1.0, r2, g2, b2, 1.0);
    let _ = cr.set_source(&grad);
    rounded(cr, 0.0, 0.0, fw, h, r);
    let _ = cr.fill();
}

pub fn rounded(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

/// Progress ring with centered text.
pub fn ring(cr: &Context, size: f64, frac: f64, color: Rgb, text: &str, font: &str) {
    let pal = theme::palette();
    let c = size / 2.0;
    let lw = (size * 0.075).max(4.0);
    let r = c - lw;
    cr.set_line_width(lw);
    cr.set_line_cap(LineCap::Round);
    theme::set_source(cr, pal.track());
    cr.new_sub_path();
    cr.arc(c, c, r, 0.0, 2.0 * PI);
    let _ = cr.stroke();
    src(cr, color, 1.0);
    cr.new_sub_path();
    let start = -PI / 2.0;
    cr.arc(c, c, r, start, start + 2.0 * PI * frac.clamp(0.0, 1.0));
    let _ = cr.stroke();
    if !text.is_empty() {
        let layout = pangocairo::functions::create_layout(cr);
        layout.set_text(text);
        layout.set_font_description(Some(&pango::FontDescription::from_string(font)));
        let (tw, th) = layout.pixel_size();
        cr.move_to(c - tw as f64 / 2.0, c - th as f64 / 2.0);
        theme::set_source(cr, pal.ink(1.0));
        pangocairo::functions::show_layout(cr, &layout);
    }
}

/// Soft glowing orb (scan / result screens). `phase` animates the glow.
pub fn orb(cr: &Context, w: f64, h: f64, color: Rgb, phase: f64) {
    let c = (w / 2.0, h / 2.0);
    let r = w.min(h) / 2.0 * (0.86 + 0.06 * (phase * 2.0 * PI).sin());
    let (cr_, cg, cb, _) = color.rgba(1.0);
    let halo = RadialGradient::new(c.0, c.1, r * 0.6, c.0, c.1, r * 1.2);
    halo.add_color_stop_rgba(0.0, cr_, cg, cb, 0.22);
    halo.add_color_stop_rgba(1.0, cr_, cg, cb, 0.0);
    let _ = cr.set_source(&halo);
    cr.arc(c.0, c.1, r * 1.2, 0.0, 2.0 * PI);
    let _ = cr.fill();
    let body = RadialGradient::new(c.0 - r * 0.3, c.1 - r * 0.35, r * 0.1, c.0, c.1, r);
    body.add_color_stop_rgba(0.0, (cr_ + 0.12).min(1.0), (cg + 0.12).min(1.0), (cb + 0.12).min(1.0), 1.0);
    body.add_color_stop_rgba(1.0, cr_ * 0.62, cg * 0.62, cb * 0.62, 1.0);
    let _ = cr.set_source(&body);
    cr.arc(c.0, c.1, r * 0.82, 0.0, 2.0 * PI);
    let _ = cr.fill();
}

pub struct Series<'a> {
    pub values: &'a [Option<f64>],
    pub color: Rgb,
}

/// Time-series chart with a light grid, y labels and x time labels.
pub fn line_chart(cr: &Context, w: f64, h: f64, ts: &[i64], series: &[Series], max_hint: Option<f64>, fmt: &dyn Fn(f64) -> String) {
    let pal = theme::palette();
    let bottom = 20.0;
    let maxv = series.iter().flat_map(|s| s.values.iter().flatten()).cloned().fold(0.0, f64::max);
    let max = max_hint.unwrap_or(0.0).max(maxv * 1.12).max(1e-9);
    let font = pango::FontDescription::from_string("Geist Mono 8.5");
    // Leave room for the widest y-axis label.
    let widest = (0..=3)
        .map(|i| {
            let layout = pangocairo::functions::create_layout(cr);
            layout.set_text(&fmt(max * (1.0 - i as f64 / 3.0)));
            layout.set_font_description(Some(&font));
            layout.pixel_size().0
        })
        .max()
        .unwrap_or(40) as f64;
    let left = widest + 14.0;
    let pw = (w - left - 6.0).max(10.0);
    let ph = (h - bottom - 8.0).max(10.0);
    let label = |cr: &Context, text: &str, x: f64, y: f64, right_align: bool| {
        let layout = pangocairo::functions::create_layout(cr);
        layout.set_text(text);
        layout.set_font_description(Some(&font));
        let (tw, th) = layout.pixel_size();
        let x = if right_align { x - tw as f64 } else { x };
        cr.move_to(x, y - th as f64 / 2.0);
        theme::set_source(cr, pal.ink(0.45));
        pangocairo::functions::show_layout(cr, &layout);
    };
    // grid
    for i in 0..=3 {
        let y = 8.0 + ph * i as f64 / 3.0;
        theme::set_source(cr, pal.ink(0.07));
        cr.set_line_width(1.0);
        cr.move_to(left, y.round() + 0.5);
        cr.line_to(left + pw, y.round() + 0.5);
        let _ = cr.stroke();
        label(cr, &fmt(max * (1.0 - i as f64 / 3.0)), left - 8.0, y, true);
    }
    if ts.len() < 2 {
        label(cr, "Not enough history yet", left + 12.0, 8.0 + ph / 2.0, false);
        return;
    }
    let (t0, t1) = (ts[0], *ts.last().unwrap());
    let span = (t1 - t0).max(1) as f64;
    let x = |t: i64| left + (t - t0) as f64 / span * pw;
    let y = |v: f64| 8.0 + ph - (v / max).clamp(0.0, 1.0) * ph;
    // x labels
    for i in 0..=3 {
        let t = t0 + ((t1 - t0) as f64 * i as f64 / 3.0) as i64;
        let text = time_label(t, span);
        let tx = x(t);
        let layout = pangocairo::functions::create_layout(cr);
        layout.set_text(&text);
        layout.set_font_description(Some(&font));
        let (tw, _) = layout.pixel_size();
        let lx = (tx - tw as f64 / 2.0).clamp(left, left + pw - tw as f64);
        cr.move_to(lx, h - bottom + 4.0);
        theme::set_source(cr, pal.ink(0.45));
        pangocairo::functions::show_layout(cr, &layout);
    }
    for s in series {
        let mut started = false;
        let mut first_x = 0.0;
        let mut last_x = 0.0;
        cr.new_path();
        for (i, v) in s.values.iter().enumerate() {
            match v {
                Some(v) => {
                    let (px, py) = (x(ts[i]), y(*v));
                    if !started {
                        cr.move_to(px, py);
                        started = true;
                        first_x = px;
                    } else {
                        cr.line_to(px, py);
                    }
                    last_x = px;
                }
                None => {}
            }
        }
        if !started {
            continue;
        }
        let path = cr.copy_path();
        cr.line_to(last_x, 8.0 + ph);
        cr.line_to(first_x, 8.0 + ph);
        cr.close_path();
        let grad = LinearGradient::new(0.0, 8.0, 0.0, 8.0 + ph);
        let (r, g, b, _) = s.color.rgba(1.0);
        grad.add_color_stop_rgba(0.0, r, g, b, if series.len() > 1 { 0.10 } else { 0.22 });
        grad.add_color_stop_rgba(1.0, r, g, b, 0.0);
        let _ = cr.set_source(&grad);
        let _ = cr.fill();
        if let Ok(p) = path {
            cr.append_path(&p);
            src(cr, s.color, 1.0);
            cr.set_line_width(1.6);
            let _ = cr.stroke();
        }
    }
}

fn time_label(t: i64, span: f64) -> String {
    use chrono_like::fmt_time;
    fmt_time(t, span)
}

mod chrono_like {
    /// Local time label: HH:MM for short spans, "Mon 14" for long ones.
    pub fn fmt_time(t: i64, span: f64) -> String {
        let dt = gtk::glib::DateTime::from_unix_local(t);
        match dt {
            Ok(d) => {
                let f = if span > 2.0 * 86_400.0 { "%b %d" } else { "%H:%M" };
                d.format(f).map(|s| s.to_string()).unwrap_or_default()
            }
            Err(_) => String::new(),
        }
    }
}
