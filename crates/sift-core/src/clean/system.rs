//! System-level cleanups that need admin rights. Each becomes a
//! [`RootCommand`] built from constants (and validated tokens read from the
//! system), run in one confirmed `pkexec` batch.

use crate::ctx::Ctx;
use crate::format::parse_size;
use crate::privileged::{valid_token, RootCommand};
use crate::runner;
use serde::Serialize;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[derive(Clone, Debug, Serialize)]
pub struct SystemTask {
    pub id: String,
    pub label: String,
    pub bytes: u64,
    /// `bytes` is an upper bound (e.g. journal: vacuum keeps recent logs).
    pub approx: bool,
    pub detail: String,
    pub default_on: bool,
    pub command: RootCommand,
}

fn sum_files(dir: &Path, filter: impl Fn(&str, &fs::Metadata) -> bool, recursive: bool) -> (u64, u64) {
    let mut bytes = 0;
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                if recursive {
                    stack.push(e.path());
                }
            } else if m.is_file() && filter(&e.file_name().to_string_lossy(), &m) {
                bytes += m.blocks() * 512;
                n += 1;
            }
        }
    }
    (bytes, n)
}

/// `Archived and active journals take up 459.7M in the file system.`
pub fn parse_journal_usage(text: &str) -> Option<u64> {
    let i = text.find("take up ")?;
    let rest = &text[i + 8..];
    let token = rest.split_whitespace().next()?;
    // journalctl uses binary prefixes without the "i".
    let (num, unit) = token.split_at(token.find(|c: char| c.is_alphabetic()).unwrap_or(token.len()));
    let v: f64 = num.parse().ok()?;
    let mult = match unit {
        "B" | "" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0f64.powi(3),
        "T" => 1024.0f64.powi(4),
        _ => return parse_size(token),
    };
    Some((v * mult) as u64)
}

/// Disabled snap revisions from `snap list --all`: (name, revision).
pub fn parse_disabled_snaps(text: &str) -> Vec<(String, String)> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 4 || !f.last()?.split(',').any(|n| n == "disabled") {
                return None;
            }
            let (name, rev) = (f[0].to_string(), f[2].to_string());
            (valid_token(&name) && rev.bytes().all(|b| b.is_ascii_digit())).then_some((name, rev))
        })
        .collect()
}

