//! Processes from /proc/[pid]/stat and statm.

use super::Process;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawProc {
    pub pid: u32,
    pub ppid: u32,
    pub comm: String,
    pub state: char,
    pub ticks: u64,
    pub threads: u32,
    pub rss_pages: u64,
    pub uid: u32,
}

/// Parse `/proc/[pid]/stat`. The command name may contain spaces and
/// parentheses, so split on the *last* ')'.
pub fn parse_stat(pid: u32, text: &str) -> Option<RawProc> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    // rest[0] = state (field 3) … utime = field 14 → rest[11]
    let n = |i: usize| rest.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    Some(RawProc {
        pid,
        ppid: n(1) as u32,
        comm,
        state: rest.first().and_then(|s| s.chars().next()).unwrap_or('?'),
        ticks: n(11) + n(12),
        threads: n(17) as u32,
        rss_pages: n(21),
        uid: 0,
    })
}

pub fn read_all() -> Vec<RawProc> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir("/proc") else { return out };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(text) = fs::read_to_string(e.path().join("stat")) else { continue };
        if let Some(mut p) = parse_stat(pid, &text) {
            p.uid = e.metadata().map(|m| m.uid()).unwrap_or(0);
            out.push(p);
        }
    }
    out
}

/// Full command line (NUL-separated → spaces).
pub fn cmdline(pid: u32) -> Option<String> {
    let b = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if b.is_empty() {
        return None;
    }
    let s: String = b
        .split(|c| *c == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    Some(s)
}

/// Better display name: comm is truncated to 15 chars; argv[0]'s basename
/// usually carries the full name.
fn display_name(p: &RawProc) -> String {
    if p.comm.len() < 15 {
        return p.comm.clone();
    }
    if let Some(cmd) = cmdline(p.pid) {
        let argv0 = cmd.split(' ').next().unwrap_or("");
        let base = argv0.rsplit('/').next().unwrap_or(argv0);
        if base.starts_with(&p.comm) {
            return base.to_string();
        }
    }
    p.comm.clone()
}

pub fn build(
    raw: &[RawProc],
    prev_ticks: &HashMap<u32, u64>,
    secs: f64,
    clk_tck: f64,
    page_size: u64,
    mem_total: u64,
    users: &HashMap<u32, String>,
) -> Vec<Process> {
    raw.iter()
        .map(|p| {
            let cpu = match prev_ticks.get(&p.pid) {
                Some(prev) if p.ticks >= *prev => (p.ticks - prev) as f64 / clk_tck / secs * 100.0,
                _ => 0.0,
            };
            let mem_bytes = p.rss_pages * page_size;
            Process {
                pid: p.pid,
                ppid: p.ppid,
                name: display_name(p),
                command: String::new(),
                user: users.get(&p.uid).cloned().unwrap_or_else(|| p.uid.to_string()),
                uid: p.uid,
                cpu,
                mem_bytes,
                mem_pct: if mem_total > 0 { mem_bytes as f64 / mem_total as f64 * 100.0 } else { 0.0 },
                state: p.state.to_string(),
                threads: p.threads,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_with_parens_in_name() {
        let text = "1234 (Web Content (x)) S 1000 1234 1234 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 31 0 12345 1000000 5000 18446744073709551615 0 0 0 0 0 0 0 4096 0 0 0 0 17 3 0 0 0 0 0";
        let p = parse_stat(1234, text).unwrap();
        assert_eq!(p.comm, "Web Content (x)");
        assert_eq!(p.state, 'S');
        assert_eq!(p.ppid, 1000);
        assert_eq!(p.ticks, 300);
        assert_eq!(p.threads, 31);
        assert_eq!(p.rss_pages, 5000);
    }

    #[test]
    fn cpu_percent_is_per_core() {
        let raw = vec![RawProc { pid: 7, comm: "busy".into(), ticks: 300, rss_pages: 10, ..Default::default() }];
        let prev: HashMap<u32, u64> = [(7, 100)].into_iter().collect();
        let v = build(&raw, &prev, 1.0, 100.0, 4096, 1 << 30, &HashMap::new());
        assert!((v[0].cpu - 200.0).abs() < 1e-9); // two full cores
        assert_eq!(v[0].mem_bytes, 40960);
    }
}
