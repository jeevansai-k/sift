//! The single guarded remover every destructive flow uses, plus the shared
//! NDJSON event vocabulary (`removed` / `failed` / `refused` / `skipped` …
//! `done`), mirroring Burrow's stream lines.

use crate::fsutil;
use crate::ops::OpLog;
use crate::safety::Guard;
use crate::trash::Trash;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Dry run: this would be removed.
    WouldRemove { path: String, bytes: u64 },
    Removed { path: String, bytes: u64, action: String },
    Failed { path: String, reason: String },
    Refused { path: String, reason: String },
    Skipped { path: String, reason: String },
    Progress { done: usize, total: usize, current: String },
    Done(Summary),
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Summary {
    pub dry_run: bool,
    pub permanent: bool,
    pub removed: u64,
    pub failed: u64,
    pub refused: u64,
    pub skipped: u64,
    /// Bytes now sitting in the Trash (freed only when it is emptied).
    pub trashed_bytes: u64,
    /// Bytes actually freed (permanent deletes, admin commands).
    pub freed_bytes: u64,
    /// Bytes a dry run would remove.
    pub would_remove_bytes: u64,
}

impl Summary {
    pub fn total_bytes(&self) -> u64 {
        self.trashed_bytes + self.freed_bytes + self.would_remove_bytes
    }

    pub fn absorb(&mut self, e: &Event) {
        match e {
            Event::WouldRemove { bytes, .. } => {
                self.removed += 1;
                self.would_remove_bytes += bytes;
            }
            Event::Removed { bytes, action, .. } => {
                self.removed += 1;
                if action == "trash" { self.trashed_bytes += bytes } else { self.freed_bytes += bytes }
            }
            Event::Failed { .. } => self.failed += 1,
            Event::Refused { .. } => self.refused += 1,
            Event::Skipped { .. } => self.skipped += 1,
            _ => {}
        }
    }

    /// "4.87 GB moved to Trash" style one-liner.
    pub fn headline(&self) -> String {
        if self.dry_run {
            return format!("{} would be removed", crate::format::bytes(self.would_remove_bytes));
        }
        let mut parts = Vec::new();
        if self.trashed_bytes > 0 {
            parts.push(format!("{} moved to Trash", crate::format::bytes(self.trashed_bytes)));
        }
        if self.freed_bytes > 0 {
            parts.push(format!("{} freed", crate::format::bytes(self.freed_bytes)));
        }
        if parts.is_empty() {
            parts.push(if self.removed > 0 { format!("{} items removed", self.removed) } else { "Nothing removed".into() });
        }
        parts.join(", ")
    }
}

/// Removes reviewed paths one at a time through the rails.
pub struct Remover<'a> {
    pub guard: &'a Guard,
    pub trash: &'a Trash,
    pub permanent: bool,
    pub dry_run: bool,
    pub log: &'a mut OpLog,
    pub summary: Summary,
}

impl<'a> Remover<'a> {
    pub fn new(guard: &'a Guard, trash: &'a Trash, permanent: bool, dry_run: bool, log: &'a mut OpLog) -> Self {
        Remover { guard, trash, permanent, dry_run, log, summary: Summary { dry_run, permanent, ..Default::default() } }
    }

    fn emit(&mut self, e: Event, sink: &mut dyn FnMut(&Event)) {
        self.summary.absorb(&e);
        sink(&e);
    }

    pub fn refuse(&mut self, path: &Path, reason: &str, sink: &mut dyn FnMut(&Event)) {
        if !self.dry_run {
            self.log.record(&path.display().to_string(), 0, "none", "refused", reason);
        }
        self.emit(Event::Refused { path: path.display().to_string(), reason: reason.into() }, sink);
    }

    pub fn skip(&mut self, path: &Path, reason: &str, sink: &mut dyn FnMut(&Event)) {
        self.emit(Event::Skipped { path: path.display().to_string(), reason: reason.into() }, sink);
    }

    /// Remove `path` (which must sit strictly inside one of `scopes`).
    /// `child` relaxes the symlink-leaf rule for entries inside a reviewed
    /// folder (the link itself is removed, never its target).
    /// `known_bytes` skips re-measuring when the caller just measured.
    pub fn remove(
        &mut self,
        path: &Path,
        scopes: &[PathBuf],
        child: bool,
        known_bytes: Option<u64>,
        sink: &mut dyn FnMut(&Event),
    ) -> bool {
        let check = if child { self.guard.check_child(path, scopes) } else { self.guard.check(path, scopes) };
        if let Err(r) = check {
            self.refuse(path, r.reason(), sink);
            return false;
        }
        let bytes = known_bytes.unwrap_or_else(|| fsutil::du(path, None).bytes);
        let shown = path.display().to_string();
        if self.dry_run {
            self.emit(Event::WouldRemove { path: shown, bytes }, sink);
            return true;
        }
        let result = if self.permanent {
            fsutil::remove_permanently(path).map(|_| "delete").map_err(|e| e.to_string())
        } else {
            self.trash.trash(path).map(|_| "trash").map_err(|e| e.message)
        };
        match result {
            Ok(action) if std::fs::symlink_metadata(path).is_err() => {
                self.log.record(&shown, bytes, action, if action == "trash" { "trashed" } else { "removed" }, "");
                self.emit(Event::Removed { path: shown, bytes, action: action.into() }, sink);
                true
            }
            Ok(_) => {
                self.log.record(&shown, 0, "none", "failed", "still present after removal");
                self.emit(Event::Failed { path: shown, reason: "still present after removal".into() }, sink);
                false
            }
            Err(reason) => {
                self.log.record(&shown, 0, "none", "failed", &reason);
                self.emit(Event::Failed { path: shown, reason }, sink);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use std::fs;

    #[test]
    fn trashes_and_refuses() {
        let d = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(d.path()).unwrap();
        let paths = Paths::for_home(home.clone(), true);
        fs::create_dir_all(home.join(".cache/a")).unwrap();
        fs::write(home.join(".cache/a/x"), vec![0u8; 5000]).unwrap();
        let guard = Guard::new(&paths, &[]);
        let trash = Trash::new(&paths);
        let mut log = OpLog::disabled();
        let mut events = Vec::new();
        let mut r = Remover::new(&guard, &trash, false, false, &mut log);
        let scopes = [home.clone()];
        assert!(r.remove(&home.join(".cache/a"), &scopes, false, None, &mut |e| events.push(e.clone())));
        assert!(!r.remove(&home.join(".ssh"), &scopes, false, None, &mut |e| events.push(e.clone())));
        assert!(!home.join(".cache/a").exists());
        assert_eq!(r.summary.removed, 1);
        assert_eq!(r.summary.refused, 1);
        assert!(r.summary.trashed_bytes >= 5000);
        assert!(matches!(&events[0], Event::Removed { action, .. } if action == "trash"));
        assert!(r.summary.headline().contains("moved to Trash"));
    }

    #[test]
    fn dry_run_touches_nothing() {
        let d = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(d.path()).unwrap();
        let paths = Paths::for_home(home.clone(), true);
        fs::create_dir_all(home.join(".cache/a")).unwrap();
        let guard = Guard::new(&paths, &[]);
        let trash = Trash::new(&paths);
        let mut log = OpLog::disabled();
        let mut r = Remover::new(&guard, &trash, false, true, &mut log);
        assert!(r.remove(&home.join(".cache/a"), &[home.clone()], false, None, &mut |_| {}));
        assert!(home.join(".cache/a").exists());
        assert_eq!(r.summary.removed, 1);
    }
}
