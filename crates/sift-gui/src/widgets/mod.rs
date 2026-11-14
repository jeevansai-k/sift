//! Small building blocks shared by every page.

pub mod charts;
pub mod icons;
pub mod review;
pub mod treemap;

use adw::prelude::*;
use gtk::pango;

pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}


pub fn wrapped(text: &str, classes: &[&str]) -> gtk::Label {
    let l = label(text, classes);
    l.set_wrap(true);
    l.set_wrap_mode(pango::WrapMode::WordChar);
    l
}

pub fn chip(text: &str, kind: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("chip");
    if !kind.is_empty() {
        l.add_css_class(kind);
    }
    l.set_valign(gtk::Align::Center);
    l
}

pub fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}

pub fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}

pub fn spacer() -> gtk::Box {
    let b = hbox(0);
    b.set_hexpand(true);
    b
}

pub fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

pub fn clear_list(l: &gtk::ListBox) {
    while let Some(c) = l.first_child() {
        l.remove(&c);
    }
}

pub fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_hscrollbar_policy(gtk::PolicyType::Never);
    s.set_vexpand(true);
    s.set_child(Some(child));
    s
}

pub fn pill(text: &str, classes: &[&str]) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.add_css_class("pill");
    for c in classes {
        b.add_css_class(c);
    }
    b
}

/// Title + italic tagline, Burrow style.
pub fn page_header(title: &str, tagline: &str) -> gtk::Box {
    let v = vbox(2);
    v.append(&label(title, &["page-title"]));
    if !tagline.is_empty() {
        v.append(&label(tagline, &["tagline"]));
    }
    v
}

pub fn card() -> gtk::Box {
    let c = vbox(10);
    c.add_css_class("card");
    c
}

/// Centered "nothing here" block.
pub fn empty_state(icon: &str, title: &str, body: &str) -> gtk::Box {
    let v = vbox(10);
    v.set_valign(gtk::Align::Center);
    v.set_halign(gtk::Align::Center);
    v.set_vexpand(true);
    let i = icons::icon(icon, 40);
    i.add_css_class("faint");
    v.append(&i);
    let t = gtk::Label::new(Some(title));
    t.add_css_class("empty-title");
    v.append(&t);
    let b = gtk::Label::new(Some(body));
    b.add_css_class("dim");
    b.set_wrap(true);
    b.set_justify(gtk::Justification::Center);
    b.set_max_width_chars(60);
    v.append(&b);
    v
}

/// Ask before doing something. `on_yes` runs only on the confirm response.
pub fn confirm(
    parent: &impl IsA<gtk::Window>,
    heading: &str,
    body: &str,
    confirm_label: &str,
    destructive: bool,
    extra: Option<&gtk::Widget>,
    on_yes: impl Fn() + 'static,
) {
    let d = adw::MessageDialog::new(Some(parent), Some(heading), Some(body));
    d.add_response("cancel", "Cancel");
    d.add_response("ok", confirm_label);
    d.set_response_appearance("ok", if destructive { adw::ResponseAppearance::Destructive } else { adw::ResponseAppearance::Suggested });
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    if let Some(w) = extra {
        d.set_extra_child(Some(w));
    }
    d.connect_response(None, move |_, r| {
        if r == "ok" {
            on_yes();
        }
    });
    d.present();
}

/// Monospace list of commands for confirmation dialogs.
pub fn command_list(cmds: &[String]) -> gtk::Widget {
    let v = vbox(4);
    for c in cmds {
        let l = wrapped(c, &["mono", "small"]);
        l.set_selectable(true);
        v.append(&l);
    }
    v.add_css_class("card");
    v.upcast()
}

pub fn fmt_bytes(b: u64) -> String {
    sift_core::format::bytes(b)
}
