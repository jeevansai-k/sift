//! Settings: appearance, monitoring/history, cleaning rules, data, about.

use crate::app::AppRef;
use crate::widgets::{self, hbox, icons, label, vbox};
use crate::worker;
use adw::prelude::*;
use sift_core::db::Db;
use sift_core::format::{bytes, tilde};
use sift_core::runner::SystemRunner;
use sift_core::service;
use sift_core::settings::Theme;
use std::cell::RefCell;
use std::rc::Rc;

const INTERVALS: &[(&str, u32)] = &[("5 seconds", 5), ("15 seconds", 15), ("30 seconds", 30), ("1 minute", 60), ("2 minutes", 120), ("5 minutes", 300)];
const RETENTION: &[(&str, u32)] = &[("1 day", 1), ("7 days", 7), ("30 days", 30), ("90 days", 90), ("1 year", 365)];

fn combo(title: &str, subtitle: &str, items: &[&str], selected: usize) -> adw::ComboRow {
    let row = adw::ComboRow::new();
    row.set_title(title);
    if !subtitle.is_empty() {
        row.set_subtitle(subtitle);
    }
    row.set_model(Some(&gtk::StringList::new(items)));
    row.set_selected(selected as u32);
    row
}

fn spin(title: &str, subtitle: &str, min: f64, max: f64, value: f64) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(min, max, 1.0);
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_value(value);
    row
}

