//! GPUs.
//!
//! - NVIDIA: `nvidia-smi`, only while the card is awake — polling a
//!   runtime-suspended laptop dGPU would wake it and burn battery.
//! - Intel / AMD (and other DRM drivers): busy time from DRM fdinfo
//!   (`/proc/<pid>/fdinfo/<fd>` → `drm-engine-<name>: <ns>`), the same
//!   per-client counters `intel_gpu_top`-style tools read. Needs no root:
//!   the compositor and your apps are your own processes. AMD's
//!   `gpu_busy_percent` is preferred when present.

use super::Gpu;
use crate::runner::{self, SystemRunner};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct Card {
    vendor: &'static str,
    device: PathBuf,
    slot: String,
    drives_display: bool,
}

#[derive(Default)]
pub struct GpuProbe {
    cards: Option<Vec<Card>>,
    names: HashMap<String, String>,
    nvidia_cache: Option<(Instant, Vec<Gpu>)>,
    drm: DrmUsage,
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// Does any connector of `cardN` have a connected, enabled display?
fn drives_display(card_name: &str) -> bool {
    let Ok(rd) = fs::read_dir("/sys/class/drm") else { return false };
    rd.flatten().any(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        n.starts_with(&format!("{card_name}-"))
            && read_trim(&e.path().join("status")).as_deref() == Some("connected")
            && read_trim(&e.path().join("enabled")).as_deref() != Some("disabled")
    })
}

fn discover() -> Vec<Card> {
    let mut cards = Vec::new();
    let Ok(rd) = fs::read_dir("/sys/class/drm") else { return cards };
    let mut seen = std::collections::HashSet::new();
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let device = e.path().join("device");
        let vendor = match read_trim(&device.join("vendor")).as_deref() {
            Some("0x10de") => "NVIDIA",
            Some("0x1002") => "AMD",
            Some("0x8086") => "Intel",
            _ => continue,
        };
        let slot = fs::canonicalize(&device)
            .ok()
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
            .unwrap_or_default();
        if seen.insert(slot.clone()) {
            cards.push(Card { vendor, device, slot, drives_display: drives_display(&name) });
        }
    }
    cards
}

/// `01:00.0 "VGA compatible controller" "NVIDIA Corporation" "GA107M [GeForce RTX 3050 Mobile]" …`
pub fn parse_lspci_name(line: &str) -> Option<String> {
    let quoted: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
    let device = quoted.get(2)?;
    let name = match (device.find('['), device.rfind(']')) {
        (Some(a), Some(b)) if b > a => &device[a + 1..b],
        _ => device,
    };
    Some(name.to_string())
}

/// `name, util, mem.used, mem.total, temp, power.draw, pstate` (csv,noheader,nounits)
pub fn parse_nvidia_smi(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split(',').map(|x| x.trim()).collect();
            let num = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok());
            Gpu {
                name: f.first().map(|s| s.replace("NVIDIA ", "")).unwrap_or_default(),
                vendor: "NVIDIA".into(),
                usage: num(1),
                mem_used: num(2).map(|m| (m * 1024.0 * 1024.0) as u64),
                mem_total: num(3).map(|m| (m * 1024.0 * 1024.0) as u64),
                temp_c: num(4),
                power_w: num(5),
                pstate: f.get(6).filter(|s| s.starts_with('P')).map(|s| s.to_string()),
                state: "active".into(),
                drives_display: false,
            }
        })
        .collect()
}

/// One DRM client's counters from an fdinfo file:
/// (pci slot, client id, [(engine, busy ns)]).
pub fn parse_fdinfo(text: &str) -> Option<(String, String, Vec<(String, u64)>)> {
    let mut pdev = None;
    let mut client = None;
    let mut engines = Vec::new();
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim();
        match k {
            "drm-pdev" => pdev = Some(v.to_string()),
            "drm-client-id" => client = Some(v.to_string()),
            _ => {
                if let Some(engine) = k.strip_prefix("drm-engine-") {
                    if engine.starts_with("capacity-") {
                        continue;
                    }
                    if let Some(ns) = v.strip_suffix("ns").and_then(|n| n.trim().parse::<u64>().ok()) {
                        engines.push((engine.to_string(), ns));
                    }
                }
            }
        }
    }
    Some((pdev?, client?, engines))
}

/// Per-device GPU busy % from DRM fdinfo deltas.
#[derive(Default)]
pub struct DrmUsage {
    fdinfos: Vec<PathBuf>,
    last_scan: Option<Instant>,
    prev: HashMap<(String, String, String), u64>,
    prev_at: Option<Instant>,
}

