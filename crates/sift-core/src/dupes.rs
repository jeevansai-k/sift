//! Duplicates: size → partial BLAKE3 → full BLAKE3. Hard links to the same
//! inode are one file, not duplicates. Removal re-hashes the file *and* an
//! unselected copy right before moving anything, and refuses to remove the
//! last copy of a group.

use crate::cancel::CancelToken;
use crate::ctx::Ctx;
use crate::ops::OpLog;
use crate::removal::{Event, Remover, Summary};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const PARTIAL: u64 = 16 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct DupeFile {
    pub path: PathBuf,
    pub mtime: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DupeGroup {
    pub id: String,
    pub size: u64,
    pub hash: String,
    pub files: Vec<DupeFile>,
    /// Bytes reclaimable by keeping one copy.
    pub wasted: u64,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct DupeReport {
    pub roots: Vec<PathBuf>,
    pub groups: Vec<DupeGroup>,
    pub scanned_files: u64,
    pub wasted: u64,
    pub partial: bool,
}

#[derive(Default, Debug)]
pub struct Progress {
    pub files_seen: AtomicU64,
    pub hashed: AtomicU64,
    pub to_hash: AtomicU64,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub min_size: u64,
    pub include_hidden: bool,
}

fn hash_partial(path: &Path, size: u64) -> Option<String> {
    if size <= PARTIAL * 2 {
        return hash_full(path, None);
    }
    let mut f = File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; PARTIAL as usize];
    let n = f.read(&mut buf).ok()?;
    h.update(&buf[..n]);
    if size > PARTIAL * 2 {
        f.seek(SeekFrom::End(-(PARTIAL as i64))).ok()?;
        let n = f.read(&mut buf).ok()?;
        h.update(&buf[..n]);
    }
    Some(h.finalize().to_hex().to_string())
}

pub fn hash_full(path: &Path, cancel: Option<&CancelToken>) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if cancel.map(|c| c.is_cancelled()).unwrap_or(false) {
            return None;
        }
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().to_hex().to_string())
}

pub fn find(ctx: &Ctx, roots: &[PathBuf], opts: &Options, progress: &Progress, cancel: &CancelToken) -> DupeReport {
    let guard = ctx.guard();
    let skip_dirs: Vec<PathBuf> = [ctx.paths.home_trash(), ctx.paths.cache.clone()].to_vec();
    // 1. Walk, grouping by size; one entry per inode.
    let mut by_size: HashMap<u64, Vec<(PathBuf, i64)>> = HashMap::new();
    let mut inodes: HashSet<(u64, u64)> = HashSet::new();
    let mut report = DupeReport { roots: roots.to_vec(), ..Default::default() };
    for root in roots {
        let Ok(rm) = fs::symlink_metadata(root) else { continue };
        if !rm.is_dir() {
            continue;
        }
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            if cancel.is_cancelled() {
                report.partial = true;
                break;
            }
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name();
                let hidden = name.to_string_lossy().starts_with('.');
                if hidden && !opts.include_hidden {
                    continue;
                }
                let Ok(m) = e.metadata() else { continue };
                let path = e.path();
                if m.is_dir() {
                    if m.dev() == rm.dev() && !skip_dirs.iter().any(|s| path.starts_with(s)) {
                        stack.push(path);
                    }
                } else if m.is_file() && m.len() >= opts.min_size.max(1) {
                    progress.files_seen.fetch_add(1, Ordering::Relaxed);
                    if inodes.insert((m.dev(), m.ino())) && guard.check_lexical(&path, roots).is_ok() {
                        by_size.entry(m.len()).or_default().push((path, m.mtime()));
                    }
                }
            }
        }
    }
    report.scanned_files = progress.files_seen.load(Ordering::Relaxed);
    let candidates: Vec<(u64, Vec<(PathBuf, i64)>)> = by_size.into_iter().filter(|(_, v)| v.len() > 1).collect();
    progress.to_hash.store(candidates.iter().map(|(_, v)| v.len() as u64).sum(), Ordering::Relaxed);

    // 2. Partial hash, 3. full hash.
    let mut groups = Vec::new();
    for (size, files) in candidates {
        if cancel.is_cancelled() {
            report.partial = true;
            break;
        }
        let mut by_partial: HashMap<String, Vec<(PathBuf, i64)>> = HashMap::new();
        for (p, mt) in files {
            if let Some(h) = hash_partial(&p, size) {
                by_partial.entry(h).or_default().push((p, mt));
            }
            progress.hashed.fetch_add(1, Ordering::Relaxed);
        }
        for (_, same) in by_partial.into_iter().filter(|(_, v)| v.len() > 1) {
            let mut by_full: HashMap<String, Vec<(PathBuf, i64)>> = HashMap::new();
            if size <= PARTIAL * 2 {
                // The partial hash already covered the whole file.
                let h = hash_partial(&same[0].0, size).unwrap_or_default();
                by_full.insert(h, same);
            } else {
                for (p, mt) in same {
                    if let Some(h) = hash_full(&p, Some(cancel)) {
                        by_full.entry(h).or_default().push((p, mt));
                    }
                }
            }
            for (hash, mut v) in by_full.into_iter().filter(|(_, v)| v.len() > 1) {
                v.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
                let n = v.len() as u64;
                groups.push(DupeGroup {
                    id: hash.chars().take(16).collect(),
                    size,
                    hash,
                    wasted: size * (n - 1),
                    files: v.into_iter().map(|(path, mtime)| DupeFile { path, mtime }).collect(),
                });
            }
        }
    }
    groups.sort_by(|a, b| b.wasted.cmp(&a.wasted));
    report.wasted = groups.iter().map(|g| g.wasted).sum();
    report.groups = groups;
    report
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    Oldest,
    Newest,
    ShortestPath,
}