pub fn build(app: &AppRef) -> gtk::Widget {
    let root = vbox(14);
    root.add_css_class("page");
    root.add_css_class("accent-settings");
    root.append(&widgets::page_header("Settings", "Everything stays on this computer."));
    let content = vbox(24);
    content.set_margin_bottom(30);
    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(820);
    clamp.set_child(Some(&content));
    root.append(&widgets::scrolled(&clamp));
    let s = app.settings.borrow().clone();

    // Appearance
    let g = adw::PreferencesGroup::new();
    g.set_title("Appearance");
    let theme_row = combo("Theme", "", &["Follow system", "Dark", "Light"], match s.theme {
        Theme::System => 0,
        Theme::Dark => 1,
        Theme::Light => 2,
    });
    let a = Rc::downgrade(app);
    theme_row.connect_selected_notify(move |r| {
        if let Some(app) = a.upgrade() {
            let t = match r.selected() {
                1 => Theme::Dark,
                2 => Theme::Light,
                _ => Theme::System,
            };
            app.update_settings(|s| s.theme = t);
        }
    });
    g.add(&theme_row);
    content.append(&g);

    // Monitoring
    let g = adw::PreferencesGroup::new();
    g.set_title("Monitoring and history");
    g.set_description(Some("Sift keeps a local history of CPU, memory, disk, network and temperatures in an SQLite file."));
    let refresh = spin("Dashboard refresh", "Seconds between live updates while the window is open", 1.0, 10.0, s.live_refresh_secs as f64);
    let a = Rc::downgrade(app);
    refresh.connect_value_notify(move |r| {
        if let Some(app) = a.upgrade() {
            let v = r.value() as u32;
            app.update_settings(|s| s.live_refresh_secs = v);
        }
    });
    g.add(&refresh);
    let int_idx = INTERVALS.iter().position(|i| i.1 == s.sample_interval_secs).unwrap_or(2);
    let interval = combo("History sample interval", "How often a row is written", &INTERVALS.iter().map(|i| i.0).collect::<Vec<_>>(), int_idx);
    let a = Rc::downgrade(app);
    interval.connect_selected_notify(move |r| {
        if let Some(app) = a.upgrade() {
            let v = INTERVALS[r.selected() as usize % INTERVALS.len()].1;
            app.update_settings(|s| s.sample_interval_secs = v);
        }
    });
    g.add(&interval);
    let ret_idx = RETENTION.iter().position(|i| i.1 == s.retention_days).unwrap_or(2);
    let retention = combo("Keep history for", "Older rows are pruned hourly", &RETENTION.iter().map(|i| i.0).collect::<Vec<_>>(), ret_idx);
    let a = Rc::downgrade(app);
    retention.connect_selected_notify(move |r| {
        if let Some(app) = a.upgrade() {
            let v = RETENTION[r.selected() as usize % RETENTION.len()].1;
            app.update_settings(|s| s.retention_days = v);
        }
    });
    g.add(&retention);
    let bg = adw::SwitchRow::new();
    bg.set_title("Record history in the background");
    bg.set_subtitle("Runs `sift sampler` as a systemd user service, so history continues while Sift is closed");
    g.add(&bg);
    content.append(&g);
    // Reflect the real service state, then react to toggles.
    let a = Rc::downgrade(app);
    let paths = app.paths.clone();
    let bg2 = bg.clone();
    worker::run(move |send| send(service::state(&paths, &SystemRunner)), move |st: service::State| {
        bg2.set_active(st.enabled || st.active);
        let a = a.clone();
        bg2.connect_active_notify(move |row| {
            let Some(app) = a.upgrade() else { return };
            let on = row.is_active();
            let paths = app.paths.clone();
            let a2 = Rc::downgrade(&app);
            let row2 = row.clone();
            worker::run(
                move |send| send(if on { service::enable(&paths, &SystemRunner) } else { service::disable(&paths, &SystemRunner) }.map_err(|e| e.message)),
                move |r: Result<(), String>| {
                    if let Some(app) = a2.upgrade() {
                        match r {
                            Ok(()) => app.toast(if on { "Background recording is on" } else { "Background recording is off" }),
                            Err(e) => {
                                app.toast(&format!("Couldn't change the service: {e}"));
                                row2.set_sensitive(true);
                            }
                        }
                    }
                },
            );
        });
    });

    // Cleaning
    let g = adw::PreferencesGroup::new();
    g.set_title("Cleaning");
    let recent = spin("Protect recent projects", "Purge won't pre-select build folders of projects changed within this many days", 0.0, 365.0, s.purge_recent_days as f64);
    let a = Rc::downgrade(app);
    recent.connect_value_notify(move |r| {
        if let Some(app) = a.upgrade() {
            let v = r.value() as u32;
            app.update_settings(|s| s.purge_recent_days = v);
        }
    });
    g.add(&recent);
    let age = spin("Protect fresh downloads", "Installers younger than this many days aren't pre-selected", 0.0, 365.0, s.installer_min_age_days as f64);
    let a = Rc::downgrade(app);
    age.connect_value_notify(move |r| {
        if let Some(app) = a.upgrade() {
            let v = r.value() as u32;
            app.update_settings(|s| s.installer_min_age_days = v);
        }
    });
    g.add(&age);
    let journal = spin("Keep system logs", "Days of journal history kept when cleaning the system journal", 1.0, 365.0, s.journal_keep_days as f64);
    let a = Rc::downgrade(app);
    journal.connect_value_notify(move |r| {
        if let Some(app) = a.upgrade() {
            let v = r.value() as u32;
            app.update_settings(|s| s.journal_keep_days = v);
        }
    });
    g.add(&journal);
    content.append(&g);

    content.append(&path_list(app, "Never clean", "Anything at or inside these folders is always skipped.", ListKind::Whitelist));
    content.append(&path_list(app, "Project folders", "Where Purge looks for build artifacts. Empty means your whole home folder.", ListKind::PurgeRoots));
    content.append(&path_list(app, "Installer folders", "Where Installers looks for leftover packages and images.", ListKind::InstallerDirs));

    // Data
    let g = adw::PreferencesGroup::new();
    g.set_title("Data");
    let db_row = adw::ActionRow::new();
    db_row.set_title("History database");
    db_row.set_subtitle(&tilde(&app.paths.db_file(), &app.paths.home));
    let clear = widgets::pill("Clear history…", &["danger", "small-btn"]);
    clear.set_valign(gtk::Align::Center);
    db_row.add_suffix(&clear);
    g.add(&db_row);
    let size = std::fs::metadata(app.paths.db_file()).map(|m| m.len()).unwrap_or(0);
    let info = adw::ActionRow::new();
    info.set_title("Size on disk");
    info.set_subtitle(&bytes(size));
    g.add(&info);
    content.append(&g);
    let a = Rc::downgrade(app);
    clear.connect_clicked(move |_| {
        let Some(app) = a.upgrade() else { return };
        let path = app.paths.db_file();
        let a2 = Rc::downgrade(&app);
        widgets::confirm(&app.window, "Clear all history?", "Deletes every recorded sample. Activity (what Sift cleaned) is kept.", "Clear history", true, None, move || {
            let r = Db::open(&path).and_then(|mut d| d.clear_history());
            if let Some(app) = a2.upgrade() {
                app.toast(&match r {
                    Ok(()) => "History cleared".to_string(),
                    Err(e) => format!("Couldn't clear history: {}", e.message),
                });
            }
        });
    });

    // About
    let g = adw::PreferencesGroup::new();
    g.set_title("About");
    let about = vbox(8);
    about.add_css_class("card");
    let top = hbox(12);
    top.append(&crate::app::logo_mark(40));
    let tv = vbox(2);
    tv.append(&label("Sift", &["section-title"]));
    tv.append(&label(&format!("Version {} · engine {}", sift_core::VERSION, sift_core::ENGINE), &["mono", "small", "dim"]));
    top.append(&tv);
    about.append(&top);
    about.append(&widgets::wrapped(
        "A Linux system companion: clean, purge, uninstall, analyze, optimize and monitor. Its design follows Burrow by caezium (MIT), whose command model comes from tw93's Mole (MIT). Fonts: Geist and Geist Mono (Vercel) and Cal Sans, under the SIL Open Font License. Sift sends nothing anywhere.",
        &["small", "dim"],
    ));
    g.add(&about);
    content.append(&g);
    root.upcast()
}