impl DrmUsage {
    /// Find every open DRM file descriptor we're allowed to inspect.
    fn rescan(&mut self) {
        self.fdinfos.clear();
        let Ok(rd) = fs::read_dir("/proc") else { return };
        for e in rd.flatten() {
            let name = e.file_name();
            if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let Ok(fds) = fs::read_dir(e.path().join("fd")) else { continue };
            for fd in fds.flatten() {
                if fs::read_link(fd.path()).map(|t| t.starts_with("/dev/dri/")).unwrap_or(false) {
                    self.fdinfos.push(e.path().join("fdinfo").join(fd.file_name()));
                }
            }
        }
        self.last_scan = Some(Instant::now());
    }

    /// Busy % per PCI slot since the previous call (empty on the first call).
    pub fn sample(&mut self) -> HashMap<String, f64> {
        if self.last_scan.map(|t| t.elapsed() > Duration::from_secs(10)).unwrap_or(true) {
            self.rescan();
        }
        let now = Instant::now();
        // Several fds can belong to one client; counters are per client.
        let mut cur: HashMap<(String, String, String), u64> = HashMap::new();
        for p in &self.fdinfos {
            let Ok(text) = fs::read_to_string(p) else { continue };
            if let Some((pdev, client, engines)) = parse_fdinfo(&text) {
                for (engine, ns) in engines {
                    let slot = cur.entry((pdev.clone(), client.clone(), engine)).or_insert(0);
                    *slot = (*slot).max(ns);
                }
            }
        }
        let mut out: HashMap<String, f64> = HashMap::new();
        if let Some(at) = self.prev_at {
            let elapsed = now.duration_since(at).as_nanos().max(1) as f64;
            // Sum per (device, engine) over clients present both times.
            let mut per_engine: HashMap<(String, String), u64> = HashMap::new();
            for (key, ns) in &cur {
                if let Some(prev) = self.prev.get(key) {
                    *per_engine.entry((key.0.clone(), key.2.clone())).or_insert(0) += ns.saturating_sub(*prev);
                }
            }
            for ((pdev, _), delta) in per_engine {
                let pct = (delta as f64 / elapsed * 100.0).clamp(0.0, 100.0);
                let e = out.entry(pdev).or_insert(0.0);
                *e = e.max(pct);
            }
            // Devices with clients but no activity still read as 0 %.
            for key in cur.keys() {
                out.entry(key.0.clone()).or_insert(0.0);
            }
        }
        self.prev = cur;
        self.prev_at = Some(now);
        out
    }
}

impl GpuProbe {
    fn name_for(&mut self, card: &Card) -> String {
        if let Some(n) = self.names.get(&card.slot) {
            return n.clone();
        }
        let short = card.slot.trim_start_matches("0000:").to_string();
        let name = runner::run(&SystemRunner, "lspci", &["-mm", "-s", &short], 3)
            .ok()
            .and_then(|o| o.stdout.lines().next().and_then(parse_lspci_name))
            .map(|n| format!("{} {}", card.vendor, n))
            .unwrap_or_else(|| format!("{} graphics", card.vendor));
        self.names.insert(card.slot.clone(), name.clone());
        name
    }

    /// Take a first DRM reading so the next `read` can report busy %.
    pub fn prime(&mut self) {
        self.drm.sample();
    }

    pub fn read(&mut self) -> Vec<Gpu> {
        let cards = self.cards.get_or_insert_with(discover).clone();
        let drm = self.drm.sample();
        let mut out = Vec::new();
        let mut nvidia_done = false;
        for card in &cards {
            match card.vendor {
                "NVIDIA" => {
                    if nvidia_done {
                        continue;
                    }
                    nvidia_done = true;
                    let status = read_trim(&card.device.join("power/runtime_status")).unwrap_or_default();
                    if status == "suspended" {
                        let name = self.name_for(card);
                        out.push(Gpu {
                            name,
                            vendor: "NVIDIA".into(),
                            state: "sleeping".into(),
                            drives_display: card.drives_display,
                            ..Default::default()
                        });
                        continue;
                    }
                    let fresh = self
                        .nvidia_cache
                        .as_ref()
                        .filter(|(t, _)| t.elapsed() < Duration::from_secs(4))
                        .map(|(_, g)| g.clone());
                    let gpus = match fresh {
                        Some(g) => g,
                        None => {
                            let g = runner::run(
                                &SystemRunner,
                                "nvidia-smi",
                                &[
                                    "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,pstate",
                                    "--format=csv,noheader,nounits",
                                ],
                                4,
                            )
                            .ok()
                            .filter(|o| o.success())
                            .map(|o| parse_nvidia_smi(&o.stdout))
                            .unwrap_or_default();
                            self.nvidia_cache = Some((Instant::now(), g.clone()));
                            g
                        }
                    };
                    if gpus.is_empty() {
                        let name = self.name_for(card);
                        out.push(Gpu {
                            name,
                            vendor: "NVIDIA".into(),
                            state: "unknown".into(),
                            drives_display: card.drives_display,
                            ..Default::default()
                        });
                    } else {
                        out.extend(gpus.into_iter().map(|mut g| {
                            g.drives_display = card.drives_display;
                            g
                        }));
                    }
                }
                "AMD" => {
                    let name = self.name_for(card);
                    let num = |f: &str| read_trim(&card.device.join(f)).and_then(|v| v.parse::<f64>().ok());
                    out.push(Gpu {
                        name,
                        vendor: "AMD".into(),
                        usage: num("gpu_busy_percent").or_else(|| drm.get(&card.slot).copied()),
                        mem_used: num("mem_info_vram_used").map(|v| v as u64),
                        mem_total: num("mem_info_vram_total").map(|v| v as u64),
                        state: "active".into(),
                        drives_display: card.drives_display,
                        ..Default::default()
                    });
                }
                _ => {
                    let name = self.name_for(card);
                    out.push(Gpu {
                        name,
                        vendor: card.vendor.into(),
                        usage: drm.get(&card.slot).copied(),
                        state: "active".into(),
                        drives_display: card.drives_display,
                        ..Default::default()
                    });
                }
            }
        }
        // The GPU driving the screen first, then discrete before integrated.
        out.sort_by_key(|g| (!g.drives_display, g.vendor == "Intel"));
        out
    }
}

