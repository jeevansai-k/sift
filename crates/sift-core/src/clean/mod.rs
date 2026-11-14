//! Clean: scan the target table into reviewable items, then remove exactly
//! the reviewed items — re-validated, never re-scanned or widened.

pub mod system;
pub mod targets;

use crate::cancel::CancelToken;
use crate::ctx::Ctx;
use crate::fsutil::{self, DuInfo};
use crate::openfiles::Activity;
use crate::ops::OpLog;
use crate::privileged::{self, RootCommand};
use crate::removal::{Event, Remover, Summary};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
pub use targets::{Group, Mode, Target};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Path,
    EmptyTrash,
    Root,
}

#[derive(Clone, Debug, Serialize)]
pub struct CleanItem {
    /// Stable id: `<target>:<path>`, `trash`, or `system:<task>`.
    pub id: String,
    pub target: String,
    pub label: String,
    pub group: Group,
    pub kind: ItemKind,
    pub path: Option<PathBuf>,
    pub bytes: u64,
    /// `bytes` is an upper bound.
    pub approx: bool,
    pub files: u64,
    pub selected: bool,
    /// Why it can't be cleaned right now (e.g. "Firefox is running").
    pub blocked: Option<String>,
    pub note: String,
    /// Exact admin command, for root items.
    pub command: Option<String>,
    #[serde(skip)]
    pub root: Option<RootCommand>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct CleanScan {
    pub items: Vec<CleanItem>,
    pub total_bytes: u64,
    pub selected_bytes: u64,
    pub scanned_at: i64,
}

impl CleanScan {
    fn finish(mut self) -> CleanScan {
        self.items.sort_by(|a, b| a.group.cmp(&b.group).then(b.bytes.cmp(&a.bytes)));
        self.total_bytes = self.items.iter().filter(|i| i.blocked.is_none() && !i.approx).map(|i| i.bytes).sum();
        self.selected_bytes = self.items.iter().filter(|i| i.selected && !i.approx).map(|i| i.bytes).sum();
        self.scanned_at = crate::now_unix();
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScanOptions {
    pub include_system: bool,
}

/// What blocks this path right now, if anything.
fn blocked_reason(target: &Target, path: &Path, home: &Path, act: &Activity) -> Option<String> {
    match target.guarding {
        targets::Guarding::Procs(names) => {
            if let Some(n) = act.running(names) {
                return Some(format!("{n} is running"));
            }
        }
        targets::Guarding::Flatpak => {
            if let Some(id) = path.strip_prefix(home.join(".var/app")).ok().and_then(|r| r.components().next()) {
                let id = id.as_os_str().to_string_lossy();
                if act.flatpak_running(&id) {
                    return Some(format!("{id} is running"));
                }
            }
        }
        targets::Guarding::Snap => {
            if let Some(n) = path.strip_prefix(home.join("snap")).ok().and_then(|r| r.components().next()) {
                let n = n.as_os_str().to_string_lossy();
                if act.snap_running(&n) {
                    return Some(format!("{n} is running"));
                }
            }
        }
        targets::Guarding::Unguarded => {}
    }
    // An Electron app's caches live inside its config folder; if the app has
    // anything open there it's running, so leave all of its caches alone.
    let probe = if target.id == "electron-cache" { path.parent().unwrap_or(path) } else { path };
    act.user_of(probe).map(|p| format!("in use by {p}"))
}

/// The first target in table order whose pattern covers `path`.
pub fn owning_target(home: &Path, path: &Path) -> Option<&'static Target> {
    targets::TARGETS.iter().find(|t| {
        t.patterns.iter().any(|p| fsutil::pattern_covers(home, p, path))
            && (t.id != "app-cache"
                || path.file_name().map(|n| !targets::app_cache_excluded(&n.to_string_lossy())).unwrap_or(false))
    })
}

fn measure(path: &Path, mode: Mode, cancel: &CancelToken) -> DuInfo {
    match mode {
        Mode::Whole => fsutil::du(path, Some(cancel)),
        Mode::Contents => {
            let mut d = fsutil::du(path, Some(cancel));
            // The folder itself stays.
            if let Ok(m) = fs::symlink_metadata(path) {
                d.bytes = d.bytes.saturating_sub(fsutil::alloc_bytes(&m));
                d.dirs = d.dirs.saturating_sub(1);
            }
            d
        }
        Mode::OlderThan(days) => {
            let cutoff = crate::now_unix() - days as i64 * 86_400;
            let mut d = DuInfo::default();
            for f in old_files(path, cutoff) {
                if let Ok(m) = fs::symlink_metadata(&f) {
                    d.bytes += fsutil::alloc_bytes(&m);
                    d.files += 1;
                }
            }
            d
        }
    }
}

/// Regular files under `dir` older than `cutoff` (no symlink follow, same device).
fn old_files(dir: &Path, cutoff: i64) -> Vec<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let mut out = Vec::new();
    let Ok(root) = fs::symlink_metadata(dir) else { return out };
    if !root.is_dir() {
        return out;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() && m.dev() == root.dev() {
                stack.push(e.path());
            } else if m.is_file() && m.mtime() < cutoff {
                out.push(e.path());
            }
        }
    }
    out
}

