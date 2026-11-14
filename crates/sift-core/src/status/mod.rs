//! Live system status from /proc and /sys — the Linux counterpart of
//! Burrow's `status --json` snapshot.

pub mod cpu;
pub mod disk;
pub mod gpu;
pub mod health;
pub mod mem;
pub mod net;
pub mod power;
pub mod procs;
pub mod system;
pub mod thermal;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub collected_at: i64,
    pub host: String,
    pub os: String,
    pub kernel: String,
    pub model: String,
    pub uptime_secs: u64,
    pub procs: u32,
    pub cpu: Cpu,
    pub memory: Memory,
    pub gpus: Vec<Gpu>,
    pub disks: Vec<Disk>,
    pub disk_io: DiskIo,
    pub network: Vec<NetIf>,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub batteries: Vec<Battery>,
    pub ac_online: Option<bool>,
    pub thermal: Thermal,
    pub top_processes: Vec<Process>,
    pub health: Health,
}

impl Snapshot {
    /// The disk holding `/` (or the first disk).
    pub fn root_disk(&self) -> Option<&Disk> {
        self.disks.iter().find(|d| d.mount == "/").or_else(|| self.disks.first())
    }

    /// Highest busy % across GPUs that report it.
    pub fn gpu_usage(&self) -> Option<f64> {
        self.gpus.iter().filter_map(|g| g.usage).reduce(f64::max)
    }

