//! Line icons drawn with Cairo on a 24×24 grid (2 px strokes, round caps),
//! tinted with the widget's current CSS color — so they follow hover,
//! active and light/dark states like text does.

use gtk::cairo::{Context, LineCap, LineJoin};
use gtk::prelude::*;
use std::f64::consts::PI;

fn rr(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

fn line(cr: &Context, pts: &[(f64, f64)]) {
    if let Some((x, y)) = pts.first() {
        cr.move_to(*x, *y);
        for (x, y) in &pts[1..] {
            cr.line_to(*x, *y);
        }
    }
}

fn circle(cr: &Context, x: f64, y: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x, y, r, 0.0, 2.0 * PI);
}

fn star(cr: &Context, cx: f64, cy: f64, r: f64) {
    // Four-point sparkle with concave sides.
    cr.move_to(cx, cy - r);
    cr.curve_to(cx + r * 0.15, cy - r * 0.15, cx + r * 0.15, cy - r * 0.15, cx + r, cy);
    cr.curve_to(cx + r * 0.15, cy + r * 0.15, cx + r * 0.15, cy + r * 0.15, cx, cy + r);
    cr.curve_to(cx - r * 0.15, cy + r * 0.15, cx - r * 0.15, cy + r * 0.15, cx - r, cy);
    cr.curve_to(cx - r * 0.15, cy - r * 0.15, cx - r * 0.15, cy - r * 0.15, cx, cy - r);
    cr.close_path();
}

