//! Leftovers: per-app data folders in your home that belong to one app.
//! Matching is deliberately conservative (exact folder names only) and
//! everything found is shown for review before anything moves.

use super::{App, Source};
use crate::ctx::Ctx;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize)]
pub struct Leftover {
    pub path: PathBuf,
    pub bytes: u64,
    pub reason: String,
    pub selected: bool,
}

/// Names too generic to ever identify one app.
const STOP: &[&str] = &[
    "app", "apps", "application", "applications", "cache", "common", "config", "data", "default", "desktop", "share",
    "state", "local", "user", "users", "system", "settings", "gtk", "gtk-3.0", "gtk-4.0", "gnome", "kde", "dconf",
    "pulse", "fontconfig", "systemd", "autostart", "menus", "mime", "icons", "fonts", "themes", "flatpak", "snap",
    "trash", "keyrings", "nautilus", "recently-used", "sessions", "environment.d", "user-dirs", "sift", "cosmic",
    "electron", "chromium", "java", "python", "node", "npm", "go", "rust", "cargo", "code", "git", "ssh", "gnupg",
];

/// Lower-cased identifiers an app's folders could be named after.
pub fn candidate_names(app: &App) -> Vec<(String, bool)> {
    // (name, strong) — strong names are exact package / app ids.
    let mut v: Vec<(String, bool)> = Vec::new();
    let mut push = |s: &str, strong: bool| {
        let s = s.trim().to_lowercase();
        if s.len() >= 3 && !STOP.contains(&s.as_str()) && !v.iter().any(|(x, _)| *x == s) {
            v.push((s, strong));
        }
    };
    match app.source {
        Source::Flatpak => {
            push(&app.package, true);
            if let Some(last) = app.package.rsplit('.').next() {
                push(last, false);
            }
        }
        Source::Snap | Source::Apt => push(&app.package, true),
        Source::AppImage | Source::Local => {}
    }
    if let Some(f) = &app.desktop_file {
        if let Some(stem) = f.file_stem() {
            let stem = stem.to_string_lossy();
            push(&stem, false);
            if let Some(last) = stem.rsplit('.').next() {
                push(last, false);
            }
        }
    }
    if let Some(w) = &app.wm_class {
        push(w, false);
    }
    if let Some(prog) = app.exec.as_deref().and_then(crate::optimize::first_program) {
        if let Some(base) = Path::new(&prog).file_name() {
            let b = base.to_string_lossy();
            if !b.to_lowercase().ends_with(".appimage") {
                push(&b, false);
            }
        }
    }
    push(&app.name.replace(' ', ""), false);
    push(&app.name.replace(' ', "-"), false);
    v
}

pub fn find(ctx: &Ctx, app: &App, all: &[App]) -> Vec<Leftover> {
    let home = &ctx.paths.home;
    let guard = ctx.guard();
    let names = candidate_names(app);
    // A name another installed app could also claim is not evidence.
    let others: HashSet<String> = all
        .iter()
        .filter(|a| a.id != app.id)
        .flat_map(|a| candidate_names(a).into_iter().map(|(n, _)| n))
        .collect();
    let mut out: Vec<Leftover> = Vec::new();
    let mut add = |path: PathBuf, reason: String, selected: bool| {
        if out.iter().any(|l| l.path == path) || guard.check_lexical(&path, &[home.clone()]).is_err() {
            return;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else { return };
        if meta.file_type().is_symlink() {
            return;
        }
        out.push(Leftover { bytes: super::path_size(&path), path, reason, selected });
    };

    // Exact per-app sandboxes first.
    if app.source == Source::Flatpak {
        add(home.join(".var/app").join(&app.package), "Flatpak app data".into(), true);
    }
    if app.source == Source::Snap {
        add(home.join("snap").join(&app.package), "Snap app data".into(), true);
    }

    let bases: [(&Path, &str); 4] = [
        (&ctx.paths.config, "settings"),
        (&ctx.paths.data, "app data"),
        (&ctx.paths.cache, "cache"),
        (&ctx.paths.state, "state"),
    ];
    for (base, what) in bases {
        let Ok(rd) = fs::read_dir(base) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if let Some((_, strong)) = names.iter().find(|(c, _)| *c == n) {
                if others.contains(&n) {
                    continue;
                }
                let rel = crate::format::tilde(&e.path(), home);
                add(e.path(), format!("{what} folder matching “{}” ({rel})", e.file_name().to_string_lossy()), *strong);
            }
        }
    }
    // ~/.<name> dot-folders (e.g. ~/.zoom) — exact, never pre-selected.
    if let Ok(rd) = fs::read_dir(home) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if let Some(stripped) = n.strip_prefix('.') {
                if names.iter().any(|(c, _)| c == stripped) && !others.contains(stripped) {
                    add(e.path(), format!("hidden folder ~/{}", e.file_name().to_string_lossy()), false);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::runner::FakeRunner;
    use crate::settings::Settings;
    use std::sync::Arc;

    fn app(id: &str, source: Source, package: &str, name: &str) -> App {
        App {
            id: id.into(),
            name: name.into(),
            source,
            package: package.into(),
            installation: Some("user".into()),
            version: String::new(),
            size: 0,
            icon: None,
            desktop_file: None,
            path: None,
            exec: None,
            wm_class: None,
            removable: true,
            protected_reason: None,
        }
    }

    #[test]
    fn finds_exact_matches_only() {
        let d = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(d.path()).unwrap();
        for rel in [".var/app/md.obsidian.Obsidian/config", ".config/obsidian", ".cache/obsidian-old", ".config/Code", ".zoom", ".ssh"] {
            fs::create_dir_all(home.join(rel)).unwrap();
        }
        let ctx = Ctx::with(Paths::for_home(home.clone(), true), Settings::default(), Arc::new(FakeRunner::default()));
        let obs = app("flatpak:user:md.obsidian.Obsidian", Source::Flatpak, "md.obsidian.Obsidian", "Obsidian");
        let zoom = app("apt:zoom", Source::Apt, "zoom", "Zoom");
        let all = vec![obs.clone(), zoom.clone()];
        let found = find(&ctx, &obs, &all);
        let rels: Vec<String> = found.iter().map(|l| l.path.strip_prefix(&home).unwrap().display().to_string()).collect();
        assert!(rels.contains(&".var/app/md.obsidian.Obsidian".to_string()));
        assert!(rels.contains(&".config/obsidian".to_string()));
        assert!(!rels.iter().any(|r| r.contains("obsidian-old")));
        assert!(found.iter().find(|l| l.path.ends_with(".var/app/md.obsidian.Obsidian")).unwrap().selected);
        let z = find(&ctx, &zoom, &all);
        assert_eq!(z.len(), 1);
        assert!(!z[0].selected);
    }
}