/// The copies to select so exactly one remains per group.
pub fn select_extras(group: &DupeGroup, keep: Keep) -> Vec<PathBuf> {
    let keep_idx = match keep {
        Keep::Oldest => group.files.iter().enumerate().min_by_key(|(_, f)| f.mtime).map(|(i, _)| i),
        Keep::Newest => group.files.iter().enumerate().max_by_key(|(_, f)| f.mtime).map(|(i, _)| i),
        Keep::ShortestPath => group
            .files
            .iter()
            .enumerate()
            .min_by_key(|(_, f)| (f.path.as_os_str().len(), f.path.clone()))
            .map(|(i, _)| i),
    };
    group.files.iter().enumerate().filter(|(i, _)| Some(*i) != keep_idx).map(|(_, f)| f.path.clone()).collect()
}

/// Remove selected copies, verifying each still matches and that an
/// unselected matching copy survives.
pub fn remove(
    ctx: &Ctx,
    roots: &[PathBuf],
    groups: &[DupeGroup],
    selected: &HashSet<PathBuf>,
    permanent: bool,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(&Event),
) -> Summary {
    let guard = ctx.guard();
    let trash = ctx.trash();
    let mut log = OpLog::begin(&ctx.paths, "duplicates", false, &ctx.source);
    let mut rm = Remover::new(&guard, &trash, permanent, false, &mut log);
    let total: usize = groups.iter().map(|g| g.files.iter().filter(|f| selected.contains(&f.path)).count()).sum();
    let mut done = 0;
    for g in groups {
        let chosen: Vec<&DupeFile> = g.files.iter().filter(|f| selected.contains(&f.path)).collect();
        if chosen.is_empty() {
            continue;
        }
        let keepers: Vec<&DupeFile> = g.files.iter().filter(|f| !selected.contains(&f.path)).collect();
        // A surviving original must exist and still match.
        let survivor = keepers.iter().find(|k| {
            fs::metadata(&k.path).map(|m| m.len() == g.size).unwrap_or(false) && hash_full(&k.path, Some(cancel)).as_deref() == Some(g.hash.as_str())
        });
        for f in chosen {
            if cancel.is_cancelled() {
                break;
            }
            sink(&Event::Progress { done, total, current: f.path.display().to_string() });
            done += 1;
            if survivor.is_none() {
                rm.refuse(&f.path, "no unselected copy of this file remains — keep at least one", sink);
                continue;
            }
            let same = fs::metadata(&f.path).map(|m| m.len() == g.size).unwrap_or(false)
                && hash_full(&f.path, Some(cancel)).as_deref() == Some(g.hash.as_str());
            if !same {
                rm.refuse(&f.path, crate::safety::Refusal::Changed.reason(), sink);
                continue;
            }
            rm.remove(&f.path, roots, false, Some(g.size), sink);
        }
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
    fn finds_and_safely_removes_duplicates() {
        let d = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(d.path()).unwrap();
        let docs = home.join("Docs");
        fs::create_dir_all(docs.join("a")).unwrap();
        let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(docs.join("a/one.bin"), &big).unwrap();
        fs::write(docs.join("two.bin"), &big).unwrap();
        fs::write(docs.join("three.bin"), &big).unwrap();
        let mut other = big.clone();
        other[50_000] ^= 1; // same size, same head/tail, different middle
        fs::write(docs.join("near.bin"), &other).unwrap();
        fs::hard_link(docs.join("two.bin"), docs.join("two-link.bin")).unwrap();
        fs::write(docs.join("s1.txt"), b"tiny same").unwrap();
        fs::write(docs.join("s2.txt"), b"tiny same").unwrap();

        let ctx = Ctx::with(Paths::for_home(home.clone(), true), Settings::default(), Arc::new(FakeRunner::default()));
        let roots = vec![docs.clone()];
        let r = find(&ctx, &roots, &Options { min_size: 1, include_hidden: false }, &Progress::default(), &CancelToken::new());
        assert_eq!(r.groups.len(), 2);
        let g = r.groups.iter().find(|g| g.size == 100_000).unwrap();
        assert_eq!(g.files.len(), 3, "hard link counted once, near-duplicate excluded");
        assert_eq!(g.wasted, 200_000);

        // Selecting every copy is refused for all of them.
        let all: HashSet<PathBuf> = g.files.iter().map(|f| f.path.clone()).collect();
        let s = remove(&ctx, &roots, std::slice::from_ref(g), &all, false, &CancelToken::new(), &mut |_| {});
        assert_eq!(s.refused, 3);
        assert!(g.files.iter().all(|f| f.path.exists()));

        let extras: HashSet<PathBuf> = select_extras(g, Keep::ShortestPath).into_iter().collect();
        assert_eq!(extras.len(), 2);
        let s = remove(&ctx, &roots, std::slice::from_ref(g), &extras, false, &CancelToken::new(), &mut |_| {});
        assert_eq!(s.removed, 2);
        assert_eq!(g.files.iter().filter(|f| f.path.exists()).count(), 1);
    }
}
