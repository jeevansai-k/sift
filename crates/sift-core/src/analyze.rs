//! Analyze: a size tree of a folder for the treemap. Read-only. Stays on one
//! filesystem, never follows symlinks, counts hard links once, and keeps
//! memory bounded by folding each folder's small files into one node.

use crate::cancel::CancelToken;
use crate::fsutil::alloc_bytes;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Files kept individually per folder; the rest fold into "N other files".
const KEEP_FILES: usize = 48;

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub name: String,
    pub bytes: u64,
    pub files: u64,
    pub is_dir: bool,
    /// Aggregate of small files (not a real path).
    pub is_rest: bool,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub mtime: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tree {
    pub root_path: PathBuf,
    pub nodes: Vec<Node>,
    pub errors: u64,
    pub partial: bool,
}

impl Tree {
    pub fn root(&self) -> usize {
        0
    }

    /// Real path of a node (`None` for aggregate nodes).
    pub fn path_of(&self, idx: usize) -> Option<PathBuf> {
        let mut parts = Vec::new();
        let mut cur = Some(idx);
        while let Some(i) = cur {
            let n = &self.nodes[i];
            if n.is_rest {
                return None;
            }
            if n.parent.is_some() {
                parts.push(n.name.clone());
            }
            cur = n.parent;
        }
        let mut p = self.root_path.clone();
        for part in parts.iter().rev() {
            p.push(part);
        }
        Some(p)
    }

    /// Children sorted largest first.
    pub fn sorted_children(&self, idx: usize) -> Vec<usize> {
        let mut c = self.nodes[idx].children.clone();
        c.sort_by(|a, b| self.nodes[*b].bytes.cmp(&self.nodes[*a].bytes));
        c
    }

    /// Ancestors from the root down to `idx` (inclusive).
    pub fn chain(&self, idx: usize) -> Vec<usize> {
        let mut v = vec![idx];
        let mut cur = self.nodes[idx].parent;
        while let Some(p) = cur {
            v.push(p);
            cur = self.nodes[p].parent;
        }
        v.reverse();
        v
    }
}

#[derive(Default, Debug)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub current: Mutex<String>,
}

impl Progress {
    pub fn snapshot(&self) -> (u64, u64, u64, String) {
        (
            self.files.load(Ordering::Relaxed),
            self.dirs.load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
            self.current.lock().map(|s| s.clone()).unwrap_or_default(),
        )
    }
}

const SKIP_ABS: &[&str] = &["/proc", "/sys", "/dev", "/run", "/tmp/.X11-unix"];

struct Walker<'a> {
    nodes: Vec<Node>,
    seen: HashSet<(u64, u64)>,
    dev: u64,
    errors: u64,
    progress: &'a Progress,
    cancel: &'a CancelToken,
    partial: bool,
}

