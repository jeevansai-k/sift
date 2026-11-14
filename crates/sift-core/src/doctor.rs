//! Doctor: a pass over things that commonly go wrong on a Linux desktop.
//! Read-only; each check says what it found and what to do about it.

use crate::ctx::Ctx;
use crate::runner;
use crate::status::Snapshot;
use serde::Serialize;
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Fail,
    Warn,
    Info,
    Ok,
}

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub id: String,
    pub title: String,
    pub level: Level,
    pub summary: String,
    pub hint: Option<String>,
    /// A Sift page that helps (`clean`, `apps`, `optimize`, `settings`).
    pub action: Option<String>,
}

fn check(id: &str, title: &str, level: Level, summary: impl Into<String>) -> Check {
    Check { id: id.into(), title: title.into(), level, summary: summary.into(), hint: None, action: None }
}

impl Check {
    fn hint(mut self, h: &str) -> Self {
        self.hint = Some(h.into());
        self
    }
    fn action(mut self, a: &str) -> Self {
        self.action = Some(a.into());
        self
    }
}

/// systemd escapes unit names (`app-hidpi\x2ddaemon@autostart.service`).
pub fn unescape_unit(name: &str) -> String {
    let b = name.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1] == b'x' {
            if let Ok(v) = u8::from_str_radix(&name[i + 2..i + 4], 16) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Count of non-empty lines.
fn lines(text: &str) -> usize {
    text.lines().filter(|l| !l.trim().is_empty()).count()
}

/// `vmlinuz-6.9.3-76060903-generic` → comparable version tuple.
pub fn kernel_version_key(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

/// Newest installed kernel version under /boot, if newer than `running`.
pub fn newer_kernel(boot_entries: &[String], running: &str) -> Option<String> {
    let run_key = kernel_version_key(running);
    boot_entries
        .iter()
        .filter_map(|n| n.strip_prefix("vmlinuz-"))
        .map(|v| (kernel_version_key(v), v.to_string()))
        .filter(|(k, _)| *k > run_key)
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, v)| v)
}

/// Is `/` on an encrypted (dm-crypt) device? From `lsblk -J`.
pub fn root_encrypted(lsblk_json: &str) -> Option<bool> {
    let v: serde_json::Value = serde_json::from_str(lsblk_json).ok()?;
    fn walk(node: &serde_json::Value, crypt_above: bool) -> Option<bool> {
        let ty = node.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let crypt = crypt_above || ty == "crypt";
        let mounts: Vec<&str> = match node.get("mountpoints") {
            Some(serde_json::Value::Array(a)) => a.iter().filter_map(|m| m.as_str()).collect(),
            _ => node.get("mountpoint").and_then(|m| m.as_str()).into_iter().collect(),
        };
        if mounts.contains(&"/") {
            return Some(crypt);
        }
        for c in node.get("children").and_then(|c| c.as_array()).into_iter().flatten() {
            if let Some(r) = walk(c, crypt) {
                return Some(r);
            }
        }
        None
    }
    v.get("blockdevices")?.as_array()?.iter().find_map(|d| walk(d, false))
}