/// Draw icon `name` into a `size`×`size` box at the origin.
pub fn draw(cr: &Context, name: &str, size: f64, rgba: (f64, f64, f64, f64)) {
    let _ = cr.save();
    let s = size / 24.0;
    cr.scale(s, s);
    cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
    cr.set_line_width(1.9);
    cr.set_line_cap(LineCap::Round);
    cr.set_line_join(LineJoin::Round);
    let mut fill = false;
    match name {
        "monitor" => line(cr, &[(2.0, 12.0), (6.0, 12.0), (9.0, 5.0), (14.0, 19.0), (17.0, 12.0), (22.0, 12.0)]),
        "clean" => {
            star(cr, 10.0, 13.0, 7.5);
            star(cr, 18.5, 5.5, 3.0);
        }
        "optimize" => {
            line(cr, &[(4.0, 20.0), (15.0, 9.0)]);
            line(cr, &[(13.0, 7.0), (17.0, 11.0)]);
            star(cr, 18.5, 4.5, 2.6);
            star(cr, 7.0, 5.0, 2.0);
            star(cr, 20.0, 15.0, 1.8);
        }
        "apps" => {
            line(cr, &[(12.0, 2.5), (21.0, 7.0), (21.0, 17.0), (12.0, 21.5), (3.0, 17.0), (3.0, 7.0), (12.0, 2.5)]);
            line(cr, &[(3.0, 7.0), (12.0, 11.5), (21.0, 7.0)]);
            line(cr, &[(12.0, 11.5), (12.0, 21.5)]);
            line(cr, &[(7.5, 4.8), (16.5, 9.3)]);
        }
        "analyze" => {
            rr(cr, 3.0, 3.0, 7.5, 7.5, 1.6);
            rr(cr, 13.5, 3.0, 7.5, 7.5, 1.6);
            rr(cr, 3.0, 13.5, 7.5, 7.5, 1.6);
            rr(cr, 13.5, 13.5, 7.5, 7.5, 1.6);
        }
        "dupes" => {
            rr(cr, 8.0, 8.0, 13.0, 13.0, 2.0);
            line(cr, &[(4.5, 16.0), (3.0, 16.0)]);
            cr.new_sub_path();
            line(cr, &[(3.0, 14.5), (3.0, 5.0)]);
            cr.arc(5.0, 5.0, 2.0, PI, 1.5 * PI);
            line(cr, &[(5.0, 3.0), (14.5, 3.0)]);
            cr.arc(14.5, 5.0, 2.0, 1.5 * PI, 2.0 * PI);
            line(cr, &[(16.5, 5.0), (16.5, 6.0)]);
        }
        "ports" => {
            circle(cr, 6.0, 12.0, 2.6);
            circle(cr, 18.0, 5.5, 2.6);
            circle(cr, 18.0, 18.5, 2.6);
            line(cr, &[(8.3, 10.7), (15.7, 6.8)]);
            line(cr, &[(8.3, 13.3), (15.7, 17.2)]);
        }
        "settings" => {
            circle(cr, 12.0, 12.0, 3.0);
            for i in 0..8 {
                let a = i as f64 * PI / 4.0;
                line(cr, &[(12.0 + a.cos() * 7.0, 12.0 + a.sin() * 7.0), (12.0 + a.cos() * 9.6, 12.0 + a.sin() * 9.6)]);
            }
            circle(cr, 12.0, 12.0, 7.0);
        }
        "cpu" => {
            rr(cr, 5.0, 5.0, 14.0, 14.0, 2.0);
            rr(cr, 9.0, 9.0, 6.0, 6.0, 1.0);
            for p in [9.0, 12.0, 15.0] {
                line(cr, &[(p, 2.0), (p, 5.0)]);
                line(cr, &[(p, 19.0), (p, 22.0)]);
                line(cr, &[(2.0, p), (5.0, p)]);
                line(cr, &[(19.0, p), (22.0, p)]);
            }
        }
        "memory" => {
            rr(cr, 2.0, 7.0, 20.0, 10.0, 1.5);
            for x in [6.0, 10.0, 14.0, 18.0] {
                line(cr, &[(x, 10.0), (x, 14.0)]);
            }
            for x in [5.0, 9.0, 15.0, 19.0] {
                line(cr, &[(x, 17.0), (x, 20.0)]);
            }
        }
        "gpu" => {
            rr(cr, 2.0, 6.0, 20.0, 12.0, 2.0);
            circle(cr, 9.0, 12.0, 3.0);
            line(cr, &[(15.0, 10.0), (19.0, 10.0)]);
            line(cr, &[(15.0, 14.0), (19.0, 14.0)]);
            line(cr, &[(5.0, 18.0), (5.0, 21.0)]);
        }
        "disk" => {
            rr(cr, 2.0, 13.0, 20.0, 8.0, 2.0);
            line(cr, &[(2.5, 14.5), (5.5, 5.0), (18.5, 5.0), (21.5, 14.5)]);
            circle(cr, 17.5, 17.0, 0.6);
            line(cr, &[(6.0, 17.0), (12.0, 17.0)]);
        }
        "network" => {
            circle(cr, 12.0, 12.0, 9.5);
            cr.new_sub_path();
            cr.save().ok();
            cr.translate(12.0, 12.0);
            cr.scale(0.45, 1.0);
            cr.arc(0.0, 0.0, 9.5, 0.0, 2.0 * PI);
            cr.restore().ok();
            line(cr, &[(2.5, 12.0), (21.5, 12.0)]);
        }
        "fan" => {
            circle(cr, 12.0, 12.0, 1.8);
            for i in 0..3 {
                let a = i as f64 * 2.0 * PI / 3.0;
                cr.save().ok();
                cr.translate(12.0, 12.0);
                cr.rotate(a);
                cr.move_to(0.0, -2.5);
                cr.curve_to(-1.0, -8.0, 3.5, -10.5, 5.5, -8.0);
                cr.curve_to(6.5, -6.0, 3.0, -4.0, 2.2, -1.2);
                cr.restore().ok();
            }
        }
        "battery" => {
            rr(cr, 2.0, 7.0, 17.0, 10.0, 2.0);
            line(cr, &[(22.0, 10.5), (22.0, 13.5)]);
            line(cr, &[(6.0, 10.5), (6.0, 13.5)]);
            line(cr, &[(10.0, 10.5), (10.0, 13.5)]);
        }
        "trash" => {
            line(cr, &[(3.0, 6.0), (21.0, 6.0)]);
            line(cr, &[(5.5, 6.0), (6.5, 20.0), (17.5, 20.0), (18.5, 6.0)]);
            line(cr, &[(9.0, 6.0), (9.0, 3.5), (15.0, 3.5), (15.0, 6.0)]);
            line(cr, &[(10.0, 10.0), (10.0, 16.0)]);
            line(cr, &[(14.0, 10.0), (14.0, 16.0)]);
        }
        "folder" => line(cr, &[(3.0, 6.0), (3.0, 19.0), (21.0, 19.0), (21.0, 8.0), (11.5, 8.0), (9.5, 5.0), (3.0, 5.0), (3.0, 6.0)]),
        "refresh" => {
            cr.new_sub_path();
            cr.arc(12.0, 12.0, 8.0, -0.3 * PI, 1.55 * PI);
            line(cr, &[(18.0, 2.5), (19.4, 7.4), (14.5, 8.6)]);
        }
        "stop" => rr(cr, 6.0, 6.0, 12.0, 12.0, 2.0),
        "check" => line(cr, &[(4.5, 12.5), (9.5, 17.5), (19.5, 6.5)]),
        "x" => {
            line(cr, &[(6.0, 6.0), (18.0, 18.0)]);
            line(cr, &[(18.0, 6.0), (6.0, 18.0)]);
        }
        "warn" => {
            line(cr, &[(12.0, 3.0), (22.0, 20.5), (2.0, 20.5), (12.0, 3.0)]);
            line(cr, &[(12.0, 9.5), (12.0, 14.0)]);
            circle(cr, 12.0, 17.2, 0.4);
        }
        "info" => {
            circle(cr, 12.0, 12.0, 9.5);
            line(cr, &[(12.0, 11.0), (12.0, 16.5)]);
            circle(cr, 12.0, 7.8, 0.4);
        }
        "back" => line(cr, &[(15.0, 5.0), (8.0, 12.0), (15.0, 19.0)]),
        "forward" => line(cr, &[(9.0, 5.0), (16.0, 12.0), (9.0, 19.0)]),
        "up" => line(cr, &[(5.0, 15.0), (12.0, 8.0), (19.0, 15.0)]),
        "plus" => {
            line(cr, &[(12.0, 5.0), (12.0, 19.0)]);
            line(cr, &[(5.0, 12.0), (19.0, 12.0)]);
        }
        "search" => {
            circle(cr, 10.5, 10.5, 6.5);
            line(cr, &[(15.5, 15.5), (20.5, 20.5)]);
        }
        "external" => {
            line(cr, &[(13.0, 4.0), (20.0, 4.0), (20.0, 11.0)]);
            line(cr, &[(20.0, 4.0), (11.0, 13.0)]);
            line(cr, &[(17.0, 14.0), (17.0, 20.0), (4.0, 20.0), (4.0, 7.0), (10.0, 7.0)]);
        }
        "home" => line(cr, &[(3.0, 11.0), (12.0, 3.5), (21.0, 11.0), (19.0, 11.0), (19.0, 20.0), (5.0, 20.0), (5.0, 11.0), (3.0, 11.0)]),
        "history" => {
            cr.new_sub_path();
            cr.arc(12.0, 12.0, 8.5, -0.85 * PI, 1.0 * PI);
            line(cr, &[(3.5, 5.0), (3.4, 9.3), (7.6, 9.0)]);
            line(cr, &[(12.0, 7.5), (12.0, 12.0), (15.0, 14.0)]);
        }
        "shield" => {
            line(cr, &[(12.0, 2.5), (20.0, 5.5), (20.0, 11.0)]);
            cr.curve_to(20.0, 16.0, 16.5, 19.5, 12.0, 21.5);
            cr.curve_to(7.5, 19.5, 4.0, 16.0, 4.0, 11.0);
            line(cr, &[(4.0, 11.0), (4.0, 5.5), (12.0, 2.5)]);
        }
        "box-open" => {
            line(cr, &[(3.0, 8.0), (12.0, 12.5), (21.0, 8.0)]);
            line(cr, &[(3.0, 8.0), (3.0, 17.0), (12.0, 21.5), (21.0, 17.0), (21.0, 8.0)]);
            line(cr, &[(12.0, 12.5), (12.0, 21.5)]);
            line(cr, &[(3.0, 8.0), (7.0, 3.5), (16.0, 3.5), (21.0, 8.0)]);
        }
        "code" => {
            line(cr, &[(8.0, 6.0), (2.5, 12.0), (8.0, 18.0)]);
            line(cr, &[(16.0, 6.0), (21.5, 12.0), (16.0, 18.0)]);
            line(cr, &[(13.5, 4.0), (10.5, 20.0)]);
        }
        "download" => {
            line(cr, &[(12.0, 3.0), (12.0, 15.0)]);
            line(cr, &[(7.0, 10.5), (12.0, 15.5), (17.0, 10.5)]);
            line(cr, &[(4.0, 16.0), (4.0, 20.0), (20.0, 20.0), (20.0, 16.0)]);
        }
        "dot" => {
            circle(cr, 12.0, 12.0, 5.0);
            fill = true;
        }
        _ => circle(cr, 12.0, 12.0, 8.0),
    }
    if fill {
        let _ = cr.fill();
    } else {
        let _ = cr.stroke();
    }
    let _ = cr.restore();
}

/// A widget showing icon `name` at `px` pixels in the current text color.
pub fn icon(name: &str, px: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(px);
    area.set_content_height(px);
    area.set_valign(gtk::Align::Center);
    area.set_halign(gtk::Align::Center);
    area.set_can_target(false);
    let name = name.to_string();
    area.set_draw_func(move |a, cr, w, h| {
        let c = a.color();
        let size = w.min(h) as f64;
        cr.translate((w as f64 - size) / 2.0, (h as f64 - size) / 2.0);
        draw(cr, &name, size, (c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64));
    });
    area.connect_state_flags_changed(|a, _| a.queue_draw());
    area
}

/// A button holding an icon and optional label.
pub fn button(name: &str, label: Option<&str>) -> gtk::Button {
    let b = gtk::Button::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&icon(name, 16));
    if let Some(l) = label {
        row.append(&gtk::Label::new(Some(l)));
    }
    row.set_halign(gtk::Align::Center);
    b.set_child(Some(&row));
    b
}