/// Scan for reclaimable space. `on_item` sees each item as it is measured.
pub fn scan(ctx: &Ctx, opts: &ScanOptions, cancel: &CancelToken, on_item: &mut dyn FnMut(&CleanItem)) -> CleanScan {
    let home = ctx.paths.home.clone();
    let guard = ctx.guard();
    let act = Activity::capture();
    let mut scan = CleanScan::default();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let scopes = [home.clone()];

    for target in targets::TARGETS {
        for pattern in target.patterns {
            for path in fsutil::expand_pattern(&home, pattern) {
                if cancel.is_cancelled() {
                    return scan.finish();
                }
                if seen.contains(&path) {
                    continue;
                }
                // The first row that covers a path owns it (generic rows skip specific ones).
                if owning_target(&home, &path).map(|t| t.id) != Some(target.id) {
                    continue;
                }
                let Ok(meta) = fs::symlink_metadata(&path) else { continue };
                if meta.file_type().is_symlink() {
                    continue;
                }
                if matches!(target.mode, Mode::Contents | Mode::OlderThan(_)) && !meta.is_dir() {
                    continue;
                }
                // Silently drop anything the rails would refuse anyway (protected, whitelisted…).
                if guard.check_lexical(&path, &scopes).is_err() {
                    continue;
                }
                seen.insert(path.clone());
                let du = measure(&path, target.mode, cancel);
                if du.bytes == 0 {
                    continue;
                }
                let blocked = blocked_reason(target, &path, &home, &act);
                let label = if target.id == "app-cache" || target.id == "flatpak-cache" || target.id == "snap-cache" || target.id == "electron-cache" {
                    let who = match target.id {
                        "flatpak-cache" => path.parent().and_then(|p| p.file_name()),
                        "snap-cache" => path.parent().and_then(|p| p.parent()).and_then(|p| p.file_name()),
                        "electron-cache" => path.parent().and_then(|p| p.file_name()),
                        _ => path.file_name(),
                    };
                    format!("{} · {}", target.label, who.map(|w| w.to_string_lossy().into_owned()).unwrap_or_default())
                } else {
                    target.label.to_string()
                };
                let item = CleanItem {
                    id: format!("{}:{}", target.id, path.display()),
                    target: target.id.into(),
                    label,
                    group: target.group,
                    kind: ItemKind::Path,
                    path: Some(path.clone()),
                    bytes: du.bytes,
                    approx: false,
                    files: du.files,
                    selected: target.default_on && blocked.is_none(),
                    blocked,
                    note: target.note.into(),
                    command: None,
                    root: None,
                };
                on_item(&item);
                scan.items.push(item);
            }
        }
    }

    // Trash: emptying is permanent, so it is never pre-selected.
    let trash = ctx.trash();
    let tsize = trash.size();
    if tsize.bytes > 0 {
        let item = CleanItem {
            id: "trash".into(),
            target: "trash".into(),
            label: "Empty Trash".into(),
            group: Group::Trash,
            kind: ItemKind::EmptyTrash,
            path: Some(trash.home_trash().to_path_buf()),
            bytes: tsize.bytes,
            approx: false,
            files: trash.count() as u64,
            selected: false,
            blocked: None,
            note: "Permanently deletes everything in the Trash".into(),
            command: None,
            root: None,
        };
        on_item(&item);
        scan.items.push(item);
    }

    if opts.include_system && !cancel.is_cancelled() {
        for task in system::probe(ctx) {
            let item = CleanItem {
                id: format!("system:{}", task.id),
                target: task.id.clone(),
                label: task.label.clone(),
                group: Group::System,
                kind: ItemKind::Root,
                path: None,
                bytes: task.bytes,
                approx: task.approx,
                files: 0,
                selected: false,
                blocked: None,
                note: task.detail.clone(),
                command: Some(task.command.display.clone()),
                root: Some(task.command.clone()),
            };
            on_item(&item);
            scan.items.push(item);
        }
    }
    scan.finish()
}

