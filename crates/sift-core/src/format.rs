//! Human-readable formatting shared by the CLI and the GUI.

/// Decimal (SI) byte formatting, as GNOME and COSMIC file managers show it.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1000 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

/// Bytes split into (number, unit) for big-number displays ("12" "GB").
pub fn bytes_parts(n: u64) -> (String, String) {
    let s = bytes(n);
    match s.split_once(' ') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (s, String::new()),
    }
}

/// Transfer rate in bytes per second.
pub fn rate(bps: f64) -> String {
    if !bps.is_finite() || bps <= 0.0 {
        return "0 B/s".into();
    }
    format!("{}/s", bytes(bps as u64))
}

/// Compact duration: "3d 4h", "2h 5m", "45s".
pub fn duration(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}

/// "just now", "5 min ago", "3 days ago" relative to `now`.
pub fn ago(ts: i64, now: i64) -> String {
    let delta = (now - ts).max(0) as u64;
    match delta {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", delta / 60),
        3600..=86_399 => {
            let h = delta / 3600;
            if h == 1 { "1 hour ago".into() } else { format!("{h} hours ago") }
        }
        _ => {
            let d = delta / 86_400;
            if d == 1 { "yesterday".into() } else if d < 60 { format!("{d} days ago") } else { format!("{} months ago", d / 30) }
        }
    }
}

/// Local date-time string for a unix timestamp.
pub fn datetime(ts: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(ts, 0) {
        chrono::LocalResult::Single(t) => t.format("%Y-%m-%d %H:%M").to_string(),
        _ => ts.to_string(),
    }
}

/// Shorten a path under `home` to `~/…` for display.
pub fn tilde(path: &std::path::Path, home: &std::path::Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Parse sizes like "1.2 GB", "512 kB", "3,4 MB", "120 bytes" (flatpak / snap output).
pub fn parse_size(text: &str) -> Option<u64> {
    let t = text.trim().replace('\u{a0}', " ").replace(',', ".");
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let v: f64 = num.trim().parse().ok()?;
    let unit = unit.trim().to_ascii_lowercase();
    let mult: f64 = match unit.as_str() {
        "" | "b" | "bytes" | "byte" => 1.0,
        "kb" | "k" => 1e3,
        "kib" => 1024.0,
        "mb" | "m" => 1e6,
        "mib" => 1024.0 * 1024.0,
        "gb" | "g" => 1e9,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tb" | "t" => 1e12,
        "tib" => 1024f64.powi(4),
        _ => return None,
    };
    Some((v * mult).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_000), "1.00 KB");
        assert_eq!(bytes(4_870_000_000), "4.87 GB");
        assert_eq!(bytes(12_300_000_000), "12.3 GB");
        assert_eq!(bytes(460_400_000_000), "460 GB");
        assert_eq!(bytes_parts(12_000_000_000), ("12.0".into(), "GB".into()));
    }

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("1.2 GB"), Some(1_200_000_000));
        assert_eq!(parse_size("512 kB"), Some(512_000));
        assert_eq!(parse_size("3,5 MB"), Some(3_500_000));
        assert_eq!(parse_size("120 bytes"), Some(120));
        assert_eq!(parse_size("1 GiB"), Some(1_073_741_824));
        assert_eq!(parse_size("n/a"), None);
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(100, 100), "just now");
        assert_eq!(ago(0, 600), "10 min ago");
        assert_eq!(ago(0, 7200), "2 hours ago");
        assert_eq!(ago(0, 86_400 * 3), "3 days ago");
        assert_eq!(duration(93_600), "1d 2h");
    }
}
