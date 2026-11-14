//! Where things live: the user's home, XDG directories, and Sift's own files.
//!
//! `SIFT_HOME` overrides the home directory *and* everything derived from it
//! (XDG dirs included), so tests and demos can point the whole engine at a
//! throwaway sandbox and never touch the real home.

use std::env;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub cache: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    /// True when `SIFT_HOME` redirected everything into a sandbox.
    pub sandboxed: bool,
}

impl Paths {
    /// Resolve paths from the environment.
    pub fn detect() -> Paths {
        if let Some(h) = env::var_os("SIFT_HOME").filter(|v| !v.is_empty()) {
            return Paths::for_home(PathBuf::from(h), true);
        }
        let home = env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let xdg = |var: &str, fallback: &str| -> PathBuf {
            env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        Paths {
            cache: xdg("XDG_CACHE_HOME", ".cache"),
            config: xdg("XDG_CONFIG_HOME", ".config"),
            data: xdg("XDG_DATA_HOME", ".local/share"),
            state: xdg("XDG_STATE_HOME", ".local/state"),
            home,
            sandboxed: false,
        }
    }

    /// Paths for an explicit home, XDG dirs at their defaults beneath it.
    pub fn for_home(home: PathBuf, sandboxed: bool) -> Paths {
        Paths {
            cache: home.join(".cache"),
            config: home.join(".config"),
            data: home.join(".local/share"),
            state: home.join(".local/state"),
            home,
            sandboxed,
        }
    }

    pub fn sift_config_dir(&self) -> PathBuf {
        self.config.join("sift")
    }

    pub fn sift_state_dir(&self) -> PathBuf {
        self.state.join("sift")
    }

    pub fn sift_cache_dir(&self) -> PathBuf {
        self.cache.join("sift")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.sift_config_dir().join("settings.json")
    }

    pub fn db_file(&self) -> PathBuf {
        self.sift_state_dir().join("sift.db")
    }

    pub fn sampler_lock(&self) -> PathBuf {
        self.sift_state_dir().join("sampler.lock")
    }

    /// The home Trash directory (freedesktop.org spec).
    pub fn home_trash(&self) -> PathBuf {
        self.data.join("Trash")
    }

    /// Expand a leading `~` or `~/` against this home.
    pub fn expand(&self, p: &str) -> PathBuf {
        if p == "~" {
            self.home.clone()
        } else if let Some(rest) = p.strip_prefix("~/") {
            self.home.join(rest)
        } else {
            PathBuf::from(p)
        }
    }

    /// Path relative to home if it lives there.
    pub fn rel_home<'a>(&self, p: &'a Path) -> Option<&'a Path> {
        p.strip_prefix(&self.home).ok()
    }
}

/// Find an executable by name on PATH plus the usual system bin dirs
/// (desktop sessions often start apps with a minimal PATH).
pub fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return is_executable(&p).then_some(p);
    }
    let mut dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in ["/usr/local/sbin", "/usr/local/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    dirs.into_iter()
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_derives_everything_from_home() {
        let p = Paths::for_home(PathBuf::from("/tmp/x"), true);
        assert_eq!(p.cache, PathBuf::from("/tmp/x/.cache"));
        assert_eq!(p.home_trash(), PathBuf::from("/tmp/x/.local/share/Trash"));
        assert_eq!(p.db_file(), PathBuf::from("/tmp/x/.local/state/sift/sift.db"));
        assert_eq!(p.expand("~/Downloads"), PathBuf::from("/tmp/x/Downloads"));
        assert_eq!(p.expand("/opt"), PathBuf::from("/opt"));
    }
}
