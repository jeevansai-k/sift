//! What is running right now: open files of your processes, process names,
//! and running Flatpak / Snap apps. Used to skip caches that are in use.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default, Debug)]
pub struct Activity {
    /// Absolute paths open by readable processes → process name.
    open: BTreeMap<PathBuf, String>,
    names: HashSet<String>,
    flatpaks: HashSet<String>,
    snaps: HashSet<String>,
}

impl Activity {
    /// Snapshot /proc (only processes we may inspect, i.e. our own).
    pub fn capture() -> Activity {
        let mut a = Activity::default();
        let Ok(rd) = fs::read_dir("/proc") else { return a };
        for e in rd.flatten() {
            let name = e.file_name();
            let Some(pid) = name.to_str().filter(|s| s.bytes().all(|b| b.is_ascii_digit())) else { continue };
            let base = Path::new("/proc").join(pid);
            let comm = fs::read_to_string(base.join("comm")).unwrap_or_default().trim().to_string();
            if !comm.is_empty() {
                a.names.insert(comm.to_lowercase());
            }
            if let Ok(exe) = fs::read_link(base.join("exe")) {
                if let Some(b) = exe.file_name() {
                    a.names.insert(b.to_string_lossy().to_lowercase());
                }
            }
            if let Ok(cg) = fs::read_to_string(base.join("cgroup")) {
                if let Some(id) = flatpak_from_cgroup(&cg) {
                    a.flatpaks.insert(id);
                }
                if let Some(s) = snap_from_cgroup(&cg) {
                    a.snaps.insert(s);
                }
            }
            if let Ok(fds) = fs::read_dir(base.join("fd")) {
                for fd in fds.flatten() {
                    if let Ok(target) = fs::read_link(fd.path()) {
                        if target.is_absolute() {
                            a.open.entry(target).or_insert_with(|| comm.clone());
                        }
                    }
                }
            }
        }
        a
    }

    /// Name of a process holding something under `dir` open, if any.
    pub fn user_of(&self, dir: &Path) -> Option<&str> {
        self.open
            .range(dir.to_path_buf()..)
            .next()
            .filter(|(p, _)| p.starts_with(dir))
            .map(|(_, n)| n.as_str())
    }

    /// First of `names` that is running (case-insensitive).
    pub fn running<'a>(&self, names: &[&'a str]) -> Option<&'a str> {
        names.iter().copied().find(|n| self.names.contains(&n.to_lowercase()))
    }

    pub fn flatpak_running(&self, app_id: &str) -> bool {
        self.flatpaks.contains(app_id)
    }

    pub fn snap_running(&self, name: &str) -> bool {
        self.snaps.contains(name)
    }

    #[cfg(test)]
    pub fn fake(open: &[(&str, &str)], names: &[&str]) -> Activity {
        Activity {
            open: open.iter().map(|(p, n)| (PathBuf::from(p), n.to_string())).collect(),
            names: names.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }
}

/// `…/app-flatpak-com.spotify.Client-12345.scope` → `com.spotify.Client`.
pub fn flatpak_from_cgroup(cg: &str) -> Option<String> {
    let i = cg.find("app-flatpak-")?;
    let rest = &cg[i + "app-flatpak-".len()..];
    let end = rest.find(".scope").unwrap_or(rest.len());
    let unit = &rest[..end];
    let dash = unit.rfind('-')?;
    let id = &unit[..dash];
    (!id.is_empty()).then(|| id.to_string())
}

/// `…/snap.firefox.firefox-<uuid>.scope` → `firefox`.
pub fn snap_from_cgroup(cg: &str) -> Option<String> {
    let i = cg.find("/snap.")?;
    let rest = &cg[i + "/snap.".len()..];
    let name = rest.split('.').next()?;
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_lookup() {
        let a = Activity::fake(
            &[("/home/u/.cache/mozilla/firefox/x/cache2/entries/AB", "firefox"), ("/home/u/.cache/pipx", "python3")],
            &["firefox"],
        );
        assert_eq!(a.user_of(Path::new("/home/u/.cache/mozilla")), Some("firefox"));
        assert_eq!(a.user_of(Path::new("/home/u/.cache/pip")), None);
        assert_eq!(a.user_of(Path::new("/home/u/.cache/pipx")), Some("python3"));
        assert_eq!(a.running(&["chrome", "firefox"]), Some("firefox"));
    }

    #[test]
    fn cgroups() {
        assert_eq!(
            flatpak_from_cgroup("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-com.spotify.Client-12345.scope\n"),
            Some("com.spotify.Client".into())
        );
        assert_eq!(
            snap_from_cgroup("0::/user.slice/user-1000.slice/user@1000.service/app.slice/snap.firefox.firefox-3a1b.scope"),
            Some("firefox".into())
        );
        assert_eq!(flatpak_from_cgroup("0::/init.scope"), None);
    }
}
