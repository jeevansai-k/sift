//! CPU: /proc/stat jiffies, /proc/loadavg, /proc/cpuinfo, cpufreq.

use std::collections::HashSet;
use std::fs;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Times {
    pub busy: u64,
    pub idle: u64,
}

impl Times {
    pub fn total(&self) -> u64 {
        self.busy + self.idle
    }
}

fn parse_line(rest: &str) -> Times {
    let v: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    // user nice system idle iowait irq softirq steal guest guest_nice
    let get = |i: usize| v.get(i).copied().unwrap_or(0);
    let idle = get(3) + get(4);
    let busy = get(0) + get(1) + get(2) + get(5) + get(6) + get(7);
    Times { busy, idle }
}

/// (aggregate, per-core) from /proc/stat.
pub fn parse_stat(text: &str) -> (Times, Vec<Times>) {
    let mut total = Times::default();
    let mut cores = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("cpu ") {
            total = parse_line(rest);
        } else if let Some(rest) = line.strip_prefix("cpu") {
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                let after = rest.trim_start_matches(|c: char| c.is_ascii_digit());
                cores.push(parse_line(after));
            }
        }
    }
    (total, cores)
}

/// Busy percentage between two readings.
pub fn usage(prev: &Times, cur: &Times) -> f64 {
    let dt = cur.total().saturating_sub(prev.total());
    if dt == 0 {
        return 0.0;
    }
    let db = cur.busy.saturating_sub(prev.busy);
    (db as f64 / dt as f64 * 100.0).clamp(0.0, 100.0)
}

pub fn parse_loadavg(text: &str) -> (f64, f64, f64) {
    let v: Vec<f64> = text.split_whitespace().take(3).filter_map(|x| x.parse().ok()).collect();
    (
        v.first().copied().unwrap_or(0.0),
        v.get(1).copied().unwrap_or(0.0),
        v.get(2).copied().unwrap_or(0.0),
    )
}

/// (model name, physical core count) from /proc/cpuinfo.
pub fn parse_cpuinfo(text: &str) -> (String, u32) {
    let mut model = String::new();
    let mut cores: HashSet<(String, String)> = HashSet::new();
    let mut phys = String::new();
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "model name" | "Model" | "Hardware" if model.is_empty() => model = v.to_string(),
            "physical id" => phys = v.to_string(),
            "core id" => {
                cores.insert((phys.clone(), v.to_string()));
            }
            _ => {}
        }
    }
    let model = model
        .replace("(R)", "")
        .replace("(TM)", "")
        .replace(" CPU", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (model, cores.len() as u32)
}

/// Mean current frequency across cores (cpufreq, kHz → MHz).
pub fn average_freq_mhz() -> Option<f64> {
    let rd = fs::read_dir("/sys/devices/system/cpu").ok()?;
    let mut sum = 0.0;
    let mut n = 0.0;
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("cpu") || !name[3..].chars().all(|c| c.is_ascii_digit()) || name.len() == 3 {
            continue;
        }
        if let Ok(s) = fs::read_to_string(e.path().join("cpufreq/scaling_cur_freq")) {
            if let Ok(khz) = s.trim().parse::<f64>() {
                sum += khz / 1000.0;
                n += 1.0;
            }
        }
    }
    (n > 0.0).then(|| sum / n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 50 0 25 400 25 0 0 0 0 0\ncpu1 50 0 25 400 25 0 0 0 0 0\nintr 1 2 3\nctxt 99\n";
    const STAT2: &str = "cpu  200 0 100 850 50 0 0 0 0 0\ncpu0 150 0 50 400 25 0 0 0 0 0\ncpu1 50 0 50 475 25 0 0 0 0 0\n";

    #[test]
    fn stat_usage() {
        let (a, ca) = parse_stat(STAT);
        let (b, cb) = parse_stat(STAT2);
        assert_eq!(ca.len(), 2);
        assert_eq!(a, Times { busy: 150, idle: 850 });
        // busy +150, idle +50 → 75%
        assert!((usage(&a, &b) - 75.0).abs() < 1e-9);
        assert!((usage(&ca[0], &cb[0]) - 100.0).abs() < 1e-9);
        assert!((usage(&ca[1], &cb[1]) - 25.0).abs() < 1e-9);
        assert_eq!(usage(&a, &a), 0.0);
    }

    #[test]
    fn cpuinfo_and_load() {
        let info = "processor\t: 0\nmodel name\t: 13th Gen Intel(R) Core(TM) i7-13650HX\nphysical id\t: 0\ncore id\t\t: 0\n\nprocessor\t: 1\nmodel name\t: 13th Gen Intel(R) Core(TM) i7-13650HX\nphysical id\t: 0\ncore id\t\t: 0\n\nprocessor\t: 2\nphysical id\t: 0\ncore id\t\t: 4\n";
        let (m, p) = parse_cpuinfo(info);
        assert_eq!(m, "13th Gen Intel Core i7-13650HX");
        assert_eq!(p, 2);
        assert_eq!(parse_loadavg("3.48 4.33 4.27 2/1500 999\n"), (3.48, 4.33, 4.27));
    }
}