pub fn probe(ctx: &Ctx) -> Vec<SystemTask> {
    let mut tasks = Vec::new();
    let r = ctx.runner();

    if crate::paths::which("apt-get").is_some() {
        let (b, n) = sum_files(Path::new("/var/cache/apt/archives"), |name, _| name.ends_with(".deb"), true);
        if b > 0 {
            tasks.push(SystemTask {
                id: "apt-cache".into(),
                label: "APT package cache".into(),
                bytes: b,
                approx: false,
                detail: format!("{n} downloaded .deb packages, already installed"),
                default_on: true,
                command: RootCommand::new("apt-cache", "APT package cache", "apt-get clean", "apt-get clean"),
            });
        }
    }

    if crate::paths::which("journalctl").is_some() {
        if let Ok(out) = runner::run(r, "journalctl", &["--disk-usage"], 10) {
            if let Some(b) = parse_journal_usage(&format!("{}{}", out.stdout, out.stderr)) {
                let days = ctx.settings.journal_keep_days;
                let cmd = format!("journalctl --vacuum-time={days}d");
                tasks.push(SystemTask {
                    id: "journal".into(),
                    label: "System journal".into(),
                    bytes: b,
                    approx: true,
                    detail: format!("Keeps the last {days} days of logs"),
                    default_on: true,
                    command: RootCommand::new("journal", "System journal", &cmd, &cmd),
                });
            }
        }
    }

    if crate::paths::which("snap").is_some() {
        if let Ok(out) = runner::run(r, "snap", &["list", "--all"], 20) {
            let revs = parse_disabled_snaps(&out.stdout);
            if !revs.is_empty() {
                let mut bytes = 0;
                for (name, rev) in &revs {
                    if let Ok(m) = fs::metadata(format!("/var/lib/snapd/snaps/{name}_{rev}.snap")) {
                        bytes += m.blocks() * 512;
                    }
                }
                let script =
                    revs.iter().map(|(n, rv)| format!("snap remove {n} --revision={rv}")).collect::<Vec<_>>().join("; ");
                let list = revs.iter().map(|(n, rv)| format!("{n} r{rv}")).collect::<Vec<_>>().join(", ");
                tasks.push(SystemTask {
                    id: "snap-revisions".into(),
                    label: "Old Snap revisions".into(),
                    bytes,
                    approx: false,
                    detail: format!("Disabled revisions kept for rollback: {list}"),
                    default_on: true,
                    command: RootCommand::new("snap-revisions", "Old Snap revisions", &script, &script),
                });
            }
        }
    }

    let keep = ctx.settings.crash_keep_days;
    let cutoff = crate::now_unix() - keep as i64 * 86_400;
    let (b, n) = sum_files(Path::new("/var/crash"), |name, m| name.ends_with(".crash") && m.mtime() < cutoff, false);
    if n > 0 {
        let cmd = format!("find /var/crash -maxdepth 1 -type f -name '*.crash' -mtime +{keep} -delete");
        tasks.push(SystemTask {
            id: "crash-reports".into(),
            label: "System crash reports".into(),
            bytes: b,
            approx: false,
            detail: format!("{n} reports older than {keep} days"),
            default_on: true,
            command: RootCommand::new("crash-reports", "System crash reports", &cmd, &cmd),
        });
    }

    let (b, n) = sum_files(Path::new("/var/lib/systemd/coredump"), |_, m| m.mtime() < cutoff, false);
    if n > 0 {
        let cmd = format!("find /var/lib/systemd/coredump -maxdepth 1 -type f -mtime +{keep} -delete");
        tasks.push(SystemTask {
            id: "coredumps".into(),
            label: "Core dumps".into(),
            bytes: b,
            approx: false,
            detail: format!("{n} crash memory dumps older than {keep} days"),
            default_on: true,
            command: RootCommand::new("coredumps", "Core dumps", &cmd, &cmd),
        });
    }

    let rotated = |name: &str, _: &fs::Metadata| {
        name.ends_with(".gz") || name.ends_with(".old") || name.rsplit('.').next().map(|e| e.chars().all(|c| c.is_ascii_digit()) && !e.is_empty()).unwrap_or(false) && name.contains('.')
    };
    let (b, n) = sum_files(Path::new("/var/log"), rotated, true);
    if n > 0 {
        let cmd = "find /var/log -type f \\( -name '*.gz' -o -name '*.old' -o -name '*.[0-9]' \\) -delete";
        tasks.push(SystemTask {
            id: "rotated-logs".into(),
            label: "Rotated system logs".into(),
            bytes: b,
            approx: true,
            detail: format!("{n} compressed or rotated log files (some folders need admin to measure)"),
            default_on: true,
            command: RootCommand::new("rotated-logs", "Rotated system logs", cmd, cmd),
        });
    }

    tasks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_usage() {
        assert_eq!(
            parse_journal_usage("Archived and active journals take up 459.7M in the file system.\n"),
            Some((459.7 * 1024.0 * 1024.0) as u64)
        );
        assert_eq!(parse_journal_usage("take up 2.0G in"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_journal_usage("nothing"), None);
    }

    #[test]
    fn disabled_snaps() {
        let text = "Name    Version   Rev    Tracking       Publisher   Notes\n\
core20  20240416  2318   latest/stable  canonical** base,disabled\n\
core20  20240705  2866   latest/stable  canonical** base\n\
firefox 130.0     4848   latest/stable  mozilla**   disabled\n\
evil;rm 1         1      latest/stable  x           disabled\n";
        let v = parse_disabled_snaps(text);
        assert_eq!(v, vec![("core20".into(), "2318".into()), ("firefox".into(), "4848".into())]);
    }
}