#[derive(Clone, Copy)]
enum ListKind {
    Whitelist,
    PurgeRoots,
    InstallerDirs,
}

fn path_list(app: &AppRef, title: &str, description: &str, kind: ListKind) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::new();
    g.set_title(title);
    g.set_description(Some(description));
    let add = widgets::pill("Add folder…", &["ghost", "small-btn"]);
    add.set_valign(gtk::Align::Center);
    g.set_header_suffix(Some(&add));
    let rows = gtk::ListBox::new();
    rows.add_css_class("boxed-list");
    rows.set_selection_mode(gtk::SelectionMode::None);
    g.add(&rows);

    // `slot` lets the render closure call itself after a removal.
    let slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let render: Rc<dyn Fn()> = {
        let a = Rc::downgrade(app);
        let rows = rows.clone();
        let slot = slot.clone();
        Rc::new(move || {
            let Some(app) = a.upgrade() else { return };
            widgets::clear_list(&rows);
            let list = {
                let mut s = app.settings.borrow().clone();
                list_mut(&mut s, kind).clone()
            };
            if list.is_empty() {
                let r = adw::ActionRow::new();
                r.set_title(match kind {
                    ListKind::PurgeRoots => "Your home folder",
                    _ => "Nothing added",
                });
                r.add_css_class("dim");
                rows.append(&r);
            }
            for item in list {
                let r = adw::ActionRow::new();
                r.set_title(&item);
                let rm = gtk::Button::new();
                rm.add_css_class("flat");
                rm.set_valign(gtk::Align::Center);
                rm.set_child(Some(&icons::icon("x", 14)));
                rm.set_tooltip_text(Some("Remove"));
                let a = Rc::downgrade(&app);
                let again = slot.clone();
                let it = item.clone();
                rm.connect_clicked(move |_| {
                    if let Some(app) = a.upgrade() {
                        app.update_settings(|s| list_mut(s, kind).retain(|x| *x != it));
                    }
                    let f = again.borrow().clone();
                    if let Some(f) = f {
                        f();
                    }
                });
                r.add_suffix(&rm);
                rows.append(&r);
            }
        })
    };
    *slot.borrow_mut() = Some(render.clone());
    render();

    let a = Rc::downgrade(app);
    add.connect_clicked(move |_| {
        let Some(app) = a.upgrade() else { return };
        let dialog = gtk::FileDialog::builder().title("Choose a folder").modal(true).build();
        let a2 = Rc::downgrade(&app);
        let r3 = render.clone();
        dialog.select_folder(Some(&app.window), gtk::gio::Cancellable::NONE, move |res| {
            let (Ok(f), Some(app)) = (res, a2.upgrade()) else { return };
            let Some(p) = f.path() else { return };
            let s = tilde(&p, &app.paths.home);
            app.update_settings(|set| {
                let list = list_mut(set, kind);
                if !list.contains(&s) {
                    list.push(s.clone());
                }
            });
            r3();
        });
    });
    g
}

fn list_mut(s: &mut sift_core::settings::Settings, kind: ListKind) -> &mut Vec<String> {
    match kind {
        ListKind::Whitelist => &mut s.whitelist,
        ListKind::PurgeRoots => &mut s.purge_roots,
        ListKind::InstallerDirs => &mut s.installer_dirs,
    }
}
