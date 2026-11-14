//! Memory: /proc/meminfo and pressure-stall information (/proc/pressure/memory).

use super::Memory;
use std::collections::HashMap;

/// meminfo values in bytes.
pub fn parse_meminfo(text: &str) -> HashMap<String, u64> {
    let mut m = HashMap::new();
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let mut parts = v.split_whitespace();
        let Some(n) = parts.next().and_then(|x| x.parse::<u64>().ok()) else { continue };
        let mult = if parts.next() == Some("kB") { 1024 } else { 1 };
        m.insert(k.trim().to_string(), n * mult);
    }
    m
}

/// (some avg10, some avg60, full avg10)
pub fn parse_psi(text: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    let field = |line: &str, key: &str| -> Option<f64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key).and_then(|v| v.parse().ok()))
    };
    let mut out = (None, None, None);
    for line in text.lines() {
        if line.starts_with("some") {
            out.0 = field(line, "avg10=");
            out.1 = field(line, "avg60=");
        } else if line.starts_with("full") {
            out.2 = field(line, "avg10=");
        }
    }
    out
}

pub fn build(meminfo: &str, psi: Option<&str>) -> Memory {
    let m = parse_meminfo(meminfo);
    let get = |k: &str| m.get(k).copied().unwrap_or(0);
    let total = get("MemTotal");
    let available = if m.contains_key("MemAvailable") {
        get("MemAvailable")
    } else {
        get("MemFree") + get("Buffers") + get("Cached")
    };
    let used = total.saturating_sub(available);
    let used_pct = if total > 0 { used as f64 / total as f64 * 100.0 } else { 0.0 };
    let swap_total = get("SwapTotal");
    let swap_used = swap_total.saturating_sub(get("SwapFree"));
    let (some10, some60, full10) = psi.map(parse_psi).unwrap_or((None, None, None));
    let pressure = pressure_label(used_pct, some10);
    Memory {
        total,
        used,
        available,
        cached: get("Cached") + get("SReclaimable"),
        used_pct,
        swap_total,
        swap_used,
        psi_some10: some10,
        psi_some60: some60,
        psi_full10: full10,
        pressure: pressure.into(),
    }
}

/// Memory pressure from PSI when the kernel provides it, else from usage.
pub fn pressure_label(used_pct: f64, psi_some10: Option<f64>) -> &'static str {
    match psi_some10 {
        Some(p) if p >= 20.0 => "critical",
        Some(p) if p >= 5.0 => "warning",
        Some(_) if used_pct >= 97.0 => "warning",
        Some(_) => "normal",
        None if used_pct >= 95.0 => "critical",
        None if used_pct >= 85.0 => "warning",
        None => "normal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo() {
        let text = "MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:    4000000 kB\nCached:          2000000 kB\nSReclaimable:     500000 kB\nSwapTotal:       2000000 kB\nSwapFree:        1500000 kB\nHugePages_Total:       0\n";
        let m = build(text, Some("some avg10=1.50 avg60=0.80 avg300=0.10 total=57550\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=100\n"));
        assert_eq!(m.total, 16_000_000 * 1024);
        assert_eq!(m.used, 12_000_000 * 1024);
        assert!((m.used_pct - 75.0).abs() < 1e-9);
        assert_eq!(m.swap_used, 500_000 * 1024);
        assert_eq!(m.cached, 2_500_000 * 1024);
        assert_eq!(m.psi_some10, Some(1.5));
        assert_eq!(m.psi_some60, Some(0.8));
        assert_eq!(m.pressure, "normal");
        assert_eq!(pressure_label(50.0, Some(25.0)), "critical");
        assert_eq!(pressure_label(90.0, None), "warning");
    }
}