/// Compact name for small labels: "RTX 3050", "UHD Graphics", "Radeon 780M".
pub fn short_name(name: &str) -> String {
    let n = name
        .replace("NVIDIA ", "")
        .replace("GeForce ", "")
        .replace("Laptop GPU", "")
        .replace("Intel ", "")
        .replace("AMD ", "");
    let words: Vec<&str> = n.split_whitespace().filter(|w| !w.ends_with("GB")).collect();
    match words.iter().position(|w| *w == "UHD" || *w == "Iris" || *w == "Arc" || *w == "Radeon" || *w == "RTX" || *w == "GTX") {
        Some(i) => words[i..].iter().take(3).cloned().collect::<Vec<_>>().join(" "),
        None => words.iter().rev().take(2).rev().cloned().collect::<Vec<_>>().join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lspci_and_smi() {
        let l = r#"01:00.0 "VGA compatible controller" "NVIDIA Corporation" "GA107M [GeForce RTX 3050 Mobile]" -ra1 "Dell" "Device 0b5c""#;
        assert_eq!(parse_lspci_name(l).as_deref(), Some("GeForce RTX 3050 Mobile"));
        let g = parse_nvidia_smi("NVIDIA GeForce RTX 3050 6GB Laptop GPU, 7, 2, 6144, 38, 3.74, P8\n");
        assert_eq!(g[0].name, "GeForce RTX 3050 6GB Laptop GPU");
        assert_eq!(g[0].usage, Some(7.0));
        assert_eq!(g[0].mem_total, Some(6144 * 1024 * 1024));
        assert_eq!(g[0].temp_c, Some(38.0));
        assert_eq!(g[0].power_w, Some(3.74));
        assert_eq!(g[0].pstate.as_deref(), Some("P8"));
        let na = parse_nvidia_smi("X, [N/A], 1, 2, [N/A], [N/A], [N/A]\n");
        assert_eq!(na[0].usage, None);
        assert_eq!(na[0].pstate, None);
    }

    #[test]
    fn fdinfo() {
        let t = "pos:\t0\nflags:\t02100002\ndrm-driver:\ti915\ndrm-client-id:\t38\ndrm-pdev:\t0000:00:02.0\ndrm-total-system0:\t195684 KiB\ndrm-engine-render:\t747804604404 ns\ndrm-engine-capacity-video:\t2\ndrm-engine-video:\t12 ns\n";
        let (pdev, client, engines) = parse_fdinfo(t).unwrap();
        assert_eq!(pdev, "0000:00:02.0");
        assert_eq!(client, "38");
        assert_eq!(engines, vec![("render".to_string(), 747804604404), ("video".to_string(), 12)]);
        assert!(parse_fdinfo("pos: 0\nflags: 1\n").is_none());
    }

    #[test]
    fn short_names() {
        assert_eq!(short_name("GeForce RTX 3050 6GB Laptop GPU"), "RTX 3050");
        assert_eq!(short_name("Intel Raptor Lake-S UHD Graphics"), "UHD Graphics");
        assert_eq!(short_name("AMD Phoenix1"), "Phoenix1");
    }

    #[test]
    fn drm_usage_reads_this_machine() {
        // Smoke test: two samples never panic and percentages stay in range.
        let mut d = DrmUsage::default();
        assert!(d.sample().is_empty());
        std::thread::sleep(Duration::from_millis(50));
        for v in d.sample().values() {
            assert!((0.0..=100.0).contains(v));
        }
    }
}
