//! The deletion rails. Every path Sift removes goes through [`Guard::check`]
//! first, whatever produced it (scan, reviewed plan, uninstall leftovers,
//! duplicate selection). A refusal is reported with a reason; it is never
//! silently "fixed" by picking a different path.

use crate::paths::Paths;
use serde::Serialize;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    NotAbsolute,
    NotNormalized,
    Missing,
    Symlink,
    SymlinkInPath,
    OutsideScope,
    Protected,
    SystemPath,
    Whitelisted,
    NotOwned,
    NotATarget,
    InUse,
    Changed,
}

impl Refusal {
    pub fn reason(&self) -> &'static str {
        match self {
            Refusal::NotAbsolute => "path is not absolute",
            Refusal::NotNormalized => "path contains . or .. components",
            Refusal::Missing => "path no longer exists",
            Refusal::Symlink => "path is a symbolic link",
            Refusal::SymlinkInPath => "a parent folder is a symbolic link",
            Refusal::OutsideScope => "path is outside the folders this operation may touch",
            Refusal::Protected => "path is protected",
            Refusal::SystemPath => "system path",
            Refusal::Whitelisted => "path is on your never-clean list",
            Refusal::NotOwned => "path is not owned by you",
            Refusal::NotATarget => "path is not something this scan could have found",
            Refusal::InUse => "in use by a running program",
            Refusal::Changed => "file changed since it was reviewed",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

/// Folders that are never removed themselves (their contents may be).
const HOME_EXACT: &[&str] = &[
    "",
    ".cache",
    ".config",
    ".local",
    ".local/bin",
    ".local/lib",
    ".local/share",
    ".local/state",
    ".local/share/applications",
    ".local/share/icons",
    ".local/share/Trash",
    ".local/share/Trash/files",
    ".local/share/Trash/info",
    "Desktop",
    "Documents",
    "Downloads",
    "Music",
    "Pictures",
    "Videos",
    "Public",
    "Templates",
    "Applications",
    "snap",
    ".var",
    ".var/app",
    ".npm",
    ".cargo",
    ".rustup",
    "go",
];

/// Subtrees under home that hold credentials, profiles, or package stores.
const HOME_TREES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".pki",
    ".password-store",
    ".local/share/keyrings",
    ".local/share/kwalletd",
    ".mozilla",
    ".thunderbird",
    ".config/google-chrome",
    ".config/chromium",
    ".config/BraveSoftware",
    ".config/vivaldi",
    ".config/microsoft-edge",
    ".config/1Password",
    ".config/Bitwarden",
    ".config/keepassxc",
    ".config/sift",
    ".local/state/sift",
    ".local/share/flatpak",
    ".local/share/containers",
    ".local/share/docker",
    ".rustup/toolchains",
    ".bitcoin",
    ".electrum",
    ".ethereum",
];

/// System trees — off limits to user-level removal entirely.
const SYSTEM_TREES: &[&str] = &[
    "/bin", "/boot", "/dev", "/efi", "/etc", "/lib", "/lib32", "/lib64", "/libx32", "/lost+found",
    "/opt", "/proc", "/root", "/run", "/sbin", "/snap", "/srv", "/sys", "/usr", "/var",
];

#[derive(Clone, Debug)]
pub struct Guard {
    home: PathBuf,
    exact: Vec<PathBuf>,
    trees: Vec<PathBuf>,
    whitelist: Vec<PathBuf>,
    uid: u32,
}

impl Guard {
    pub fn new(paths: &Paths, whitelist: &[PathBuf]) -> Guard {
        let home = paths.home.clone();
        let mut exact: Vec<PathBuf> = HOME_EXACT.iter().map(|r| join_rel(&home, r)).collect();
        exact.push(PathBuf::from("/"));
        exact.push(PathBuf::from("/home"));
        exact.push(PathBuf::from("/media"));
        exact.push(PathBuf::from("/mnt"));
        exact.push(PathBuf::from("/tmp"));
        exact.push(paths.cache.clone());
        exact.push(paths.config.clone());
        exact.push(paths.data.clone());
        exact.push(paths.state.clone());
        let trees = HOME_TREES.iter().map(|r| home.join(r)).collect();
        Guard {
            home,
            exact,
            trees,
            whitelist: whitelist.iter().filter(|p| p.is_absolute()).cloned().collect(),
            uid: crate::current_uid(),
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Lexical rules only (no filesystem access).
    pub fn check_lexical(&self, path: &Path, scopes: &[PathBuf]) -> Result<(), Refusal> {
        if !path.is_absolute() {
            return Err(Refusal::NotAbsolute);
        }
        if !is_normalized(path) {
            return Err(Refusal::NotNormalized);
        }
        if !scopes.iter().any(|s| path != s.as_path() && path.starts_with(s)) {
            return Err(Refusal::OutsideScope);
        }
        if self.exact.iter().any(|p| p == path) {
            return Err(Refusal::Protected);
        }
        if self.trees.iter().any(|t| path.starts_with(t)) {
            return Err(Refusal::Protected);
        }
        if !path.starts_with(&self.home) && SYSTEM_TREES.iter().any(|t| path.starts_with(t)) {
            return Err(Refusal::SystemPath);
        }
        // A whitelisted path is protected, and so is every folder containing one.
        if self.whitelist.iter().any(|w| path.starts_with(w) || w.starts_with(path)) {
            return Err(Refusal::Whitelisted);
        }
        Ok(())
    }

    /// Full check before removing `path` itself.
    pub fn check(&self, path: &Path, scopes: &[PathBuf]) -> Result<(), Refusal> {
        self.check_lexical(path, scopes)?;
        self.check_parent_chain(path)?;
        let meta = std::fs::symlink_metadata(path).map_err(|_| Refusal::Missing)?;
        if meta.file_type().is_symlink() {
            return Err(Refusal::Symlink);
        }
        if meta.uid() != self.uid {
            return Err(Refusal::NotOwned);
        }
        Ok(())
    }

    /// Like [`check`](Self::check) but a symlink leaf is allowed (removing the
    /// link itself is harmless; used for children of a reviewed cache folder).
    pub fn check_child(&self, path: &Path, scopes: &[PathBuf]) -> Result<(), Refusal> {
        self.check_lexical(path, scopes)?;
        self.check_parent_chain(path)?;
        let meta = std::fs::symlink_metadata(path).map_err(|_| Refusal::Missing)?;
        if !meta.file_type().is_symlink() && meta.uid() != self.uid {
            return Err(Refusal::NotOwned);
        }
        Ok(())
    }

    /// No symlink anywhere above `path`: its parent must canonicalize to itself.
    pub fn check_parent_chain(&self, path: &Path) -> Result<(), Refusal> {
        let parent = path.parent().ok_or(Refusal::Protected)?;
        match std::fs::canonicalize(parent) {
            Ok(c) if c == parent => Ok(()),
            Ok(_) => Err(Refusal::SymlinkInPath),
            Err(_) => Err(Refusal::Missing),
        }
    }
}

fn join_rel(home: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() { home.to_path_buf() } else { home.join(rel) }
}

/// Absolute with only normal components: no `.`, no `..`.
pub fn is_normalized(path: &Path) -> bool {
    let mut comps = path.components();
    if comps.next() != Some(Component::RootDir) {
        return false;
    }
    comps.all(|c| matches!(c, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sandbox() -> (tempfile::TempDir, Paths, Guard) {
        let dir = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(dir.path()).unwrap();
        let paths = Paths::for_home(home.clone(), true);
        fs::create_dir_all(home.join(".cache/app/sub")).unwrap();
        fs::write(home.join(".cache/app/sub/f"), b"x").unwrap();
        let guard = Guard::new(&paths, &[home.join(".cache/keep")]);
        (dir, paths, guard)
    }

    #[test]
    fn allows_cache_children() {
        let (_d, p, g) = sandbox();
        let scopes = [p.home.clone()];
        assert_eq!(g.check(&p.home.join(".cache/app"), &scopes), Ok(()));
    }

    #[test]
    fn refuses_roots_and_protected() {
        let (_d, p, g) = sandbox();
        let scopes = [p.home.clone()];
        assert_eq!(g.check_lexical(&p.home, &[PathBuf::from("/")]), Err(Refusal::Protected));
        assert_eq!(g.check_lexical(&p.home.join(".cache"), &scopes), Err(Refusal::Protected));
        assert_eq!(g.check_lexical(&p.home.join(".ssh/id_ed25519"), &scopes), Err(Refusal::Protected));
        assert_eq!(g.check_lexical(&p.home.join("Documents"), &scopes), Err(Refusal::Protected));
        assert_eq!(g.check_lexical(Path::new("/usr/lib/x"), &[PathBuf::from("/usr")]), Err(Refusal::SystemPath));
    }

    #[test]
    fn refuses_escapes() {
        let (_d, p, g) = sandbox();
        let scopes = [p.home.join(".cache")];
        assert_eq!(g.check_lexical(Path::new("relative/x"), &scopes), Err(Refusal::NotAbsolute));
        let sneaky = PathBuf::from(format!("{}/.cache/../Documents", p.home.display()));
        assert_eq!(g.check_lexical(&sneaky, &scopes), Err(Refusal::NotNormalized));
        assert_eq!(g.check_lexical(&p.home.join("Music/x"), &scopes), Err(Refusal::OutsideScope));
        // The scope root itself is never a target.
        assert_eq!(g.check_lexical(&p.home.join(".cache"), &scopes), Err(Refusal::OutsideScope));
    }

    #[test]
    fn whitelist_protects_item_and_ancestors() {
        let (_d, p, g) = sandbox();
        let scopes = [p.home.clone()];
        assert_eq!(g.check_lexical(&p.home.join(".cache/keep"), &scopes), Err(Refusal::Whitelisted));
        assert_eq!(g.check_lexical(&p.home.join(".cache/keep/a"), &scopes), Err(Refusal::Whitelisted));
        let g2 = Guard::new(&p, &[p.home.join(".cache/app/sub/f")]);
        assert_eq!(g2.check_lexical(&p.home.join(".cache/app"), &scopes), Err(Refusal::Whitelisted));
    }

    #[test]
    fn refuses_symlinks() {
        let (_d, p, g) = sandbox();
        let scopes = [p.home.clone()];
        std::os::unix::fs::symlink(p.home.join(".cache/app"), p.home.join(".cache/link")).unwrap();
        assert_eq!(g.check(&p.home.join(".cache/link"), &scopes), Err(Refusal::Symlink));
        assert_eq!(g.check(&p.home.join(".cache/link/sub"), &scopes), Err(Refusal::SymlinkInPath));
        assert_eq!(g.check_child(&p.home.join(".cache/link"), &scopes), Ok(()));
        assert_eq!(g.check(&p.home.join(".cache/missing"), &scopes), Err(Refusal::Missing));
    }
}
