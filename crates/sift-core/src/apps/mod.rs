//! Apps: what's installed (APT, Flatpak, Snap, AppImage, local launchers),
//! and uninstalling one with its leftovers — previewed first, always.

pub mod desktop;
pub mod leftovers;

use crate::ctx::Ctx;
use crate::fsutil;
use crate::ops::OpLog;
use crate::optimize::first_program;
use crate::privileged::{self, valid_token, RootCommand};
use crate::removal::{Event, Remover};
use crate::runner;
use desktop::DesktopEntry;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub use leftovers::Leftover;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Apt,
    Flatpak,
    Snap,
    AppImage,
    Local,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::Apt => "APT",
            Source::Flatpak => "Flatpak",
            Source::Snap => "Snap",
            Source::AppImage => "AppImage",
            Source::Local => "Local",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct App {
    pub id: String,
    pub name: String,
    pub source: Source,
    /// Package name / Flatpak id / snap name / file name.
    pub package: String,
    /// Flatpak installation (`user` / `system`).
    pub installation: Option<String>,
    pub version: String,
    pub size: u64,
    pub icon: Option<String>,
    pub desktop_file: Option<PathBuf>,
    /// AppImage file or local launcher.
    pub path: Option<PathBuf>,
    pub exec: Option<String>,
    pub wm_class: Option<String>,
    pub removable: bool,
    pub protected_reason: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DpkgInfo {
    pub size: u64,
    pub version: String,
    pub priority: String,
    pub essential: bool,
}

/// Packages that hold the desktop together; never removed from here.
const PROTECTED_PACKAGES: &[&str] = &[
    "pop-desktop", "ubuntu-desktop", "ubuntu-desktop-minimal", "kubuntu-desktop", "xubuntu-desktop", "cosmic-session",
    "cosmic-comp", "cosmic-panel", "cosmic-settings", "cosmic-files", "cosmic-launcher", "cosmic-greeter",
    "cosmic-app-library", "cosmic-osd", "cosmic-bg", "cosmic-workspaces", "gnome-shell", "gnome-session",
    "gnome-control-center", "gnome-settings-daemon", "plasma-desktop", "plasma-workspace", "gdm3", "sddm", "lightdm",
    "systemd", "network-manager", "apt", "dpkg", "sudo", "polkitd", "pkexec", "xorg", "xwayland", "dbus",
    "pipewire", "wireplumber", "pulseaudio", "flatpak", "snapd", "nautilus", "dolphin", "linux-generic",
    "linux-system76", "grub-efi-amd64", "systemd-boot", "kernelstub",
];

fn is_protected_pkg(name: &str) -> bool {
    PROTECTED_PACKAGES.contains(&name) || name.starts_with("linux-image") || name.starts_with("linux-modules") || name.starts_with("libc6")
}

/// `${binary:Package}\t${Installed-Size}\t${Version}\t${Priority}\t${Essential}\t${db:Status-Abbrev}`
pub fn parse_dpkg_query(text: &str) -> HashMap<String, DpkgInfo> {
    let mut m = HashMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 6 || !f[5].starts_with("ii") {
            continue;
        }
        let name = f[0].split(':').next().unwrap_or(f[0]).to_string();
        m.insert(
            name,
            DpkgInfo {
                size: f[1].trim().parse::<u64>().unwrap_or(0) * 1024,
                version: f[2].to_string(),
                priority: f[3].to_string(),
                essential: f[4] == "yes",
            },
        );
    }
    m
}

/// `dpkg -S` output → path → first owning package.
pub fn parse_dpkg_search(text: &str) -> HashMap<PathBuf, String> {
    let mut m = HashMap::new();
    for line in text.lines() {
        if line.starts_with("diversion ") {
            continue;
        }
        let Some((pkgs, path)) = line.rsplit_once(": ") else { continue };
        let first = pkgs.split(',').next().unwrap_or("").trim();
        let name = first.split(':').next().unwrap_or(first).to_string();
        if !name.is_empty() {
            m.insert(PathBuf::from(path.trim()), name);
        }
    }
    m
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlatpakRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub size: u64,
    pub installation: String,
}

