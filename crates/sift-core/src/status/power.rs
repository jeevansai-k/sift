//! Batteries and AC from /sys/class/power_supply.

use super::Battery;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Read a power_supply directory into key → value.
pub fn read_supply(dir: &Path) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Ok(v) = fs::read_to_string(e.path()) {
                    m.insert(e.file_name().to_string_lossy().into_owned(), v.trim().to_string());
                }
            }
        }
    }
    m
}

/// Interpret one battery's attributes.
pub fn parse_battery(name: &str, a: &HashMap<String, String>) -> Battery {
    let num = |k: &str| a.get(k).and_then(|v| v.parse::<f64>().ok());
    let (now, full, design) = match (num("energy_now"), num("energy_full"), num("energy_full_design")) {
        (Some(n), Some(f), d) => (Some(n), Some(f), d),
        _ => (num("charge_now"), num("charge_full"), num("charge_full_design")),
    };
    let percent = num("capacity").or_else(|| match (now, full) {
        (Some(n), Some(f)) if f > 0.0 => Some(n / f * 100.0),
        _ => None,
    });
    let voltage = num("voltage_now");
    let power_w = num("power_now").map(|p| p / 1e6).or_else(|| match (num("current_now"), voltage) {
        (Some(i), Some(v)) => Some(i * v / 1e12),
        _ => None,
    });
    let uses_charge = a.contains_key("charge_now") && !a.contains_key("energy_now");
    let status = a.get("status").cloned().unwrap_or_else(|| "Unknown".into());
    let time_left_secs = match (now, full, num("power_now"), num("current_now")) {
        _ if status == "Full" => None,
        (Some(n), Some(f), p, i) => {
            let rate = if uses_charge { i } else { p.or(i) };
            rate.filter(|r| *r > 0.0).map(|r| {
                let remaining = if status == "Charging" { (f - n).max(0.0) } else { n };
                (remaining / r * 3600.0) as u64
            })
        }
        _ => None,
    };
    Battery {
        name: name.to_string(),
        percent: percent.unwrap_or(0.0).clamp(0.0, 100.0),
        status,
        health_pct: match (full, design) {
            (Some(f), Some(d)) if d > 0.0 => Some((f / d * 100.0).min(100.0)),
            _ => None,
        },
        cycles: a.get("cycle_count").and_then(|v| v.parse().ok()).filter(|c: &u32| *c > 0),
        time_left_secs: time_left_secs.filter(|s| *s < 48 * 3600),
        // Idle batteries report a few milliwatts of noise.
        power_w: power_w.filter(|w| *w >= 0.1),
    }
}

/// (batteries, AC online?)
pub fn read() -> (Vec<Battery>, Option<bool>) {
    let mut bats = Vec::new();
    let mut ac: Option<bool> = None;
    let Ok(rd) = fs::read_dir("/sys/class/power_supply") else { return (bats, ac) };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let a = read_supply(&e.path());
        match a.get("type").map(|s| s.as_str()) {
            Some("Battery") if a.get("scope").map(|s| s.as_str()) != Some("Device") => {
                bats.push(parse_battery(&e.file_name().to_string_lossy(), &a));
            }
            Some("Mains") | Some("USB") => {
                if a.get("online").map(|v| v == "1").unwrap_or(false) {
                    ac = Some(true);
                } else if ac.is_none() {
                    ac = Some(false);
                }
            }
            _ => {}
        }
    }
    (bats, ac)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn energy_battery_discharging() {
        let a = attrs(&[
            ("capacity", "52"),
            ("status", "Discharging"),
            ("energy_now", "26000000"),
            ("energy_full", "50000000"),
            ("energy_full_design", "55000000"),
            ("power_now", "13000000"),
            ("cycle_count", "123"),
        ]);
        let b = parse_battery("BAT0", &a);
        assert_eq!(b.percent, 52.0);
        assert!((b.health_pct.unwrap() - 90.909).abs() < 0.01);
        assert_eq!(b.cycles, Some(123));
        assert_eq!(b.time_left_secs, Some(7200));
        assert_eq!(b.power_w, Some(13.0));
    }

    #[test]
    fn charge_battery_charging() {
        let a = attrs(&[
            ("status", "Charging"),
            ("charge_now", "2000000"),
            ("charge_full", "4000000"),
            ("charge_full_design", "4000000"),
            ("current_now", "1000000"),
            ("voltage_now", "12000000"),
        ]);
        let b = parse_battery("BAT1", &a);
        assert_eq!(b.percent, 50.0);
        assert_eq!(b.time_left_secs, Some(7200));
        assert_eq!(b.power_w, Some(12.0));
        assert_eq!(b.cycles, None);
    }
}