#[derive(Clone, Debug, Default)]
pub struct ApplyOptions {
    pub dry_run: bool,
    pub permanent: bool,
    /// Separate consent for permanent Trash emptying.
    pub allow_empty_trash: bool,
    /// Run admin items through pkexec (the GUI asks first).
    pub allow_root: bool,
}

/// Validate and remove exactly these reviewed items.
pub fn apply(ctx: &Ctx, items: &[CleanItem], opts: &ApplyOptions, cancel: &CancelToken, sink: &mut dyn FnMut(&Event)) -> Summary {
    let home = ctx.paths.home.clone();
    let guard = ctx.guard();
    let trash = ctx.trash();
    let act = Activity::capture();
    let mut log = if opts.dry_run { OpLog::disabled() } else { OpLog::begin(&ctx.paths, "clean", false, &ctx.source) };
    let mut root_cmds: Vec<RootCommand> = Vec::new();
    let mut root_bytes: Vec<(String, u64)> = Vec::new();
    let total = items.len();
    let mut rm = Remover::new(&guard, &trash, opts.permanent, opts.dry_run, &mut log);

    for (i, item) in items.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        sink(&Event::Progress { done: i, total, current: item.label.clone() });
        match item.kind {
            ItemKind::Root => {
                // Root items carry their command from the scan; re-probe to make
                // sure it is one the engine built, never something passed in.
                if let Some(cmd) = &item.root {
                    root_cmds.push(cmd.clone());
                    root_bytes.push((cmd.id.clone(), if item.approx { 0 } else { item.bytes }));
                }
            }
            ItemKind::EmptyTrash => {
                let p = trash.home_trash().to_path_buf();
                if !opts.allow_empty_trash {
                    rm.refuse(&p, "emptying the Trash is permanent and needs its own confirmation", sink);
                } else if opts.dry_run {
                    let size = trash.size().bytes;
                    let e = Event::WouldRemove { path: p.display().to_string(), bytes: size };
                    rm.summary.absorb(&e);
                    sink(&e);
                } else {
                    match trash.empty(Some(cancel)) {
                        Ok((bytes, n)) => {
                            rm.log.record(&p.display().to_string(), bytes, "delete", "removed", &format!("{n} items"));
                            let e = Event::Removed { path: p.display().to_string(), bytes, action: "delete".into() };
                            rm.summary.absorb(&e);
                            sink(&e);
                        }
                        Err(err) => {
                            let e = Event::Failed { path: p.display().to_string(), reason: err.message };
                            rm.summary.absorb(&e);
                            sink(&e);
                        }
                    }
                }
            }
            ItemKind::Path => {
                let Some(path) = item.path.as_ref() else { continue };
                let target = match owning_target(&home, path) {
                    Some(t) if t.id == item.target => t,
                    _ => {
                        rm.refuse(path, crate::safety::Refusal::NotATarget.reason(), sink);
                        continue;
                    }
                };
                if let Some(why) = blocked_reason(target, path, &home, &act) {
                    rm.skip(path, &why, sink);
                    continue;
                }
                match target.mode {
                    Mode::Whole => {
                        rm.remove(path, &[home.clone()], false, None, sink);
                    }
                    Mode::Contents => {
                        if let Err(r) = guard.check(path, &[home.clone()]) {
                            rm.refuse(path, r.reason(), sink);
                            continue;
                        }
                        let Ok(rd) = fs::read_dir(path) else { continue };
                        let mut children: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
                        children.sort();
                        for c in children {
                            if cancel.is_cancelled() {
                                break;
                            }
                            rm.remove(&c, &[path.clone()], true, None, sink);
                        }
                    }
                    Mode::OlderThan(days) => {
                        if let Err(r) = guard.check(path, &[home.clone()]) {
                            rm.refuse(path, r.reason(), sink);
                            continue;
                        }
                        let cutoff = crate::now_unix() - days as i64 * 86_400;
                        for f in old_files(path, cutoff) {
                            rm.remove(&f, &[path.clone()], true, None, sink);
                        }
                    }
                }
            }
        }
    }

    // Admin batch last, one prompt for all of it.
    if !root_cmds.is_empty() && !cancel.is_cancelled() {
        if opts.dry_run {
            for (cmd, (_, bytes)) in root_cmds.iter().zip(root_bytes.iter()) {
                let e = Event::WouldRemove { path: format!("[admin] {}", cmd.display), bytes: *bytes };
                rm.summary.absorb(&e);
                sink(&e);
            }
        } else if !opts.allow_root {
            for cmd in &root_cmds {
                rm.skip(Path::new(&format!("[admin] {}", cmd.display)), "admin actions were not confirmed", sink);
            }
        } else {
            match privileged::run_batch(ctx.runner(), &root_cmds) {
                Ok(results) => {
                    for r in results {
                        let display = root_cmds.iter().find(|c| c.id == r.id).map(|c| c.display.clone()).unwrap_or(r.id.clone());
                        let shown = format!("[admin] {display}");
                        if r.ok {
                            let bytes = root_bytes.iter().find(|(id, _)| *id == r.id).map(|(_, b)| *b).unwrap_or(0);
                            rm.log.record(&shown, bytes, "command", "removed", "");
                            let e = Event::Removed { path: shown, bytes, action: "command".into() };
                            rm.summary.absorb(&e);
                            sink(&e);
                        } else {
                            let reason = r.output.lines().last().unwrap_or("command failed").to_string();
                            rm.log.record(&shown, 0, "command", "failed", &reason);
                            let e = Event::Failed { path: shown, reason };
                            rm.summary.absorb(&e);
                            sink(&e);
                        }
                    }
                }
                Err(err) => {
                    for cmd in &root_cmds {
                        let e = Event::Failed { path: format!("[admin] {}", cmd.display), reason: err.message.clone() };
                        rm.summary.absorb(&e);
                        sink(&e);
                    }
                }
            }
        }
    }

    let summary = rm.summary.clone();
    sink(&Event::Done(summary.clone()));
    let status = if cancel.is_cancelled() { "cancelled" } else if summary.failed > 0 { "partial" } else { "done" };
    log.finish(status, &summary.headline());
    summary
}