/// `flatpak list --app --columns=application,name,version,size,installation`
pub fn parse_flatpak_list(text: &str) -> Vec<FlatpakRow> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = if l.contains('\t') { l.split('\t').collect() } else { l.split("  ").filter(|s| !s.trim().is_empty()).collect() };
            if f.len() < 5 {
                return None;
            }
            Some(FlatpakRow {
                id: f[0].trim().into(),
                name: f[1].trim().into(),
                version: f[2].trim().into(),
                size: crate::format::parse_size(f[3]).unwrap_or(0),
                installation: f[4].trim().into(),
            })
        })
        .filter(|r| r.id.contains('.'))
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapRow {
    pub name: String,
    pub version: String,
    pub rev: String,
    pub notes: String,
}

pub fn parse_snap_list(text: &str) -> Vec<SnapRow> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 4 {
                return None;
            }
            Some(SnapRow { name: f[0].into(), version: f[1].into(), rev: f[2].into(), notes: f.last().unwrap_or(&"").to_string() })
        })
        .collect()
}

fn appimage_files(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in ["Applications", "AppImages", "Apps", ".local/bin", "Downloads", "Desktop", "opt", "bin"] {
        if let Ok(rd) = fs::read_dir(home.join(d)) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                if n.ends_with(".appimage") && e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    out.push(e.path());
                }
            }
        }
    }
    out.sort();
    out
}