    /// The busiest physical interface right now (for the network card).
    pub fn primary_net(&self) -> Option<&NetIf> {
        self.network
            .iter()
            .filter(|n| n.physical)
            .max_by(|a, b| (a.rx_bps + a.tx_bps).total_cmp(&(b.rx_bps + b.tx_bps)).then(a.ip.is_some().cmp(&b.ip.is_some())))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cpu {
    pub model: String,
    pub usage: f64,
    pub per_core: Vec<f64>,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub logical: u32,
    pub physical: u32,
    pub freq_mhz: Option<f64>,
    pub temp_c: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Memory {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub cached: u64,
    pub used_pct: f64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub psi_some10: Option<f64>,
    pub psi_some60: Option<f64>,
    pub psi_full10: Option<f64>,
    /// normal / warning / critical
    pub pressure: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Gpu {
    pub name: String,
    pub vendor: String,
    pub usage: Option<f64>,
    pub mem_used: Option<u64>,
    pub mem_total: Option<u64>,
    pub temp_c: Option<f64>,
    pub power_w: Option<f64>,
    /// NVIDIA performance state (P0 busiest … P8 idle).
    pub pstate: Option<String>,
    /// active / sleeping / unknown
    pub state: String,
    /// Has a connected, enabled display.
    pub drives_display: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Disk {
    pub mount: String,
    pub device: String,
    pub fstype: String,
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub used_pct: f64,
    pub removable: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiskIo {
    pub read_bps: f64,
    pub write_bps: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetIf {
    pub name: String,
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub rx_total: u64,
    pub tx_total: u64,
    pub ip: Option<String>,
    pub physical: bool,
    pub wireless: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Battery {
    pub name: String,
    pub percent: f64,
    pub status: String,
    pub health_pct: Option<f64>,
    pub cycles: Option<u32>,
    pub time_left_secs: Option<u64>,
    pub power_w: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Fan {
    pub label: String,
    pub rpm: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sensor {
    pub label: String,
    pub temp_c: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Thermal {
    pub cpu_temp: Option<f64>,
    pub gpu_temp: Option<f64>,
    pub fans: Vec<Fan>,
    pub sensors: Vec<Sensor>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub command: String,
    pub user: String,
    pub uid: u32,
    pub cpu: f64,
    pub mem_bytes: u64,
    pub mem_pct: f64,
    pub state: String,
    pub threads: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HealthFactor {
    pub name: String,
    pub penalty: f64,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Health {
    pub score: u8,
    pub label: String,
    pub message: String,
    pub factors: Vec<HealthFactor>,
}

/// Raw counters read in one pass; rates come from two of these.
#[derive(Debug, Clone, Default)]
struct Counters {
    at: Option<Instant>,
    cpu_total: cpu::Times,
    cpu_cores: Vec<cpu::Times>,
    disk_read_sectors: u64,
    disk_write_sectors: u64,
    net: HashMap<String, (u64, u64)>,
    proc_ticks: HashMap<u32, u64>,
}

struct StaticInfo {
    host: String,
    os: String,
    kernel: String,
    model: String,
    cpu_model: String,
    physical: u32,
}

/// Stateful collector: keeps the previous counters so each call reports
/// rates and CPU usage over the interval since the last call.
pub struct Collector {
    prev: Option<Counters>,
    statics: Option<StaticInfo>,
    gpu: gpu::GpuProbe,
    users: HashMap<u32, String>,
    clk_tck: f64,
    page_size: u64,
    /// How many processes to keep in the snapshot.
    pub top_n: usize,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    pub fn new() -> Collector {
        // SAFETY: sysconf has no preconditions.
        let clk = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        Collector {
            prev: None,
            statics: None,
            gpu: gpu::GpuProbe::default(),
            users: system::read_users(),
            clk_tck: if clk > 0 { clk as f64 } else { 100.0 },
            page_size: if page > 0 { page as u64 } else { 4096 },
            top_n: 40,
        }
    }

    fn read_counters(&self, procs: &[procs::RawProc]) -> Counters {
        let stat = fs::read_to_string("/proc/stat").unwrap_or_default();
        let (cpu_total, cpu_cores) = cpu::parse_stat(&stat);
        let diskstats = fs::read_to_string("/proc/diskstats").unwrap_or_default();
        let physical = disk::physical_block_devices();
        let (r, w) = disk::sum_diskstats(&diskstats, &physical);
        let netdev = fs::read_to_string("/proc/net/dev").unwrap_or_default();
        let net = net::parse_net_dev(&netdev).into_iter().map(|(n, rx, tx)| (n, (rx, tx))).collect();
        let proc_ticks = procs.iter().map(|p| (p.pid, p.ticks)).collect();
        Counters {
            at: Some(Instant::now()),
            cpu_total,
            cpu_cores,
            disk_read_sectors: r,
            disk_write_sectors: w,
            net,
            proc_ticks,
        }
    }

    /// Take a snapshot. The first call primes the counters with a short
    /// sleep so even a one-shot `sift status` reports real rates.
    pub fn collect(&mut self) -> Snapshot {
        if self.prev.is_none() {
            let raw = procs::read_all();
            self.prev = Some(self.read_counters(&raw));
            self.gpu.prime();
            std::thread::sleep(Duration::from_millis(400));
        }
        let raw = procs::read_all();
        let cur = self.read_counters(&raw);
        let prev = self.prev.take().unwrap_or_default();
        let secs = match (prev.at, cur.at) {
            (Some(a), Some(b)) => b.duration_since(a).as_secs_f64().max(0.001),
            _ => 1.0,
        };

        let statics = self.statics.get_or_insert_with(|| {
            let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
            let (cpu_model, physical) = cpu::parse_cpuinfo(&cpuinfo);
            StaticInfo {
                host: system::hostname(),
                os: system::os_name(),
                kernel: system::kernel(),
                model: system::model(),
                cpu_model,
                physical,
            }
        });

        // CPU
        let usage = cpu::usage(&prev.cpu_total, &cur.cpu_total);
        let per_core: Vec<f64> = cur
            .cpu_cores
            .iter()
            .enumerate()
            .map(|(i, c)| prev.cpu_cores.get(i).map(|p| cpu::usage(p, c)).unwrap_or(0.0))
            .collect();
        let loadavg = fs::read_to_string("/proc/loadavg").unwrap_or_default();
        let (load1, load5, load15) = cpu::parse_loadavg(&loadavg);
        let thermal = thermal::read();
        let cpu = Cpu {
            model: statics.cpu_model.clone(),
            usage,
            logical: per_core.len() as u32,
            physical: if statics.physical > 0 { statics.physical } else { per_core.len() as u32 },
            per_core,
            load1,
            load5,
            load15,
            freq_mhz: cpu::average_freq_mhz(),
            temp_c: thermal.cpu_temp,
        };

        // Memory
        let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let psi = fs::read_to_string("/proc/pressure/memory").ok();
        let memory = mem::build(&meminfo, psi.as_deref());

        // Disks + IO
        let mountinfo = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        let disks = disk::list_disks(&mountinfo);
        let disk_io = DiskIo {
            read_bps: cur.disk_read_sectors.saturating_sub(prev.disk_read_sectors) as f64 * 512.0 / secs,
            write_bps: cur.disk_write_sectors.saturating_sub(prev.disk_write_sectors) as f64 * 512.0 / secs,
        };

        // Network
        let addrs = net::interface_addresses();
        let mut network = Vec::new();
        let (mut rx_sum, mut tx_sum) = (0.0, 0.0);
        let mut names: Vec<&String> = cur.net.keys().collect();
        names.sort();
        for name in names {
            if name == "lo" {
                continue;
            }
            let (rx, tx) = cur.net[name];
            let (prx, ptx) = prev.net.get(name).copied().unwrap_or((rx, tx));
            let physical = net::is_physical(name);
            let iface = NetIf {
                name: name.clone(),
                rx_bps: rx.saturating_sub(prx) as f64 / secs,
                tx_bps: tx.saturating_sub(ptx) as f64 / secs,
                rx_total: rx,
                tx_total: tx,
                ip: addrs.get(name).cloned(),
                physical,
                wireless: net::is_wireless(name),
            };
            if physical {
                rx_sum += iface.rx_bps;
                tx_sum += iface.tx_bps;
            }
            network.push(iface);
        }

        // GPU, power
        let mut gpus = self.gpu.read();
        let gpu_temp = gpus.iter().find_map(|g| g.temp_c).or(thermal.gpu_temp);
        for g in gpus.iter_mut() {
            if g.temp_c.is_none() && g.vendor == "AMD" {
                g.temp_c = thermal.gpu_temp;
            }
        }
        let (batteries, ac_online) = power::read();

        // Processes
        let mut processes = procs::build(
            &raw,
            &prev.proc_ticks,
            secs,
            self.clk_tck,
            self.page_size,
            memory.total,
            &self.users,
        );
        processes.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.mem_bytes.cmp(&a.mem_bytes)));
        let proc_count = processes.len() as u32;
        // Keep the busiest by CPU plus the largest by memory.
        let mut top: Vec<Process> = processes.iter().take(self.top_n).cloned().collect();
        let mut by_mem: Vec<&Process> = processes.iter().collect();
        by_mem.sort_by(|a, b| b.mem_bytes.cmp(&a.mem_bytes));
        for p in by_mem.into_iter().take(self.top_n / 2) {
            if !top.iter().any(|t| t.pid == p.pid) {
                top.push(p.clone());
            }
        }
        for p in top.iter_mut() {
            p.command = procs::cmdline(p.pid).unwrap_or_else(|| p.name.clone());
        }

        let mut snap = Snapshot {
            collected_at: crate::now_unix(),
            host: statics.host.clone(),
            os: statics.os.clone(),
            kernel: statics.kernel.clone(),
            model: statics.model.clone(),
            uptime_secs: system::uptime_secs(),
            procs: proc_count,
            cpu,
            memory,
            gpus,
            disks,
            disk_io,
            network,
            net_rx_bps: rx_sum,
            net_tx_bps: tx_sum,
            batteries,
            ac_online,
            thermal: Thermal { gpu_temp, ..thermal },
            top_processes: top,
            health: Health::default(),
        };
        snap.health = health::score(&snap);
        self.prev = Some(cur);
        snap
    }
}
