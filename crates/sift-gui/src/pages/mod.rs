pub mod activity;
pub mod analyze;
pub mod apps;
pub mod clean;
pub mod doctor;
pub mod dupes;
pub mod history;
pub mod monitor;
pub mod optimize;
pub mod ports;
pub mod settings;

use crate::theme::Rgb;
use crate::widgets::{hbox, icons};
use gtk::prelude::*;

/// Icon drawn in a fixed color (not the text color).
pub fn colored_icon(name: &str, px: i32, color: Rgb) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(px);
    area.set_content_height(px);
    area.set_valign(gtk::Align::Center);
    let name = name.to_string();
    area.set_draw_func(move |_, cr, w, h| {
        let size = w.min(h) as f64;
        icons::draw(cr, &name, size, color.rgba(1.0));
    });
    area
}

/// "◆ CPU" style card heading in a metric color, with a right-aligned badge.
pub fn card_heading(icon: &str, title: &str, color: Rgb) -> (gtk::Box, gtk::Label) {
    let row = hbox(8);
    row.append(&colored_icon(icon, 16, color));
    let t = gtk::Label::new(None);
    t.set_markup(&format!("<span foreground='{}'>{}</span>", color.css(), gtk::glib::markup_escape_text(&title.to_uppercase())));
    t.add_css_class("card-title");
    row.append(&t);
    let sp = hbox(0);
    sp.set_hexpand(true);
    row.append(&sp);
    let badge = crate::widgets::chip("", "");
    badge.set_visible(false);
    row.append(&badge);
    (row, badge)
}

pub fn set_badge(badge: &gtk::Label, text: &str, kind: &str) {
    badge.set_text(text);
    badge.set_visible(!text.is_empty());
    for k in ["good", "warn", "bad", "info"] {
        badge.remove_css_class(k);
    }
    if !kind.is_empty() {
        badge.add_css_class(kind);
    }
}
