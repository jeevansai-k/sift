//! Temperatures and fans from hwmon (and thermal zones as a fallback).

use super::{Fan, Sensor, Thermal};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct Chip {
    pub name: String,
    pub temps: Vec<(String, f64)>,
    pub fans: Vec<(String, u32)>,
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub fn read_chips(root: &Path) -> Vec<Chip> {
    let mut chips = Vec::new();
    let Ok(rd) = fs::read_dir(root) else { return chips };
    let mut dirs: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let name = read_trim(&dir.join("name")).unwrap_or_default();
        let mut chip = Chip { name, ..Default::default() };
        let Ok(files) = fs::read_dir(&dir) else { continue };
        let mut files: Vec<String> = files.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        files.sort();
        for f in &files {
            if let Some(idx) = f.strip_prefix("temp").and_then(|r| r.strip_suffix("_input")) {
                if let Some(v) = read_trim(&dir.join(f)).and_then(|s| s.parse::<f64>().ok()) {
                    let label = read_trim(&dir.join(format!("temp{idx}_label"))).unwrap_or_else(|| format!("temp{idx}"));
                    let c = v / 1000.0;
                    if (-40.0..150.0).contains(&c) && c != 0.0 {
                        chip.temps.push((label, c));
                    }
                }
            } else if let Some(idx) = f.strip_prefix("fan").and_then(|r| r.strip_suffix("_input")) {
                if let Some(v) = read_trim(&dir.join(f)).and_then(|s| s.parse::<u32>().ok()) {
                    let label = read_trim(&dir.join(format!("fan{idx}_label"))).unwrap_or_default();
                    chip.fans.push((label, v));
                }
            }
        }
        chips.push(chip);
    }
    chips
}

/// Pick CPU / GPU temperatures and one consistent set of fans.
pub fn interpret(chips: &[Chip]) -> Thermal {
    let mut t = Thermal::default();
    for c in chips {
        let pick = |labels: &[&str]| -> Option<f64> {
            labels
                .iter()
                .find_map(|l| c.temps.iter().find(|(lab, _)| lab == l).map(|(_, v)| *v))
                .or_else(|| c.temps.first().map(|(_, v)| *v))
        };
        match c.name.as_str() {
            "coretemp" => t.cpu_temp = t.cpu_temp.or(pick(&["Package id 0"])),
            "k10temp" | "zenpower" => t.cpu_temp = t.cpu_temp.or(pick(&["Tctl", "Tdie"])),
            "cpu_thermal" | "soc_thermal" => t.cpu_temp = t.cpu_temp.or(pick(&[])),
            "amdgpu" | "nouveau" | "radeon" => t.gpu_temp = t.gpu_temp.or(pick(&["edge"])),
            _ => {}
        }
        for (label, v) in &c.temps {
            t.sensors.push(Sensor { label: format!("{} {}", c.name, label), temp_c: *v });
        }
    }
    // Several drivers often expose the same fans (dell_smm, dell_ddv, …):
    // take the chip with the most fans, preferring labelled ones.
    if let Some(best) = chips
        .iter()
        .filter(|c| !c.fans.is_empty())
        .max_by_key(|c| (c.fans.len(), c.fans.iter().filter(|(l, _)| !l.is_empty()).count()))
    {
        t.fans = best
            .fans
            .iter()
            .enumerate()
            .map(|(i, (l, r))| Fan { label: if l.is_empty() { format!("Fan {}", i + 1) } else { l.clone() }, rpm: *r })
            .collect();
    }
    t
}

pub fn read() -> Thermal {
    let chips = read_chips(Path::new("/sys/class/hwmon"));
    let mut t = interpret(&chips);
    if t.cpu_temp.is_none() {
        t.cpu_temp = thermal_zone_cpu();
    }
    t
}

fn thermal_zone_cpu() -> Option<f64> {
    let rd = fs::read_dir("/sys/class/thermal").ok()?;
    for e in rd.flatten() {
        let ty = read_trim(&e.path().join("type")).unwrap_or_default();
        if ty == "x86_pkg_temp" || ty.contains("cpu") {
            if let Some(v) = read_trim(&e.path().join("temp")).and_then(|s| s.parse::<f64>().ok()) {
                return Some(v / 1000.0);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_package_temp_and_best_fans() {
        let chips = vec![
            Chip { name: "nvme".into(), temps: vec![("Composite".into(), 37.0)], fans: vec![] },
            Chip {
                name: "coretemp".into(),
                temps: vec![("Core 0".into(), 46.0), ("Package id 0".into(), 51.0)],
                fans: vec![],
            },
            Chip { name: "dell_smm".into(), temps: vec![], fans: vec![("".into(), 4979), ("".into(), 5004)] },
            Chip {
                name: "alienware_wmi".into(),
                temps: vec![],
                fans: vec![("CPU Fan".into(), 5004), ("GPU Fan".into(), 5004)],
            },
        ];
        let t = interpret(&chips);
        assert_eq!(t.cpu_temp, Some(51.0));
        assert_eq!(t.fans.len(), 2);
        assert_eq!(t.fans[0].label, "CPU Fan");
        assert_eq!(t.sensors.len(), 3);
    }
}