pub fn inventory(ctx: &Ctx) -> Vec<App> {
    let r = ctx.runner();
    let mut apps: Vec<App> = Vec::new();
    let home = &ctx.paths.home;

    // Flatpak
    let flatpak_rows = if crate::paths::which("flatpak").is_some() {
        runner::run(r, "flatpak", &["list", "--app", "--columns=application,name,version,size,installation"], 30)
            .map(|o| parse_flatpak_list(&o.stdout))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let flatpak_dirs = [
        (PathBuf::from("/var/lib/flatpak/exports/share/applications"), "system"),
        (ctx.paths.data.join("flatpak/exports/share/applications"), "user"),
    ];
    let mut flatpak_desktops: HashMap<String, DesktopEntry> = HashMap::new();
    for (dir, _) in &flatpak_dirs {
        for e in desktop::read_dir(dir) {
            let id = e.flatpak_id.clone().unwrap_or_else(|| e.id.clone());
            flatpak_desktops.entry(id).or_insert(e);
        }
    }
    for row in &flatpak_rows {
        let de = flatpak_desktops.get(&row.id);
        apps.push(App {
            id: format!("flatpak:{}:{}", row.installation, row.id),
            name: de.map(|d| d.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| row.name.clone()),
            source: Source::Flatpak,
            package: row.id.clone(),
            installation: Some(row.installation.clone()),
            version: row.version.clone(),
            size: row.size,
            icon: de.and_then(|d| d.icon.clone()).or_else(|| Some(row.id.clone())),
            desktop_file: de.map(|d| d.file.clone()),
            path: None,
            exec: de.and_then(|d| d.exec.clone()),
            wm_class: de.and_then(|d| d.startup_wm_class.clone()),
            removable: true,
            protected_reason: None,
        });
    }

    // Snap
    if crate::paths::which("snap").is_some() {
        let rows = runner::run(r, "snap", &["list"], 20).map(|o| parse_snap_list(&o.stdout)).unwrap_or_default();
        let entries = desktop::read_dir(Path::new("/var/lib/snapd/desktop/applications"));
        let mut seen = HashSet::new();
        for e in entries {
            let snap = e.snap_name.clone().unwrap_or_else(|| e.id.split('_').next().unwrap_or(&e.id).to_string());
            if !seen.insert(snap.clone()) {
                continue;
            }
            let row = rows.iter().find(|r| r.name == snap);
            let size = row
                .and_then(|r| fs::metadata(format!("/var/lib/snapd/snaps/{}_{}.snap", r.name, r.rev)).ok())
                .map(|m| m.len())
                .unwrap_or(0);
            apps.push(App {
                id: format!("snap:{snap}"),
                name: e.name.clone(),
                source: Source::Snap,
                package: snap.clone(),
                installation: None,
                version: row.map(|r| r.version.clone()).unwrap_or_default(),
                size,
                icon: e.icon.clone(),
                desktop_file: Some(e.file.clone()),
                path: None,
                exec: e.exec.clone(),
                wm_class: e.startup_wm_class.clone(),
                removable: valid_token(&snap) && snap != "snapd" && snap != "snap-store",
                protected_reason: (snap == "snapd" || snap == "snap-store").then(|| "part of the Snap system".to_string()),
            });
        }
    }

    // APT / system launchers
    let system_entries: Vec<DesktopEntry> = desktop::read_dir(Path::new("/usr/share/applications"));
    let dpkg = if crate::paths::which("dpkg-query").is_some() {
        runner::run(
            r,
            "dpkg-query",
            &["-W", "-f=${binary:Package}\t${Installed-Size}\t${Version}\t${Priority}\t${Essential}\t${db:Status-Abbrev}\n"],
            30,
        )
        .map(|o| parse_dpkg_query(&o.stdout))
        .unwrap_or_default()
    } else {
        HashMap::new()
    };
    let owners = if !system_entries.is_empty() && crate::paths::which("dpkg").is_some() {
        let mut args: Vec<String> = vec!["-S".into()];
        args.extend(system_entries.iter().map(|e| e.file.display().to_string()));
        r.run("dpkg", &args, std::time::Duration::from_secs(30)).map(|o| parse_dpkg_search(&o.stdout)).unwrap_or_default()
    } else {
        HashMap::new()
    };
    let mut by_pkg: HashMap<String, Vec<DesktopEntry>> = HashMap::new();
    for e in system_entries {
        match owners.get(&e.file) {
            Some(pkg) => by_pkg.entry(pkg.clone()).or_default().push(e),
            None => apps.push(local_app(e, false)),
        }
    }
    for (pkg, entries) in by_pkg {
        // Prefer the launcher named like the package as the face of the app.
        let main = entries
            .iter()
            .find(|e| e.id == pkg || e.id.ends_with(&format!(".{pkg}")))
            .unwrap_or(&entries[0])
            .clone();
        let info = dpkg.get(&pkg).cloned().unwrap_or_default();
        let protected = if is_protected_pkg(&pkg) {
            Some("part of your desktop or system".to_string())
        } else if info.essential || info.priority == "required" || info.priority == "important" {
            Some(format!("marked {} by your distribution", if info.essential { "essential".to_string() } else { info.priority.clone() }))
        } else {
            None
        };
        apps.push(App {
            id: format!("apt:{pkg}"),
            name: main.name.clone(),
            source: Source::Apt,
            package: pkg.clone(),
            installation: None,
            version: info.version.clone(),
            size: info.size,
            icon: main.icon.clone(),
            desktop_file: Some(main.file.clone()),
            path: None,
            exec: main.exec.clone(),
            wm_class: main.startup_wm_class.clone(),
            removable: protected.is_none() && valid_token(&pkg),
            protected_reason: protected,
        });
    }
    for e in desktop::read_dir(Path::new("/usr/local/share/applications")) {
        apps.push(local_app(e, false));
    }

    // User launchers + AppImages
    let user_dir = ctx.paths.data.join("applications");
    let system_ids: HashSet<String> = apps.iter().filter_map(|a| a.desktop_file.as_ref()).filter_map(|f| f.file_stem().map(|s| s.to_string_lossy().into_owned())).collect();
    let mut referenced: HashSet<PathBuf> = HashSet::new();
    for e in desktop::read_dir(&user_dir) {
        if system_ids.contains(&e.id) {
            continue; // a personal override of an installed app's launcher
        }
        let prog = e.exec.as_deref().and_then(first_program);
        if let Some(p) = prog.filter(|p| p.to_ascii_lowercase().ends_with(".appimage")) {
            let path = PathBuf::from(&p);
            referenced.insert(path.clone());
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            apps.push(App {
                id: format!("appimage:{}", path.display()),
                name: e.name.clone(),
                source: Source::AppImage,
                package: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                installation: None,
                version: String::new(),
                size,
                icon: e.icon.clone(),
                desktop_file: Some(e.file.clone()),
                removable: path.starts_with(home) && path.exists(),
                protected_reason: (!path.starts_with(home)).then(|| "AppImage lives outside your home folder".to_string()),
                path: Some(path),
                exec: e.exec.clone(),
                wm_class: e.startup_wm_class.clone(),
            });
        } else {
            apps.push(local_app(e, true));
        }
    }
    for f in appimage_files(home) {
        if referenced.contains(&f) {
            continue;
        }
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let pretty = name.split(|c: char| c == '-' || c == '_' || c == '.').next().unwrap_or(&name).to_string();
        apps.push(App {
            id: format!("appimage:{}", f.display()),
            name: pretty,
            source: Source::AppImage,
            package: name,
            installation: None,
            version: String::new(),
            size: fs::metadata(&f).map(|m| m.len()).unwrap_or(0),
            icon: None,
            desktop_file: None,
            path: Some(f.clone()),
            exec: None,
            wm_class: None,
            removable: true,
            protected_reason: None,
        });
    }

    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

fn local_app(e: DesktopEntry, user_owned: bool) -> App {
    App {
        id: format!("local:{}", e.file.display()),
        name: e.name.clone(),
        source: Source::Local,
        package: e.id.clone(),
        installation: None,
        version: String::new(),
        size: 0,
        icon: e.icon.clone(),
        desktop_file: Some(e.file.clone()),
        path: Some(e.file.clone()),
        exec: e.exec.clone(),
        wm_class: e.startup_wm_class.clone(),
        removable: user_owned,
        protected_reason: (!user_owned).then(|| "installed by hand system-wide — remove it the way it was installed".to_string()),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UninstallPlan {
    pub app: App,
    /// Exact commands that will run.
    pub commands: Vec<String>,
    pub needs_root: bool,
    /// Other packages APT would remove alongside.
    pub also_removes: Vec<String>,
    pub blocked: Option<String>,
    /// Files moved to the Trash as part of removing the app itself.
    pub trash_files: Vec<PathBuf>,
    pub leftovers: Vec<Leftover>,
    #[serde(skip)]
    root: Option<RootCommand>,
    #[serde(skip)]
    user_argv: Option<Vec<String>>,
}

/// `Remv pkg [ver]` lines from `apt-get -s remove`.
pub fn parse_apt_simulation(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("Remv "))
        .filter_map(|r| r.split_whitespace().next())
        .map(|n| n.split(':').next().unwrap_or(n).to_string())
        .collect()
}

pub fn plan_uninstall(ctx: &Ctx, app: &App, all: &[App]) -> UninstallPlan {
    let mut plan = UninstallPlan {
        app: app.clone(),
        commands: Vec::new(),
        needs_root: false,
        also_removes: Vec::new(),
        blocked: app.protected_reason.clone().filter(|_| !app.removable),
        trash_files: Vec::new(),
        leftovers: leftovers::find(ctx, app, all),
        root: None,
        user_argv: None,
    };
    if !app.removable && plan.blocked.is_none() {
        plan.blocked = Some("this app can't be removed from Sift".into());
    }
    match app.source {
        Source::Apt => {
            if !valid_token(&app.package) {
                plan.blocked = Some("unexpected package name".into());
            } else {
                let sim = runner::run(ctx.runner(), "apt-get", &["-s", "remove", &app.package], 60).map(|o| o.stdout).unwrap_or_default();
                let removed = parse_apt_simulation(&sim);
                let dpkg = runner::run(
                    ctx.runner(),
                    "dpkg-query",
                    &["-W", "-f=${binary:Package}\t${Installed-Size}\t${Version}\t${Priority}\t${Essential}\t${db:Status-Abbrev}\n"],
                    30,
                )
                .map(|o| parse_dpkg_query(&o.stdout))
                .unwrap_or_default();
                if plan.blocked.is_some() {
                    // Already refused for its own reason.
                } else if let Some(bad) = removed.iter().find(|p| {
                    is_protected_pkg(p)
                        || dpkg.get(*p).map(|i| i.essential || i.priority == "required" || i.priority == "important").unwrap_or(false)
                }) {
                    plan.blocked = Some(format!("removing it would also remove {bad}, which your system needs"));
                }
                plan.also_removes = removed.into_iter().filter(|p| *p != app.package).collect();
                let cmd = format!("apt-get remove -y {}", app.package);
                plan.commands.push(cmd.clone());
                plan.needs_root = true;
                plan.root = Some(RootCommand::new("apt-remove", &format!("Remove {}", app.name), &cmd, &cmd));
            }
        }
        Source::Flatpak => {
            let inst = if app.installation.as_deref() == Some("system") { "--system" } else { "--user" };
            let argv: Vec<String> = ["flatpak", "uninstall", inst, "-y", "--noninteractive", &app.package].iter().map(|s| s.to_string()).collect();
            plan.commands.push(argv.join(" "));
            plan.user_argv = Some(argv);
        }
        Source::Snap => {
            if valid_token(&app.package) {
                let cmd = format!("snap remove {}", app.package);
                plan.commands.push(cmd.clone());
                plan.needs_root = true;
                plan.root = Some(RootCommand::new("snap-remove", &format!("Remove {}", app.name), &cmd, &cmd));
            } else {
                plan.blocked = Some("unexpected snap name".into());
            }
        }
        Source::AppImage => {
            if let Some(p) = &app.path {
                plan.trash_files.push(p.clone());
                plan.trash_files.extend(launchers_running(&ctx.paths.data.join("applications"), p));
                plan.commands.push(format!("move {} to Trash", crate::format::tilde(p, &ctx.paths.home)));
            }
        }
        Source::Local => {
            if let Some(f) = &app.desktop_file {
                plan.trash_files.push(f.clone());
                plan.commands.push(format!("move launcher {} to Trash", crate::format::tilde(f, &ctx.paths.home)));
            }
        }
    }
    plan.trash_files.sort();
    plan.trash_files.dedup();
    plan
}

/// User launchers whose Exec runs `target`.
fn launchers_running(dir: &Path, target: &Path) -> Vec<PathBuf> {
    desktop::read_dir(dir)
        .into_iter()
        .filter(|e| e.exec.as_deref().and_then(first_program).map(|p| Path::new(&p) == target).unwrap_or(false))
        .map(|e| e.file)
        .collect()
}

#[derive(Clone, Debug, Serialize)]
pub struct UninstallReport {
    pub ok: bool,
    pub message: String,
    pub steps: Vec<(String, bool, String)>,
    pub trashed_bytes: u64,
}

/// Run a previewed plan. Leftovers are only touched once the app itself is
/// gone, so a failed or cancelled removal never strips a working app's data.
pub fn uninstall(ctx: &Ctx, plan: &UninstallPlan, leftovers: &[PathBuf], allow_root: bool, sink: &mut dyn FnMut(&Event)) -> UninstallReport {
    let mut steps = Vec::new();
    if let Some(b) = &plan.blocked {
        return UninstallReport { ok: false, message: b.clone(), steps, trashed_bytes: 0 };
    }
    let mut log = OpLog::begin(&ctx.paths, "uninstall", false, &ctx.source);
    let mut ok = true;

    if let Some(argv) = &plan.user_argv {
        let res = ctx.runner().run(&argv[0], &argv[1..], std::time::Duration::from_secs(900));
        let (good, out) = match res {
            Ok(o) => (o.success(), o.brief_error()),
            Err(e) => (false, e.to_string()),
        };
        log.record(&argv.join(" "), plan.app.size, "command", if good { "done" } else { "failed" }, &out);
        steps.push((argv.join(" "), good, out));
        ok &= good;
    }
    if let Some(root) = &plan.root {
        if !allow_root {
            steps.push((root.display.clone(), false, "admin action not confirmed".into()));
            ok = false;
        } else {
            match privileged::run_batch(ctx.runner(), std::slice::from_ref(root)) {
                Ok(rs) => {
                    for r in rs {
                        let tail = r.output.lines().last().unwrap_or("").to_string();
                        log.record(&format!("[admin] {}", root.display), plan.app.size, "command", if r.ok { "done" } else { "failed" }, &tail);
                        ok &= r.ok;
                        steps.push((root.display.clone(), r.ok, tail));
                    }
                }
                Err(e) => {
                    ok = false;
                    steps.push((root.display.clone(), false, e.message));
                }
            }
        }
    }

    let mut trashed = 0;
    if ok {
        let guard = ctx.guard();
        let trash = ctx.trash();
        let mut rm = Remover::new(&guard, &trash, false, false, &mut log);
        let home = [ctx.paths.home.clone()];
        for f in &plan.trash_files {
            let good = rm.remove(f, &home, false, None, sink);
            ok &= good;
            steps.push((format!("move {} to Trash", f.display()), good, String::new()));
        }
        let allowed: HashSet<&PathBuf> = plan.leftovers.iter().map(|l| &l.path).collect();
        for l in leftovers {
            if !allowed.contains(l) {
                rm.refuse(l, "not one of the leftovers Sift found for this app", sink);
                continue;
            }
            rm.remove(l, &home, false, None, sink);
        }
        trashed = rm.summary.trashed_bytes;
    }
    let message = if ok {
        format!("{} was removed", plan.app.name)
    } else {
        steps.iter().find(|s| !s.1).map(|s| format!("Stopped: {}", if s.2.is_empty() { &s.0 } else { &s.2 })).unwrap_or_else(|| "Uninstall failed".into())
    };
    log.finish(if ok { "done" } else { "failed" }, &message);
    UninstallReport { ok, message, steps, trashed_bytes: trashed }
}

/// Size of leftover candidates for display.
pub fn path_size(p: &Path) -> u64 {
    fsutil::du(p, None).bytes
}

/// Owner uid of a path (0 when unknown).
pub fn owner(p: &Path) -> u32 {
    fs::symlink_metadata(p).map(|m| m.uid()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpkg_parsers() {
        let q = "firefox\t250000\t130.0\toptional\tno\tii \nlibc6:amd64\t13000\t2.39\trequired\tno\tii \nbash\t1800\t5.2\trequired\tyes\tii \ngone\t1\t1\toptional\tno\trc \n";
        let m = parse_dpkg_query(q);
        assert_eq!(m["firefox"].size, 250000 * 1024);
        assert!(m["bash"].essential);
        assert_eq!(m["libc6"].priority, "required");
        assert!(!m.contains_key("gone"));
        let s = parse_dpkg_search("code: /usr/share/applications/code.desktop\nlibreoffice-writer, libreoffice-core: /usr/share/applications/w.desktop\nfoo:amd64: /usr/share/applications/foo.desktop\n");
        assert_eq!(s[Path::new("/usr/share/applications/code.desktop")], "code");
        assert_eq!(s[Path::new("/usr/share/applications/w.desktop")], "libreoffice-writer");
        assert_eq!(s[Path::new("/usr/share/applications/foo.desktop")], "foo");
    }

    #[test]
    fn flatpak_and_snap_lists() {
        let f = parse_flatpak_list("md.obsidian.Obsidian\tObsidian\t1.7.7\t412.0 MB\tuser\ncom.spotify.Client\tSpotify\t1.2\t1.1 GB\tsystem\n");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].size, 412_000_000);
        assert_eq!(f[1].installation, "system");
        let s = parse_snap_list("Name  Version  Rev  Tracking  Publisher  Notes\nfirefox  130.0  4848  latest/stable  mozilla**  -\ncore22  2024  1564  latest/stable  canonical**  base\n");
        assert_eq!(s[0].name, "firefox");
        assert_eq!(s[1].notes, "base");
        assert_eq!(parse_apt_simulation("Remv code [1.95]\nRemv libfoo:amd64 [1]\nInst x\n"), vec!["code", "libfoo"]);
    }

    #[test]
    fn protected_names() {
        assert!(is_protected_pkg("pop-desktop"));
        assert!(is_protected_pkg("linux-image-6.9.3-76060903-generic"));
        assert!(!is_protected_pkg("code"));
    }
}