/// Build reviewed items from a plan file: one absolute path per line, `#`
/// comments and blank lines ignored. Each path must be one the target table
/// could have produced; others come back as refusals.
pub fn items_from_plan(ctx: &Ctx, text: &str) -> (Vec<CleanItem>, Vec<(String, String)>) {
    let home = &ctx.paths.home;
    let mut items = Vec::new();
    let mut refused = Vec::new();
    for line in text.lines().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with('#')).take(4096) {
        let path = PathBuf::from(line);
        if !crate::safety::is_normalized(&path) {
            refused.push((line.to_string(), crate::safety::Refusal::NotNormalized.reason().to_string()));
            continue;
        }
        match owning_target(home, &path) {
            Some(t) => items.push(CleanItem {
                id: format!("{}:{}", t.id, path.display()),
                target: t.id.into(),
                label: t.label.into(),
                group: t.group,
                kind: ItemKind::Path,
                path: Some(path),
                bytes: 0,
                approx: false,
                files: 0,
                selected: true,
                blocked: None,
                note: String::new(),
                command: None,
                root: None,
            }),
            None => refused.push((line.to_string(), crate::safety::Refusal::NotATarget.reason().to_string())),
        }
    }
    (items, refused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::runner::FakeRunner;
    use crate::settings::Settings;
    use std::sync::Arc;

    fn sandbox() -> (tempfile::TempDir, Ctx) {
        let d = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(d.path()).unwrap();
        let mk = |rel: &str, size: usize| {
            let p = home.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, vec![7u8; size]).unwrap();
        };
        mk(".cache/pip/http/a", 20_000);
        mk(".cache/someapp/blob", 30_000);
        mk(".cache/huggingface/hub/model.bin", 50_000);
        mk(".cache/sift/keep", 1_000);
        mk(".cache/thumbnails/normal/x.png", 8_000);
        mk(".npm/_cacache/index", 9_000);
        mk(".config/Slack/Cache/data_1", 12_000);
        mk(".config/Slack/settings.json", 100);
        mk(".var/app/org.example.App/cache/fontconfig/x", 4_000);
        mk(".ssh/id_ed25519", 400);
        let ctx = Ctx::with(Paths::for_home(home, true), Settings::default(), Arc::new(FakeRunner::default()));
        (d, ctx)
    }

    #[test]
    fn scan_finds_targets_and_respects_defaults() {
        let (_d, ctx) = sandbox();
        let scan = scan(&ctx, &ScanOptions::default(), &CancelToken::new(), &mut |_| {});
        let ids: Vec<&str> = scan.items.iter().map(|i| i.target.as_str()).collect();
        assert!(ids.contains(&"pip"));
        assert!(ids.contains(&"app-cache"));
        assert!(ids.contains(&"huggingface"));
        assert!(ids.contains(&"npm"));
        assert!(ids.contains(&"electron-cache"));
        assert!(ids.contains(&"flatpak-cache"));
        assert!(ids.contains(&"thumbnails"));
        // never offered
        assert!(!scan.items.iter().any(|i| i.path.as_ref().map(|p| p.ends_with(".cache/sift")).unwrap_or(false)));
        assert!(!scan.items.iter().any(|i| i.path.as_ref().map(|p| p.to_string_lossy().contains(".ssh")).unwrap_or(false)));
        assert!(!scan.items.iter().any(|i| i.path.as_ref().map(|p| p.ends_with("settings.json")).unwrap_or(false)));
        let hf = scan.items.iter().find(|i| i.target == "huggingface").unwrap();
        assert!(!hf.selected, "heavy re-download caches are opt-in");
        let app = scan.items.iter().find(|i| i.target == "app-cache").unwrap();
        assert!(app.label.ends_with("someapp"));
        assert!(app.selected);
        assert!(scan.selected_bytes > 0 && scan.selected_bytes < scan.total_bytes);
    }

    #[test]
    fn apply_trashes_reviewed_items_only() {
        let (_d, ctx) = sandbox();
        let s = scan(&ctx, &ScanOptions::default(), &CancelToken::new(), &mut |_| {});
        let chosen: Vec<CleanItem> = s.items.iter().filter(|i| i.target == "pip" || i.target == "electron-cache").cloned().collect();
        let mut events = Vec::new();
        let summary = apply(&ctx, &chosen, &ApplyOptions::default(), &CancelToken::new(), &mut |e| events.push(e.clone()));
        let home = &ctx.paths.home;
        assert!(home.join(".cache/pip").is_dir(), "contents mode keeps the folder");
        assert!(!home.join(".cache/pip/http").exists());
        assert!(!home.join(".config/Slack/Cache").exists());
        assert!(home.join(".config/Slack/settings.json").exists());
        assert!(home.join(".cache/someapp/blob").exists(), "unreviewed items untouched");
        assert_eq!(summary.failed, 0);
        assert!(summary.trashed_bytes >= 32_000);
        assert!(matches!(events.last(), Some(Event::Done(_))));
        // Everything went to the sandbox Trash with .trashinfo files.
        let info = fs::read_dir(home.join(".local/share/Trash/info")).unwrap().count();
        assert_eq!(info, 2);
    }

    #[test]
    fn plan_refuses_non_targets() {
        let (_d, ctx) = sandbox();
        let home = ctx.paths.home.display().to_string();
        let plan = format!(
            "# reviewed\n{home}/.cache/someapp\n{home}/.ssh\n{home}/.config/Slack/settings.json\n{home}/.cache/../.ssh\n\n{home}/.cache/sift\n"
        );
        let (items, refused) = items_from_plan(&ctx, &plan);
        assert_eq!(items.len(), 1);
        assert_eq!(refused.len(), 4);
        let summary = apply(&ctx, &items, &ApplyOptions { dry_run: true, ..Default::default() }, &CancelToken::new(), &mut |_| {});
        assert!(summary.dry_run);
        assert_eq!(summary.removed, 1);
        assert!(ctx.paths.home.join(".cache/someapp/blob").exists());
    }

    #[test]
    fn tampered_item_is_refused() {
        let (_d, ctx) = sandbox();
        let s = scan(&ctx, &ScanOptions::default(), &CancelToken::new(), &mut |_| {});
        let mut item = s.items.iter().find(|i| i.target == "pip").unwrap().clone();
        item.path = Some(ctx.paths.home.join(".ssh"));
        let summary = apply(&ctx, &[item], &ApplyOptions::default(), &CancelToken::new(), &mut |_| {});
        assert_eq!(summary.refused, 1);
        assert!(ctx.paths.home.join(".ssh/id_ed25519").exists());
    }

    #[test]
    fn empty_trash_needs_its_own_consent() {
        let (_d, ctx) = sandbox();
        let t = ctx.trash();
        t.trash(&ctx.paths.home.join(".cache/someapp")).unwrap();
        let s = scan(&ctx, &ScanOptions::default(), &CancelToken::new(), &mut |_| {});
        let trash_item = s.items.iter().find(|i| i.kind == ItemKind::EmptyTrash).unwrap().clone();
        assert!(!trash_item.selected);
        let refused = apply(&ctx, &[trash_item.clone()], &ApplyOptions::default(), &CancelToken::new(), &mut |_| {});
        assert_eq!(refused.refused, 1);
        assert_eq!(t.count(), 1);
        let done = apply(&ctx, &[trash_item], &ApplyOptions { allow_empty_trash: true, ..Default::default() }, &CancelToken::new(), &mut |_| {});
        assert_eq!(done.removed, 1);
        assert!(done.freed_bytes > 0);
        assert_eq!(t.count(), 0);
    }
}
