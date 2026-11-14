//! Filesystem helpers: du-style sizing that never follows symlinks or
//! crosses mounts, and permanent removal that copes with read-only trees.

use crate::cancel::CancelToken;
use serde::Serialize;
use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct DuInfo {
    /// Allocated bytes (what removing it would actually free).
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
    /// Newest modification time seen (unix seconds).
    pub newest_mtime: i64,
    pub errors: u64,
    pub partial: bool,
}

/// Allocated size of one inode.
pub fn alloc_bytes(meta: &Metadata) -> u64 {
    meta.blocks() * 512
}

pub fn mtime(meta: &Metadata) -> i64 {
    meta.mtime()
}

/// Size a file or tree. Stays on the starting device, never follows
/// symlinks, counts hard-linked inodes once.
pub fn du(path: &Path, cancel: Option<&CancelToken>) -> DuInfo {
    let mut info = DuInfo::default();
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => {
            info.errors += 1;
            return info;
        }
    };
    info.newest_mtime = meta.mtime();
    if !meta.is_dir() {
        info.bytes = alloc_bytes(&meta);
        info.files = 1;
        return info;
    }
    let dev = meta.dev();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    info.bytes = alloc_bytes(&meta);
    info.dirs = 1;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.map(|c| c.is_cancelled()).unwrap_or(false) {
            info.partial = true;
            break;
        }
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => {
                info.errors += 1;
                continue;
            }
        };
        for entry in rd.flatten() {
            let m = match entry.metadata() {
                Ok(m) => m,
                Err(_) => {
                    info.errors += 1;
                    continue;
                }
            };
            if m.mtime() > info.newest_mtime {
                info.newest_mtime = m.mtime();
            }
            if m.is_dir() {
                if m.dev() != dev {
                    continue;
                }
                info.dirs += 1;
                info.bytes += alloc_bytes(&m);
                stack.push(entry.path());
            } else {
                if m.nlink() > 1 && !seen.insert((m.dev(), m.ino())) {
                    continue;
                }
                info.files += 1;
                info.bytes += alloc_bytes(&m);
            }
        }
    }
    info
}

/// Remove a file, symlink, or whole tree permanently. Never follows symlinks.
/// Read-only directories you own are made writable first (Go's module cache
/// and some package stores are deliberately read-only).
pub fn remove_permanently(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        match fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                make_tree_writable(path);
                fs::remove_dir_all(path)
            }
            Err(e) => Err(e),
        }
    } else {
        fs::remove_file(path)
    }
}

/// Add owner write+exec to every directory in a tree we own (no symlink follow).
pub fn make_tree_writable(root: &Path) {
    let uid = crate::current_uid();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(meta) = fs::symlink_metadata(&dir) else { continue };
        if !meta.is_dir() || meta.uid() != uid {
            continue;
        }
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0o700 {
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(mode | 0o700));
        }
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    stack.push(e.path());
                }
            }
        }
    }
}

/// Make a single directory owner-writable (needed to rename a read-only
/// directory into the Trash, which rewrites its `..`).
pub fn make_dir_writable(dir: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(dir) else { return false };
    if !meta.is_dir() || meta.uid() != crate::current_uid() {
        return false;
    }
    let mode = meta.permissions().mode();
    fs::set_permissions(dir, fs::Permissions::from_mode(mode | 0o700)).is_ok()
}

/// Match one path component against a pattern with `*` wildcards.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == t[ti] || p[pi] == '?') {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            mark = ti;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Expand a base-relative pattern like `.cache/mozilla/*/cache2` into the
/// existing paths it names. Wildcard components only match real directories
/// (not symlinks); the final component may be any entry type.
pub fn expand_pattern(base: &Path, pattern: &str) -> Vec<std::path::PathBuf> {
    let comps: Vec<&str> = pattern.split('/').filter(|c| !c.is_empty()).collect();
    let mut current = vec![base.to_path_buf()];
    for (i, comp) in comps.iter().enumerate() {
        let last = i + 1 == comps.len();
        let mut next = Vec::new();
        for dir in &current {
            if comp.contains('*') || comp.contains('?') {
                let Ok(rd) = fs::read_dir(dir) else { continue };
                let mut names: Vec<_> = rd
                    .flatten()
                    .filter(|e| wildcard_match(comp, &e.file_name().to_string_lossy()))
                    .filter(|e| last || e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|e| e.path())
                    .collect();
                names.sort();
                next.extend(names);
            } else {
                let p = dir.join(comp);
                match fs::symlink_metadata(&p) {
                    Ok(m) if last || m.is_dir() => next.push(p),
                    _ => {}
                }
            }
        }
        current = next;
        if current.is_empty() {
            break;
        }
    }
    current
}

/// True if `path` (relative to `base`) could be produced by `pattern`,
/// checked component by component without touching the filesystem.
pub fn pattern_covers(base: &Path, pattern: &str, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(base) else { return false };
    let comps: Vec<&str> = pattern.split('/').filter(|c| !c.is_empty()).collect();
    let rel_comps: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    comps.len() == rel_comps.len() && comps.iter().zip(rel_comps.iter()).all(|(p, r)| wildcard_match(p, r))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("cmake-build-*", "cmake-build-debug"));
        assert!(!wildcard_match("cmake-build-*", "cmake"));
        assert!(wildcard_match("*.AppImage", "Obsidian-1.5.AppImage"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("abc", "abcd"));
    }

    #[test]
    fn expands_and_covers() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path();
        fs::create_dir_all(base.join(".cache/mozilla/firefox/abc.default/cache2")).unwrap();
        fs::create_dir_all(base.join(".cache/mozilla/firefox/xyz.dev/cache2")).unwrap();
        let found = expand_pattern(base, ".cache/mozilla/firefox/*/cache2");
        assert_eq!(found.len(), 2);
        assert!(pattern_covers(base, ".cache/mozilla/firefox/*/cache2", &found[0]));
        assert!(!pattern_covers(base, ".cache/mozilla/firefox/*/cache2", &base.join(".cache/mozilla")));
        assert!(!pattern_covers(base, ".cache/*", &base.join(".config/x")));
    }

    #[test]
    fn du_counts_hardlinks_once_and_skips_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("t");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("a/f"), vec![1u8; 10_000]).unwrap();
        fs::hard_link(root.join("a/f"), root.join("a/g")).unwrap();
        std::os::unix::fs::symlink("/usr", root.join("link")).unwrap();
        let info = du(&root, None);
        assert_eq!(info.files, 2); // f (once) + the symlink entry itself
        assert!(info.bytes >= 10_000);
        assert!(info.bytes < 40_000);
    }

    #[test]
    fn removes_read_only_trees() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("ro");
        fs::create_dir_all(root.join("inner")).unwrap();
        fs::write(root.join("inner/f"), b"x").unwrap();
        fs::set_permissions(root.join("inner"), fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
        remove_permanently(&root).unwrap();
        assert!(!root.exists());
    }
}
