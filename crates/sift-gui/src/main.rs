//! Sift — a Linux port of Burrow's system companion, in GTK4 + libadwaita.

mod app;
mod fonts;
mod pages;
mod theme;
mod widgets;
mod worker;

use adw::prelude::*;

fn main() -> gtk::glib::ExitCode {
    let paths = sift_core::paths::Paths::detect();
    fonts::register(&paths.sift_cache_dir());
    let mut flags = gtk::gio::ApplicationFlags::empty();
    if std::env::var_os("SIFT_SCREENSHOT_DIR").is_some() || paths.sandboxed {
        flags |= gtk::gio::ApplicationFlags::NON_UNIQUE;
    }
    let application = adw::Application::builder().application_id(app::APP_ID).flags(flags).build();
    application.connect_activate(move |a| {
        if let Some(w) = a.active_window() {
            w.present();
            return;
        }
        app::App::build(a, paths.clone());
    });
    application.run_with_args::<&str>(&[])
}
