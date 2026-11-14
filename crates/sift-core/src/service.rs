//! The background history sampler as a systemd *user* service
//! (`~/.config/systemd/user/sift-sampler.service`). Nothing runs as root;
//! it is only installed/enabled when you switch it on.

use crate::envelope::EngineError;
use crate::paths::Paths;
use crate::runner::{self, Runner};
use std::fs;
use std::path::{Path, PathBuf};

pub const UNIT: &str = "sift-sampler.service";

pub fn unit_path(paths: &Paths) -> PathBuf {
    paths.config.join("systemd/user").join(UNIT)
}

pub fn unit_text(sift_bin: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=Sift system history sampler\n\
         Documentation=man:sift(1)\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} sampler\n\
         Restart=on-failure\n\
         RestartSec=30\n\
         Nice=10\n\
         IOSchedulingClass=idle\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        sift_bin.display()
    )
}

/// The `sift` CLI: next to this executable, else on PATH.
pub fn find_sift_binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("sift");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    crate::paths::which("sift")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    pub installed: bool,
    pub enabled: bool,
    pub active: bool,
}

pub fn state(paths: &Paths, r: &dyn Runner) -> State {
    let q = |verb: &str| {
        runner::run(r, "systemctl", &["--user", verb, UNIT], 5).map(|o| o.success()).unwrap_or(false)
    };
    State { installed: unit_path(paths).exists(), enabled: q("is-enabled"), active: q("is-active") }
}

pub fn enable(paths: &Paths, r: &dyn Runner) -> Result<(), EngineError> {
    if paths.sandboxed {
        return Err(EngineError::new("refused", "not touching systemd from a SIFT_HOME sandbox"));
    }
    let bin = find_sift_binary().ok_or_else(|| EngineError::new("missing", "the `sift` command wasn't found — run `make install` first"))?;
    let unit = unit_path(paths);
    if let Some(d) = unit.parent() {
        fs::create_dir_all(d)?;
    }
    fs::write(&unit, unit_text(&bin))?;
    let reload = runner::run(r, "systemctl", &["--user", "daemon-reload"], 15)?;
    if !reload.success() {
        return Err(EngineError::new("systemd", reload.brief_error()));
    }
    let en = runner::run(r, "systemctl", &["--user", "enable", "--now", UNIT], 20)?;
    if !en.success() {
        return Err(EngineError::new("systemd", en.brief_error()));
    }
    Ok(())
}

pub fn disable(paths: &Paths, r: &dyn Runner) -> Result<(), EngineError> {
    if paths.sandboxed {
        return Err(EngineError::new("refused", "not touching systemd from a SIFT_HOME sandbox"));
    }
    let out = runner::run(r, "systemctl", &["--user", "disable", "--now", UNIT], 20)?;
    if !out.success() && unit_path(paths).exists() {
        return Err(EngineError::new("systemd", out.brief_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_runs_sampler() {
        let t = unit_text(Path::new("/home/u/.local/bin/sift"));
        assert!(t.contains("ExecStart=/home/u/.local/bin/sift sampler"));
        assert!(t.contains("WantedBy=default.target"));
    }

    #[test]
    fn sandbox_never_touches_systemd() {
        let p = Paths::for_home(PathBuf::from("/tmp/nowhere"), true);
        let r = crate::runner::FakeRunner::default();
        assert!(enable(&p, &r).is_err());
        assert!(r.calls.lock().unwrap().is_empty());
    }
}
