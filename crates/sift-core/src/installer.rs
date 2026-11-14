//! Installers: packages, AppImages, disk images and archives left in
//! Downloads / Desktop after you installed what they contained.

use crate::cancel::CancelToken;
use crate::ctx::Ctx;
use crate::fsutil;
use crate::openfiles::Activity;
use crate::ops::OpLog;
use crate::removal::{Event, Remover, Summary};
use serde::Serialize;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    LinuxPackage,
    AppImage,
    DiskImage,
    ForeignInstaller,
    Archive,
}

impl Category {
    pub fn title(&self) -> &'static str {
        match self {
            Category::LinuxPackage => "Linux packages",
            Category::AppImage => "AppImages",
            Category::DiskImage => "Disk images",
            Category::ForeignInstaller => "Windows, macOS & Android installers",
            Category::Archive => "Archives",
        }
    }
}

/// Classify a file name by extension (case-insensitive).
pub fn classify(name: &str) -> Option<Category> {
    let n = name.to_ascii_lowercase();
    let ends = |exts: &[&str]| exts.iter().any(|e| n.ends_with(e));
    if ends(&[".deb", ".rpm", ".run", ".flatpakref", ".flatpakrepo", ".flatpak", ".snap", ".pkg.tar.zst", ".pkg.tar.xz"]) {
        Some(Category::LinuxPackage)
    } else if ends(&[".appimage"]) {
        Some(Category::AppImage)
    } else if ends(&[".iso"]) {
        Some(Category::DiskImage)
    } else if ends(&[".exe", ".msi", ".msix", ".dmg", ".pkg", ".apk", ".xapk"]) {
        Some(Category::ForeignInstaller)
    } else if ends(&[".zip", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst", ".7z", ".rar"]) {
        Some(Category::Archive)
    } else {
        None
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct InstallerFile {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    pub category: Category,
    pub bytes: u64,
    pub mtime: i64,
    pub age_days: i64,
    pub selected: bool,
    /// Why it is not pre-selected / can't be removed now.
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct InstallerScan {
    pub dirs: Vec<PathBuf>,
    pub items: Vec<InstallerFile>,
    pub total_bytes: u64,
    pub selected_bytes: u64,
}

/// AppImages referenced by a launcher in ~/.local/share/applications.
fn launcher_targets(ctx: &Ctx) -> Vec<String> {
    let dir = ctx.paths.data.join("applications");
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Ok(text) = fs::read_to_string(e.path()) {
                for line in text.lines() {
                    if let Some(exec) = line.strip_prefix("Exec=") {
                        out.push(exec.to_string());
                    }
                }
            }
        }
    }
    out
}

fn candidates(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if ft.is_file() && classify(&name).is_some() {
            out.push(e.path());
        } else if ft.is_dir() && depth > 0 && !name.starts_with('.') {
            candidates(&e.path(), depth - 1, out);
        }
    }
}

pub fn scan(ctx: &Ctx, cancel: &CancelToken, on_item: &mut dyn FnMut(&InstallerFile)) -> InstallerScan {
    let dirs = ctx.settings.installer_dir_paths(&ctx.paths);
    let guard = ctx.guard();
    let act = Activity::capture();
    let launchers = launcher_targets(ctx);
    let now = crate::now_unix();
    let min_age = ctx.settings.installer_min_age_days as i64;
    let mut scan = InstallerScan { dirs: dirs.clone(), ..Default::default() };
    for dir in &dirs {
        let mut files = Vec::new();
        candidates(dir, 1, &mut files);
        for path in files {
            if cancel.is_cancelled() {
                break;
            }
            if guard.check_lexical(&path, &dirs).is_err() {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(category) = classify(&name) else { continue };
            let age_days = (now - meta.mtime()).max(0) / 86_400;
            let path_str = path.display().to_string();
            let mut note = None;
            let nested = path.parent() != Some(dir.as_path());
            if act.user_of(&path).is_some() {
                note = Some("in use right now".to_string());
            } else if category == Category::AppImage && launchers.iter().any(|l| l.contains(&path_str)) {
                note = Some("an app launcher runs this AppImage".to_string());
            } else if nested {
                note = Some("inside a folder — may belong to a project".to_string());
            } else if age_days < min_age {
                note = Some(format!("downloaded {age_days} days ago"));
            } else if category == Category::Archive {
                note = Some("archives may hold files you still need".to_string());
            }
            let item = InstallerFile {
                id: path_str,
                bytes: fsutil::alloc_bytes(&meta),
                mtime: meta.mtime(),
                age_days,
                selected: note.is_none(),
                note,
                name,
                category,
                path,
            };
            on_item(&item);
            scan.items.push(item);
        }
    }
    scan.items.sort_by(|a, b| a.category.cmp(&b.category).then(b.bytes.cmp(&a.bytes)));
    scan.total_bytes = scan.items.iter().map(|i| i.bytes).sum();
    scan.selected_bytes = scan.items.iter().filter(|i| i.selected).map(|i| i.bytes).sum();
    scan
}

pub fn apply(ctx: &Ctx, paths: &[PathBuf], dry_run: bool, permanent: bool, cancel: &CancelToken, sink: &mut dyn FnMut(&Event)) -> Summary {
    let dirs = ctx.settings.installer_dir_paths(&ctx.paths);
    let guard = ctx.guard();
    let trash = ctx.trash();
    let act = Activity::capture();
    let mut log = if dry_run { OpLog::disabled() } else { OpLog::begin(&ctx.paths, "installer", false, &ctx.source) };
    let mut rm = Remover::new(&guard, &trash, permanent, dry_run, &mut log);
    for (i, p) in paths.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        sink(&Event::Progress { done: i, total: paths.len(), current: p.display().to_string() });
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let within = dirs.iter().any(|d| p.parent() == Some(d.as_path()) || p.parent().and_then(|x| x.parent()) == Some(d.as_path()));
        if classify(&name).is_none() || !within {
            rm.refuse(p, "not an installer in your Downloads/Desktop folders", sink);
            continue;
        }
        if fs::symlink_metadata(p).map(|m| !m.is_file()).unwrap_or(true) {
            rm.refuse(p, "not a regular file", sink);
            continue;
        }
        if let Some(by) = act.user_of(p) {
            rm.skip(p, &format!("in use by {by}"), sink);
            continue;
        }
        rm.remove(p, &dirs, false, None, sink);
    }
    let summary = rm.summary.clone();
    sink(&Event::Done(summary.clone()));
    log.finish(if summary.failed > 0 { "partial" } else { "done" }, &summary.headline());
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::runner::FakeRunner;
    use crate::settings::Settings;
    use std::sync::Arc;

    #[test]
    fn classifies() {
        assert_eq!(classify("code_1.95_amd64.deb"), Some(Category::LinuxPackage));
        assert_eq!(classify("Obsidian-1.7.AppImage"), Some(Category::AppImage));
        assert_eq!(classify("pop-os_24.04.iso"), Some(Category::DiskImage));
        assert_eq!(classify("Setup.EXE"), Some(Category::ForeignInstaller));
        assert_eq!(classify("src.tar.gz"), Some(Category::Archive));
        assert_eq!(classify("report.pdf"), None);
    }

    #[test]
    fn scan_and_remove() {
        let d = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(d.path()).unwrap();
        fs::create_dir_all(home.join("Downloads/sub")).unwrap();
        fs::write(home.join("Downloads/app.deb"), vec![0u8; 4000]).unwrap();
        fs::write(home.join("Downloads/sub/tool.AppImage"), vec![0u8; 4000]).unwrap();
        fs::write(home.join("Downloads/notes.txt"), b"keep").unwrap();
        let settings = Settings { installer_min_age_days: 0, ..Default::default() };
        let ctx = Ctx::with(Paths::for_home(home.clone(), true), settings, Arc::new(FakeRunner::default()));
        let s = scan(&ctx, &CancelToken::new(), &mut |_| {});
        assert_eq!(s.items.len(), 2);
        let top = s.items.iter().find(|i| i.name == "app.deb").unwrap();
        let nested = s.items.iter().find(|i| i.name == "tool.AppImage").unwrap();
        assert!(top.selected);
        assert!(!nested.selected, "files inside subfolders are never pre-selected");
        let mut paths: Vec<PathBuf> = s.items.iter().map(|i| i.path.clone()).collect();
        paths.push(home.join("Downloads/notes.txt"));
        let sum = apply(&ctx, &paths, false, false, &CancelToken::new(), &mut |_| {});
        assert_eq!(sum.removed, 2);
        assert_eq!(sum.refused, 1);
        assert!(home.join("Downloads/notes.txt").exists());
        assert!(!home.join("Downloads/app.deb").exists());
    }
}
