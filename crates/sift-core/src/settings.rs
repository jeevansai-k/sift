//! User settings, stored as JSON in `~/.config/sift/settings.json`.

use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    /// Dashboard refresh while the window is open.
    pub live_refresh_secs: u32,
    /// How often a history row is written.
    pub sample_interval_secs: u32,
    pub retention_days: u32,
    /// Never clean anything at or under these paths (`~/` allowed).
    pub whitelist: Vec<String>,
    /// Where Purge looks for projects. Empty = your home folder.
    pub purge_roots: Vec<String>,
    /// Projects touched this recently are listed but not pre-selected.
    pub purge_recent_days: u32,
    pub installer_dirs: Vec<String>,
    /// Installers younger than this are listed but not pre-selected.
    pub installer_min_age_days: u32,
    pub dupes_min_size: u64,
    /// `journalctl --vacuum-time` keeps this many days of logs.
    pub journal_keep_days: u32,
    /// System crash reports older than this are offered for removal.
    pub crash_keep_days: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Theme::System,
            live_refresh_secs: 2,
            sample_interval_secs: 30,
            retention_days: 30,
            whitelist: Vec::new(),
            purge_roots: Vec::new(),
            purge_recent_days: 7,
            installer_dirs: vec!["~/Downloads".into(), "~/Desktop".into()],
            installer_min_age_days: 7,
            dupes_min_size: 1_000_000,
            journal_keep_days: 14,
            crash_keep_days: 7,
        }
    }
}

impl Settings {
    pub fn load(paths: &Paths) -> Settings {
        fs::read_to_string(paths.settings_file())
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default()
            .clamped()
    }

    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        let dir = paths.sift_config_dir();
        fs::create_dir_all(&dir)?;
        let tmp = dir.join(".settings.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&self.clone().clamped())?)?;
        fs::rename(tmp, paths.settings_file())
    }

    /// Keep values in sane ranges whatever the file says.
    pub fn clamped(mut self) -> Settings {
        self.live_refresh_secs = self.live_refresh_secs.clamp(1, 10);
        self.sample_interval_secs = self.sample_interval_secs.clamp(5, 300);
        self.retention_days = self.retention_days.clamp(1, 365);
        self.purge_recent_days = self.purge_recent_days.min(365);
        self.installer_min_age_days = self.installer_min_age_days.min(365);
        self.journal_keep_days = self.journal_keep_days.clamp(1, 365);
        self.crash_keep_days = self.crash_keep_days.min(365);
        if self.installer_dirs.is_empty() {
            self.installer_dirs = Settings::default().installer_dirs;
        }
        self
    }

    pub fn whitelist_paths(&self, paths: &Paths) -> Vec<PathBuf> {
        self.whitelist.iter().map(|s| paths.expand(s)).filter(|p| p.is_absolute()).collect()
    }

    pub fn purge_root_paths(&self, paths: &Paths) -> Vec<PathBuf> {
        let v: Vec<PathBuf> = self.purge_roots.iter().map(|s| paths.expand(s)).filter(|p| p.is_absolute()).collect();
        if v.is_empty() { vec![paths.home.clone()] } else { v }
    }

    pub fn installer_dir_paths(&self, paths: &Paths) -> Vec<PathBuf> {
        self.installer_dirs.iter().map(|s| paths.expand(s)).filter(|p| p.is_absolute()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_clamp() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::for_home(d.path().to_path_buf(), true);
        assert_eq!(Settings::load(&p), Settings::default());
        let s = Settings { sample_interval_secs: 1, retention_days: 9999, ..Default::default() };
        s.save(&p).unwrap();
        let back = Settings::load(&p);
        assert_eq!(back.sample_interval_secs, 5);
        assert_eq!(back.retention_days, 365);
        // Unknown / partial files fall back field-by-field.
        std::fs::write(p.settings_file(), r#"{"retention_days": 7, "bogus": 1}"#).unwrap();
        assert_eq!(Settings::load(&p).retention_days, 7);
        assert_eq!(Settings::load(&p).live_refresh_secs, 2);
    }
}