pub fn run(ctx: &Ctx, snap: Option<&Snapshot>) -> Vec<Check> {
    let r = ctx.runner();
    let mut out = Vec::new();

    // Disk headroom
    if let Some(s) = snap {
        for d in s.disks.iter().filter(|d| !d.removable) {
            let free_pct = 100.0 - d.used_pct;
            let level = if free_pct < 5.0 { Level::Fail } else if free_pct < 15.0 { Level::Warn } else { Level::Ok };
            let mut c = check(
                &format!("disk:{}", d.mount),
                &format!("Free space on {}", d.mount),
                level,
                format!("{} free of {} ({:.0}% used)", crate::format::bytes(d.free), crate::format::bytes(d.total), d.used_pct),
            );
            if level != Level::Ok {
                c = c.hint("Clean caches, purge old build folders, or look for big folders in Analyze").action("clean");
            }
            out.push(c);
        }

        // Memory pressure
        let m = &s.memory;
        let psi = m.psi_some60.or(m.psi_some10).unwrap_or(0.0);
        let level = if psi >= 20.0 { Level::Fail } else if psi >= 5.0 || m.used_pct >= 92.0 { Level::Warn } else { Level::Ok };
        let mut c = check(
            "memory",
            "Memory pressure",
            level,
            format!("{:.0}% in use, pressure {} (stall {psi:.1}%)", m.used_pct, m.pressure),
        );
        if level != Level::Ok {
            c = c.hint("Close heavy apps or browser tabs; the Monitor's process list shows the biggest users");
        }
        out.push(c);
        if m.swap_total > 0 {
            let pct = m.swap_used as f64 / m.swap_total as f64 * 100.0;
            if pct > 80.0 {
                out.push(check("swap", "Swap", Level::Warn, format!("{pct:.0}% of swap in use")).hint("Heavy swapping slows everything down; more RAM or fewer open apps helps"));
            }
        }

        // Thermals
        if let Some(t) = s.thermal.cpu_temp {
            let level = if t >= 95.0 { Level::Fail } else if t >= 85.0 { Level::Warn } else { Level::Ok };
            let mut c = check("thermal", "CPU temperature", level, format!("{t:.0} °C"));
            if level != Level::Ok {
                c = c.hint("Check for runaway processes and blocked air vents");
            }
            out.push(c);
        }

        // Battery health
        if let Some(b) = s.batteries.first() {
            if let Some(h) = b.health_pct {
                let level = if h < 60.0 { Level::Fail } else if h < 80.0 { Level::Warn } else { Level::Ok };
                let cycles = b.cycles.map(|c| format!(", {c} cycles")).unwrap_or_default();
                out.push(check("battery", "Battery health", level, format!("{h:.0}% of design capacity{cycles}")));
            }
        }

        // Zombies
        let _ = s;
        let zombies = crate::status::procs::read_all().iter().filter(|p| p.state == 'Z').count();
        if zombies > 5 {
            out.push(check("zombies", "Zombie processes", Level::Warn, format!("{zombies} defunct processes")).hint("Usually a buggy parent app; restarting it clears them"));
        }
    }

    // Failed services
    if crate::paths::which("systemctl").is_some() {
        let sys = runner::run(r, "systemctl", &["--failed", "--no-legend", "--plain"], 10).map(|o| o.stdout).unwrap_or_default();
        let user = runner::run(r, "systemctl", &["--user", "--failed", "--no-legend", "--plain"], 10).map(|o| o.stdout).unwrap_or_default();
        let names: Vec<String> = sys
            .lines()
            .chain(user.lines())
            .filter_map(|l| l.split_whitespace().next())
            .map(unescape_unit)
            .collect();
        if names.is_empty() {
            out.push(check("services", "Services", Level::Ok, "No failed services"));
        } else {
            out.push(
                check("services", "Services", Level::Warn, format!("{} failed: {}", names.len(), names.join(", ")))
                    .hint("Inspect with `systemctl status <name>`; Optimize can reset failed user services")
                    .action("optimize"),
            );
        }
    }

    // Reboot required / newer kernel
    let kernel = crate::status::system::kernel();
    let boot: Vec<String> = fs::read_dir("/boot")
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    if Path::new("/run/reboot-required").exists() || Path::new("/var/run/reboot-required").exists() {
        out.push(check("reboot", "Restart required", Level::Warn, "Installed updates need a restart to take effect").hint("Restart when convenient"));
    } else if let Some(v) = newer_kernel(&boot, &kernel) {
        out.push(check("reboot", "Restart required", Level::Warn, format!("Kernel {v} is installed; running {kernel}")).hint("Restart to use the newer kernel"));
    } else {
        out.push(check("reboot", "Restart", Level::Ok, format!("Running the newest installed kernel ({kernel})")));
    }

    // Updates (simulation needs no root)
    if crate::paths::which("apt-get").is_some() {
        if let Ok(o) = runner::run(r, "apt-get", &["-s", "-o", "Debug::NoLocking=1", "upgrade"], 60) {
            let inst: Vec<&str> = o.stdout.lines().filter(|l| l.starts_with("Inst ")).collect();
            let security = inst.iter().filter(|l| l.contains("-security")).count();
            let level = if security > 0 { Level::Warn } else if inst.is_empty() { Level::Ok } else { Level::Info };
            let summary = if inst.is_empty() {
                "Packages are up to date (as of the last package list refresh)".to_string()
            } else {
                format!("{} package updates available ({security} security)", inst.len())
            };
            let mut c = check("updates", "Software updates", level, summary);
            if !inst.is_empty() {
                c = c.hint("Install them from your software center or with `sudo apt upgrade`");
            }
            out.push(c);
        }
    }

    // Firewall
    let ufw_enabled = fs::read_to_string("/etc/ufw/ufw.conf").ok().map(|t| t.lines().any(|l| l.trim() == "ENABLED=yes"));
    let firewalld = crate::paths::which("firewall-cmd").is_some()
        && runner::run(r, "systemctl", &["is-active", "firewalld"], 5).map(|o| o.stdout.trim() == "active").unwrap_or(false);
    match (ufw_enabled, firewalld) {
        (_, true) | (Some(true), _) => out.push(check("firewall", "Firewall", Level::Ok, "Enabled")),
        (Some(false), false) => out.push(
            check("firewall", "Firewall", Level::Warn, "ufw is installed but turned off")
                .hint("Turn it on with `sudo ufw enable` (allows outgoing, blocks unsolicited incoming)"),
        ),
        (None, false) => out.push(check("firewall", "Firewall", Level::Info, "No ufw or firewalld found")),
    }

    // Secure Boot
    let sb = fs::read("/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c").ok();
    match (Path::new("/sys/firmware/efi").exists(), sb) {
        (false, _) => out.push(check("secureboot", "Secure Boot", Level::Info, "Legacy BIOS boot (no UEFI)")),
        (true, Some(b)) if b.last() == Some(&1) => out.push(check("secureboot", "Secure Boot", Level::Ok, "Enabled")),
        (true, _) => out.push(check("secureboot", "Secure Boot", Level::Info, "Disabled — common when using NVIDIA or custom kernels")),
    }

    // Disk encryption
    if let Ok(o) = runner::run(r, "lsblk", &["-J", "-o", "NAME,TYPE,MOUNTPOINTS"], 10) {
        match root_encrypted(&o.stdout) {
            Some(true) => out.push(check("encryption", "Disk encryption", Level::Ok, "The system drive is encrypted (LUKS)")),
            Some(false) => {
                let laptop = snap.map(|s| !s.batteries.is_empty()).unwrap_or(false);
                out.push(
                    check("encryption", "Disk encryption", if laptop { Level::Warn } else { Level::Info }, "The system drive is not encrypted")
                        .hint("Anyone with the drive can read your files; encryption is set up when (re)installing"),
                )
            }
            None => {}
        }
    }

    // Clock sync
    if let Ok(o) = runner::run(r, "timedatectl", &["show", "-p", "NTPSynchronized", "--value"], 5) {
        match o.stdout.trim() {
            "yes" => out.push(check("time", "Clock", Level::Ok, "Synchronized with network time")),
            "no" => out.push(check("time", "Clock", Level::Warn, "Not synchronized with network time").hint("Enable it with `timedatectl set-ntp true`")),
            _ => {}
        }
    }

    // Errors logged this boot
    if let Ok(o) = runner::run(r, "journalctl", &["-b", "-p", "3", "-q", "--no-pager", "-o", "cat"], 15) {
        let n = lines(&o.stdout);
        let level = if n > 200 { Level::Warn } else { Level::Ok };
        out.push(check("journal", "Errors this boot", level, format!("{n} error messages in the system log since startup")).hint("See them with `journalctl -b -p 3`"));
    }

    // Trash and cache size
    let trash = ctx.trash().size();
    if trash.bytes > 1_000_000_000 {
        out.push(
            check("trash", "Trash", Level::Warn, format!("{} is waiting in the Trash", crate::format::bytes(trash.bytes)))
                .hint("Space in the Trash isn't freed until you empty it")
                .action("clean"),
        );
    }

    // History recording
    if !crate::sampler::SamplerLock::is_held(&ctx.paths) {
        out.push(
            check("history", "Background history", Level::Info, "History is only recorded while Sift is open")
                .hint("Turn on background recording in Settings")
                .action("settings"),
        );
    }

    out.sort_by(|a, b| a.level.cmp(&b.level));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernels() {
        let boot = vec!["vmlinuz-6.9.3-76060903-generic".to_string(), "vmlinuz-6.12.10-76061203-generic".into(), "initrd.img".into()];
        assert_eq!(newer_kernel(&boot, "6.9.3-76060903-generic").as_deref(), Some("6.12.10-76061203-generic"));
        assert_eq!(newer_kernel(&boot, "6.12.10-76061203-generic"), None);
    }

    #[test]
    fn encryption() {
        let enc = r#"{"blockdevices":[{"name":"nvme0n1","type":"disk","mountpoints":[null],"children":[{"name":"nvme0n1p3","type":"part","mountpoints":[null],"children":[{"name":"cryptdata","type":"crypt","mountpoints":[null],"children":[{"name":"data-root","type":"lvm","mountpoints":["/"]}]}]}]}]}"#;
        assert_eq!(root_encrypted(enc), Some(true));
        let plain = r#"{"blockdevices":[{"name":"nvme0n1","type":"disk","mountpoints":[null],"children":[{"name":"nvme0n1p9","type":"part","mountpoints":["/"]}]}]}"#;
        assert_eq!(root_encrypted(plain), Some(false));
    }
}
