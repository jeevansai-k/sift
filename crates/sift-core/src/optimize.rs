//! Optimize: safe maintenance. User-level tasks run directly; admin tasks
//! go through one pkexec batch. Every task says exactly what it runs.

use crate::ctx::Ctx;
use crate::ops::OpLog;
use crate::paths::which;
use crate::privileged::{self, valid_token, RootCommand};
use crate::removal::Event;
use crate::runner;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize)]
pub struct OptTask {
    pub id: String,
    pub title: String,
    pub description: String,
    pub needs_root: bool,
    pub default_on: bool,
    /// Exact commands, shown before running.
    pub commands: Vec<String>,
    /// Extra preview lines (packages to remove, launchers to trash…).
    pub preview: Vec<String>,
    #[serde(skip)]
    argv: Vec<Vec<String>>,
    #[serde(skip)]
    root: Option<RootCommand>,
    #[serde(skip)]
    trash_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskResult {
    pub id: String,
    pub ok: bool,
    pub output: String,
}

fn user_task(id: &str, title: &str, description: &str, argv: Vec<Vec<String>>) -> OptTask {
    OptTask {
        id: id.into(),
        title: title.into(),
        description: description.into(),
        needs_root: false,
        default_on: true,
        commands: argv.iter().map(|a| a.join(" ")).collect(),
        preview: Vec::new(),
        argv,
        root: None,
        trash_paths: Vec::new(),
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// Packages `apt-get -s autoremove` would remove ("Remv name [version]").
pub fn parse_apt_removals(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("Remv "))
        .filter_map(|r| r.split_whitespace().next())
        .filter(|n| valid_token(n))
        .map(|n| n.to_string())
        .collect()
}

/// Desktop launchers in `dir` whose program no longer exists.
pub fn broken_launchers(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().map(|x| x != "desktop").unwrap_or(true) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&p) else { continue };
        let mut exec = None;
        let mut try_exec = None;
        let mut in_entry = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            if let Some(v) = line.strip_prefix("TryExec=") {
                try_exec = Some(v.to_string());
            } else if let Some(v) = line.strip_prefix("Exec=") {
                exec = Some(v.to_string());
            }
        }
        let program = try_exec.or_else(|| exec.and_then(|e| first_program(&e)));
        let Some(program) = program else { continue };
        let missing = if program.starts_with('/') { !Path::new(&program).exists() } else { which(&program).is_none() };
        if missing {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// First real program in an Exec line (skips `env VAR=x`, strips quotes).
pub fn first_program(exec: &str) -> Option<String> {
    let mut tokens = shell_words(exec).into_iter();
    loop {
        let t = tokens.next()?;
        if t == "env" || t.contains('=') && !t.starts_with('/') {
            continue;
        }
        return Some(t);
    }
}

fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn plan(ctx: &Ctx) -> Vec<OptTask> {
    let mut tasks = Vec::new();
    let data = &ctx.paths.data;

    if which("fc-cache").is_some() {
        tasks.push(user_task("font-cache", "Rebuild font cache", "Fixes missing or stale fonts in apps", vec![s(&["fc-cache", "-f"])]));
    }
    let apps = data.join("applications");
    if which("update-desktop-database").is_some() && apps.is_dir() {
        tasks.push(user_task(
            "desktop-db",
            "Refresh app launchers",
            "Re-indexes your installed .desktop launchers and file associations",
            vec![vec!["update-desktop-database".into(), apps.display().to_string()]],
        ));
    }
    let mime = data.join("mime");
    if which("update-mime-database").is_some() && mime.join("packages").is_dir() {
        tasks.push(user_task(
            "mime-db",
            "Refresh file types",
            "Rebuilds your personal MIME type database",
            vec![vec!["update-mime-database".into(), mime.display().to_string()]],
        ));
    }
    if which("gtk-update-icon-cache").is_some() {
        let icons = data.join("icons");
        let mut argv = Vec::new();
        if let Ok(rd) = fs::read_dir(&icons) {
            for e in rd.flatten() {
                if e.path().join("index.theme").exists() {
                    argv.push(vec!["gtk-update-icon-cache".into(), "-f".into(), "-t".into(), e.path().display().to_string()]);
                }
            }
        }
        if !argv.is_empty() {
            tasks.push(user_task("icon-cache", "Rebuild icon caches", "Refreshes icon themes you installed yourself", argv));
        }
    }
    if which("resolvectl").is_some() && Path::new("/run/systemd/resolve").exists() {
        tasks.push(user_task("dns", "Flush DNS cache", "Forgets cached DNS answers (helps after network changes)", vec![s(&["resolvectl", "flush-caches"])]));
    }
    if which("systemctl").is_some() {
        tasks.push(user_task(
            "user-services",
            "Reset failed user services",
            "Clears the failed state of your login services so they can start again",
            vec![s(&["systemctl", "--user", "reset-failed"]), s(&["systemctl", "--user", "daemon-reload"])],
        ));
    }
    let broken = broken_launchers(&apps);
    if !broken.is_empty() {
        let mut t = user_task(
            "broken-launchers",
            "Remove broken launchers",
            "Moves launchers whose program no longer exists to the Trash",
            Vec::new(),
        );
        t.commands = vec!["move to Trash:".into()];
        t.preview = broken.iter().map(|p| crate::format::tilde(p, &ctx.paths.home)).collect();
        t.trash_paths = broken;
        tasks.push(t);
    }
    if which("flatpak").is_some() {
        let mut t = user_task(
            "flatpak-unused",
            "Remove unused Flatpak runtimes",
            "Uninstalls runtimes no installed Flatpak app needs (your user installation)",
            vec![s(&["flatpak", "uninstall", "--user", "--unused", "-y", "--noninteractive"])],
        );
        t.default_on = false;
        tasks.push(t);
    }

    // Admin tasks (off by default — each confirmed batch asks for your password once).
    if let Some(fstrim) = ["/usr/sbin/fstrim", "/sbin/fstrim"].iter().find(|p| Path::new(p).exists()) {
        let cmd = format!("{fstrim} -av");
        tasks.push(root_task("trim", "TRIM SSDs", "Tells your SSDs which blocks are free (keeps write speed up)", &cmd, Vec::new()));
    }
    if which("updatedb").is_some() {
        tasks.push(root_task("updatedb", "Rebuild file search index", "Refreshes the `locate` database", "updatedb", Vec::new()));
    }
    if which("apt-get").is_some() {
        if let Ok(out) = runner::run(ctx.runner(), "apt-get", &["-s", "autoremove"], 60) {
            let pkgs = parse_apt_removals(&out.stdout);
            if !pkgs.is_empty() {
                let cmd = format!("apt-get remove -y {}", pkgs.join(" "));
                tasks.push(root_task(
                    "apt-autoremove",
                    "Remove unneeded packages",
                    "Packages installed as dependencies that nothing needs anymore",
                    &cmd,
                    pkgs,
                ));
            }
        }
    }
    tasks
}

fn root_task(id: &str, title: &str, description: &str, cmd: &str, preview: Vec<String>) -> OptTask {
    OptTask {
        id: id.into(),
        title: title.into(),
        description: description.into(),
        needs_root: true,
        default_on: false,
        commands: vec![cmd.into()],
        preview,
        argv: Vec::new(),
        root: Some(RootCommand::new(id, title, cmd, cmd)),
        trash_paths: Vec::new(),
    }
}

/// Run the selected tasks (`ids`) from a fresh plan. Returns per-task results.
pub fn run(ctx: &Ctx, ids: &[String], dry_run: bool, allow_root: bool, sink: &mut dyn FnMut(&Event, Option<&TaskResult>)) -> Vec<TaskResult> {
    let tasks: Vec<OptTask> = plan(ctx).into_iter().filter(|t| ids.contains(&t.id)).collect();
    let mut log = if dry_run { OpLog::disabled() } else { OpLog::begin(&ctx.paths, "optimize", false, &ctx.source) };
    let mut results = Vec::new();
    let mut roots = Vec::new();
    let total = tasks.len();
    for (i, t) in tasks.iter().enumerate() {
        sink(&Event::Progress { done: i, total, current: t.title.clone() }, None);
        if t.needs_root {
            if let Some(r) = &t.root {
                roots.push(r.clone());
            }
            continue;
        }
        let res = if dry_run {
            TaskResult { id: t.id.clone(), ok: true, output: format!("would run: {}", t.commands.join("; ")) }
        } else if !t.trash_paths.is_empty() {
            let trash = ctx.trash();
            let guard = ctx.guard();
            let scope = [ctx.paths.data.join("applications")];
            let mut errs = Vec::new();
            for p in &t.trash_paths {
                match guard.check(p, &scope) {
                    Ok(()) => {
                        if let Err(e) = trash.trash(p) {
                            errs.push(format!("{}: {}", p.display(), e.message));
                        } else {
                            log.record(&p.display().to_string(), 0, "trash", "trashed", "broken launcher");
                        }
                    }
                    Err(r) => errs.push(format!("{}: {}", p.display(), r.reason())),
                }
            }
            TaskResult { id: t.id.clone(), ok: errs.is_empty(), output: if errs.is_empty() { format!("{} launchers moved to Trash", t.trash_paths.len()) } else { errs.join("\n") } }
        } else {
            let mut ok = true;
            let mut out = String::new();
            for argv in &t.argv {
                let args: Vec<String> = argv[1..].to_vec();
                match ctx.runner().run(&argv[0], &args, std::time::Duration::from_secs(600)) {
                    Ok(o) => {
                        ok &= o.success();
                        let text = format!("{}{}", o.stdout, o.stderr);
                        let tail: Vec<&str> = text.lines().rev().take(3).collect();
                        out.push_str(&tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
                    }
                    Err(e) => {
                        ok = false;
                        out.push_str(&e.to_string());
                    }
                }
            }
            if !dry_run {
                log.record(&t.commands.join("; "), 0, "command", if ok { "done" } else { "failed" }, out.lines().last().unwrap_or(""));
            }
            TaskResult { id: t.id.clone(), ok, output: out.trim().to_string() }
        };
        let e = if res.ok { Event::Removed { path: t.title.clone(), bytes: 0, action: "command".into() } } else { Event::Failed { path: t.title.clone(), reason: res.output.clone() } };
        sink(&e, Some(&res));
        results.push(res);
    }
    if !roots.is_empty() {
        let root_results: Vec<TaskResult> = if dry_run {
            roots.iter().map(|r| TaskResult { id: r.id.clone(), ok: true, output: format!("would run as admin: {}", r.display) }).collect()
        } else if !allow_root {
            roots.iter().map(|r| TaskResult { id: r.id.clone(), ok: false, output: "admin actions were not confirmed".into() }).collect()
        } else {
            match privileged::run_batch(ctx.runner(), &roots) {
                Ok(rs) => rs.into_iter().map(|r| TaskResult { id: r.id, ok: r.ok, output: r.output }).collect(),
                Err(e) => roots.iter().map(|r| TaskResult { id: r.id.clone(), ok: false, output: e.message.clone() }).collect(),
            }
        };
        for res in root_results {
            let title = tasks.iter().find(|t| t.id == res.id).map(|t| t.title.clone()).unwrap_or(res.id.clone());
            if !dry_run {
                log.record(&format!("[admin] {title}"), 0, "command", if res.ok { "done" } else { "failed" }, res.output.lines().last().unwrap_or(""));
            }
            let e = if res.ok { Event::Removed { path: title, bytes: 0, action: "command".into() } } else { Event::Failed { path: title, reason: res.output.clone() } };
            sink(&e, Some(&res));
            results.push(res);
        }
    }
    let failed = results.iter().filter(|r| !r.ok).count();
    log.finish(if failed > 0 { "partial" } else { "done" }, &format!("{} of {} tasks succeeded", results.len() - failed, results.len()));
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apt_removals() {
        let text = "NOTE: This is only a simulation!\nReading package lists...\nRemv linux-headers-6.8.0-31 [6.8.0-31.31]\nRemv libfoo1 [1.2-3]\nRemv evil$(x) [1]\n";
        assert_eq!(parse_apt_removals(text), vec!["linux-headers-6.8.0-31".to_string(), "libfoo1".to_string()]);
    }

    #[test]
    fn exec_parsing() {
        assert_eq!(first_program("env GDK_BACKEND=x11 /opt/app/bin %U").as_deref(), Some("/opt/app/bin"));
        assert_eq!(first_program("\"/home/u/Apps/My App.AppImage\" --no-sandbox").as_deref(), Some("/home/u/Apps/My App.AppImage"));
        assert_eq!(first_program("firefox %u").as_deref(), Some("firefox"));
    }

    #[test]
    fn finds_broken_launchers() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("ok.desktop"), "[Desktop Entry]\nType=Application\nExec=sh -c true\n").unwrap();
        fs::write(d.path().join("gone.desktop"), "[Desktop Entry]\nType=Application\nExec=/opt/nowhere/app %U\n").unwrap();
        fs::write(d.path().join("notes.txt"), "Exec=/nope").unwrap();
        let b = broken_launchers(d.path());
        assert_eq!(b.len(), 1);
        assert!(b[0].ends_with("gone.desktop"));
    }
}