impl Walker<'_> {
    /// Iterative post-order walk building nodes; returns the root index.
    fn walk(&mut self, root: &Path) -> usize {
        let meta = fs::symlink_metadata(root);
        let mtime = meta.as_ref().map(|m| m.mtime()).unwrap_or(0);
        self.nodes.push(Node {
            name: root.display().to_string(),
            bytes: 0,
            files: 0,
            is_dir: true,
            is_rest: false,
            parent: None,
            children: vec![],
            mtime,
        });
        // (dir path, node index, entered?)
        let mut stack: Vec<(PathBuf, usize, bool)> = vec![(root.to_path_buf(), 0, false)];
        while let Some((dir, idx, entered)) = stack.pop() {
            if entered {
                // Post-order: sum children, fold small files.
                self.finish_dir(idx);
                continue;
            }
            if self.cancel.is_cancelled() {
                self.partial = true;
                self.finish_dir(idx);
                continue;
            }
            stack.push((dir.clone(), idx, true));
            if let Ok(mut cur) = self.progress.current.lock() {
                *cur = dir.display().to_string();
            }
            self.progress.dirs.fetch_add(1, Ordering::Relaxed);
            let rd = match fs::read_dir(&dir) {
                Ok(r) => r,
                Err(_) => {
                    self.errors += 1;
                    continue;
                }
            };
            for e in rd.flatten() {
                let Ok(m) = e.metadata() else {
                    self.errors += 1;
                    continue;
                };
                let name = e.file_name().to_string_lossy().into_owned();
                if m.is_dir() {
                    let path = e.path();
                    if m.dev() != self.dev || SKIP_ABS.iter().any(|s| path == Path::new(s)) {
                        continue;
                    }
                    let child = self.nodes.len();
                    self.nodes.push(Node {
                        name,
                        bytes: alloc_bytes(&m),
                        files: 0,
                        is_dir: true,
                        is_rest: false,
                        parent: Some(idx),
                        children: vec![],
                        mtime: m.mtime(),
                    });
                    self.nodes[idx].children.push(child);
                    stack.push((path, child, false));
                } else {
                    if m.nlink() > 1 && !self.seen.insert((m.dev(), m.ino())) {
                        continue;
                    }
                    let b = alloc_bytes(&m);
                    self.progress.files.fetch_add(1, Ordering::Relaxed);
                    self.progress.bytes.fetch_add(b, Ordering::Relaxed);
                    let child = self.nodes.len();
                    self.nodes.push(Node {
                        name,
                        bytes: b,
                        files: 1,
                        is_dir: false,
                        is_rest: false,
                        parent: Some(idx),
                        children: vec![],
                        mtime: m.mtime(),
                    });
                    self.nodes[idx].children.push(child);
                }
            }
        }
        0
    }

    fn finish_dir(&mut self, idx: usize) {
        let children = self.nodes[idx].children.clone();
        let mut bytes = self.nodes[idx].bytes;
        let mut files = 0;
        for &c in &children {
            bytes += self.nodes[c].bytes;
            files += self.nodes[c].files;
        }
        self.nodes[idx].bytes = bytes;
        self.nodes[idx].files = files;
        // Fold the smallest plain files beyond KEEP_FILES into one node.
        let mut file_children: Vec<usize> = children.iter().copied().filter(|&c| !self.nodes[c].is_dir).collect();
        if file_children.len() > KEEP_FILES {
            file_children.sort_by(|a, b| self.nodes[*b].bytes.cmp(&self.nodes[*a].bytes));
            let fold: HashSet<usize> = file_children[KEEP_FILES..].iter().copied().collect();
            let (mut fb, mut fn_) = (0u64, 0u64);
            for &c in &fold {
                fb += self.nodes[c].bytes;
                fn_ += 1;
            }
            let rest = Node {
                name: format!("{fn_} smaller files"),
                bytes: fb,
                files: fn_,
                is_dir: false,
                is_rest: true,
                parent: Some(idx),
                children: vec![],
                mtime: 0,
            };
            let rest_idx = self.nodes.len();
            self.nodes.push(rest);
            let kept: Vec<usize> = children.into_iter().filter(|c| !fold.contains(c)).chain([rest_idx]).collect();
            self.nodes[idx].children = kept;
            // Folded nodes stay in the arena but are unreachable.
        }
    }
}

/// Build the size tree for `root`.
pub fn scan(root: &Path, progress: &Progress, cancel: &CancelToken) -> Result<Tree, crate::envelope::EngineError> {
    let meta = fs::symlink_metadata(root)?;
    if !meta.is_dir() {
        return Err(crate::envelope::EngineError::new("invalid", format!("{} is not a folder", root.display())));
    }
    let mut w = Walker { nodes: Vec::new(), seen: HashSet::new(), dev: meta.dev(), errors: 0, progress, cancel, partial: false };
    w.walk(root);
    Ok(Tree { root_path: root.to_path_buf(), nodes: w.nodes, errors: w.errors, partial: w.partial })
}

/// JSON-friendly summary of one level (for the CLI): largest children first.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: Option<PathBuf>,
    pub bytes: u64,
    pub files: u64,
    pub is_dir: bool,
    pub children: Vec<Entry>,
}

pub fn entries(tree: &Tree, idx: usize, depth: usize, limit: usize) -> Vec<Entry> {
    tree.sorted_children(idx)
        .into_iter()
        .take(limit)
        .map(|c| {
            let n = &tree.nodes[c];
            Entry {
                name: n.name.clone(),
                path: tree.path_of(c),
                bytes: n.bytes,
                files: n.files,
                is_dir: n.is_dir,
                children: if depth > 1 && n.is_dir { entries(tree, c, depth - 1, limit) } else { vec![] },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_tree_with_folding() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("r");
        fs::create_dir_all(root.join("big")).unwrap();
        fs::write(root.join("big/blob"), vec![1u8; 200_000]).unwrap();
        for i in 0..60 {
            fs::write(root.join(format!("f{i}")), vec![1u8; 100 + i * 10]).unwrap();
        }
        std::os::unix::fs::symlink("/usr", root.join("link")).unwrap();
        let p = Progress::default();
        let t = scan(&root, &p, &CancelToken::new()).unwrap();
        let kids = t.sorted_children(0);
        assert_eq!(t.nodes[kids[0]].name, "big");
        assert!(kids.iter().any(|&k| t.nodes[k].is_rest));
        assert!(kids.len() <= KEEP_FILES + 3);
        let total: u64 = kids.iter().map(|&k| t.nodes[k].bytes).sum();
        assert!(t.nodes[0].bytes >= total);
        assert_eq!(t.path_of(kids[0]).unwrap(), root.join("big"));
        let rest = kids.iter().find(|&&k| t.nodes[k].is_rest).unwrap();
        assert_eq!(t.path_of(*rest), None);
        let (files, _, _, _) = p.snapshot();
        assert!(files >= 61);
        let e = entries(&t, 0, 2, 5);
        assert_eq!(e[0].children.len(), 1);
    }
}
