//! 0–100 health score with a one-line reason (Burrow's "74 · Fair · Disk
//! Almost Full"). Start at 100 and subtract weighted penalties; the biggest
//! penalty names the reason.

use super::{Health, HealthFactor, Snapshot};

fn ramp(value: f64, start: f64, per_unit: f64, cap: f64) -> f64 {
    if value <= start { 0.0 } else { ((value - start) * per_unit).min(cap) }
}

pub fn score(s: &Snapshot) -> Health {
    let mut factors: Vec<HealthFactor> = Vec::new();
    let mut add = |name: &str, penalty: f64, message: &str| {
        if penalty > 0.05 {
            factors.push(HealthFactor { name: name.into(), penalty, message: message.into() });
        }
    };

    add("cpu", ramp(s.cpu.usage, 60.0, 0.4, 15.0), "CPU Busy");

    let mem = ramp(s.memory.used_pct, 80.0, 1.0, 20.0);
    let psi = s.memory.psi_some10.map(|p| ramp(p, 5.0, 0.8, 20.0)).unwrap_or(0.0);
    add("memory", mem.max(psi), if psi > mem { "High Memory Pressure" } else { "Memory Nearly Full" });

    if s.memory.swap_total > 0 {
        let swap_pct = s.memory.swap_used as f64 / s.memory.swap_total as f64 * 100.0;
        add("swap", ramp(swap_pct, 60.0, 0.15, 6.0), "Heavy Swap Use");
    }

    if let Some(d) = s.root_disk() {
        let p = ramp(d.used_pct, 80.0, 1.6, 32.0);
        let msg = if d.used_pct >= 95.0 { "Disk Almost Full" } else { "Disk Filling Up" };
        add("disk", p, msg);
    }

    if let Some(t) = s.thermal.cpu_temp {
        add("thermal", ramp(t, 80.0, 1.0, 15.0), "Running Hot");
    }

    if let Some(h) = s.batteries.first().and_then(|b| b.health_pct) {
        add("battery", ramp(100.0 - h, 25.0, 0.3, 6.0), "Battery Worn");
    }

    let load_per_core = if s.cpu.logical > 0 { s.cpu.load5 / s.cpu.logical as f64 } else { 0.0 };
    add("load", ramp(load_per_core, 1.5, 6.0, 8.0), "System Overloaded");

    let total: f64 = factors.iter().map(|f| f.penalty).sum();
    let score = (100.0 - total).round().clamp(0.0, 100.0) as u8;
    factors.sort_by(|a, b| b.penalty.total_cmp(&a.penalty));
    let label = match score {
        90..=100 => "Excellent",
        75..=89 => "Good",
        60..=74 => "Fair",
        40..=59 => "Poor",
        _ => "Critical",
    };
    let message = match factors.first() {
        Some(f) if f.penalty >= 2.0 => f.message.clone(),
        _ => "All Systems Normal".into(),
    };
    Health { score, label: label.into(), message, factors }
}

#[cfg(test)]
mod tests {
    use super::super::{Disk, Snapshot};
    use super::*;

    #[test]
    fn healthy_machine() {
        let mut s = Snapshot::default();
        s.cpu.usage = 10.0;
        s.memory.used_pct = 40.0;
        s.disks.push(Disk { mount: "/".into(), used_pct: 50.0, ..Default::default() });
        let h = score(&s);
        assert_eq!(h.score, 100);
        assert_eq!(h.label, "Excellent");
        assert_eq!(h.message, "All Systems Normal");
    }

    #[test]
    fn full_disk_dominates() {
        let mut s = Snapshot::default();
        s.cpu.usage = 30.0;
        s.memory.used_pct = 80.0;
        s.disks.push(Disk { mount: "/".into(), used_pct: 95.0, ..Default::default() });
        let h = score(&s);
        assert_eq!(h.score, 76);
        assert_eq!(h.message, "Disk Almost Full");
        assert_eq!(h.label, "Good");
    }

    #[test]
    fn pressure_beats_usage() {
        let mut s = Snapshot::default();
        s.memory.used_pct = 85.0;
        s.memory.psi_some10 = Some(25.0);
        let h = score(&s);
        assert_eq!(h.message, "High Memory Pressure");
        assert_eq!(h.score, 84);
    }
}
