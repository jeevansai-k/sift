//! History recording. Exactly one writer at a time holds an `flock` on
//! `sampler.lock`: the background service (`sift sampler`) if it's running,
//! otherwise the GUI while its window is open.

use crate::db::{Db, ProcSample, Sample};
use crate::paths::Paths;
use crate::settings::Settings;
use crate::status::{Collector, Snapshot};
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

pub struct SamplerLock {
    _file: File,
}

impl SamplerLock {
    /// Non-blocking: `None` if another sampler already records history.
    pub fn try_acquire(paths: &Paths) -> Option<SamplerLock> {
        std::fs::create_dir_all(paths.sift_state_dir()).ok()?;
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(paths.sampler_lock()).ok()?;
        // SAFETY: valid fd for the lifetime of `file`.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        (rc == 0).then_some(SamplerLock { _file: file })
    }

    /// Is some process currently holding the lock?
    pub fn is_held(paths: &Paths) -> bool {
        let Ok(file) = OpenOptions::new().read(true).open(paths.sampler_lock()) else { return false };
        // SAFETY: valid fd; we release immediately if we got it.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
            false
        } else {
            true
        }
    }
}

/// Averages live snapshots between history writes, so a 30 s row reflects
/// the whole 30 s rather than the last 2 s.
#[derive(Default)]
pub struct Accumulator {
    n: f64,
    cpu: f64,
    mem_pct: f64,
    disk_read: f64,
    disk_write: f64,
    net_rx: f64,
    net_tx: f64,
    gpu: (f64, f64),
    last: Option<Snapshot>,
}

impl Accumulator {
    pub fn add(&mut self, s: &Snapshot) {
        self.n += 1.0;
        self.cpu += s.cpu.usage;
        self.mem_pct += s.memory.used_pct;
        self.disk_read += s.disk_io.read_bps;
        self.disk_write += s.disk_io.write_bps;
        self.net_rx += s.net_rx_bps;
        self.net_tx += s.net_tx_bps;
        if let Some(u) = s.gpu_usage() {
            self.gpu.0 += u;
            self.gpu.1 += 1.0;
        }
        self.last = Some(s.clone());
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0.0
    }

    /// Build a history row and reset.
    pub fn take(&mut self) -> Option<(Sample, Vec<ProcSample>)> {
        let last = self.last.take()?;
        let n = self.n.max(1.0);
        let mut sample = to_sample(&last);
        sample.cpu = self.cpu / n;
        sample.mem_pct = self.mem_pct / n;
        sample.disk_read = self.disk_read / n;
        sample.disk_write = self.disk_write / n;
        sample.net_rx = self.net_rx / n;
        sample.net_tx = self.net_tx / n;
        if self.gpu.1 > 0.0 {
            sample.gpu = Some(self.gpu.0 / self.gpu.1);
        }
        *self = Accumulator::default();
        Some((sample, top_procs(&last)))
    }
}

pub fn to_sample(s: &Snapshot) -> Sample {
    let disk = s.root_disk();
    Sample {
        ts: s.collected_at,
        cpu: s.cpu.usage,
        mem_pct: s.memory.used_pct,
        mem_used: s.memory.used as i64,
        swap_used: s.memory.swap_used as i64,
        load1: s.cpu.load1,
        disk_free: disk.map(|d| d.free as i64).unwrap_or(0),
        disk_used_pct: disk.map(|d| d.used_pct).unwrap_or(0.0),
        disk_read: s.disk_io.read_bps,
        disk_write: s.disk_io.write_bps,
        net_rx: s.net_rx_bps,
        net_tx: s.net_tx_bps,
        gpu: s.gpu_usage(),
        cpu_temp: s.thermal.cpu_temp,
        gpu_temp: s.thermal.gpu_temp,
        battery: s.batteries.first().map(|b| b.percent),
        health: s.health.score as i64,
    }
}

/// Top 10 by CPU plus top 5 by memory.
pub fn top_procs(s: &Snapshot) -> Vec<ProcSample> {
    let mut by_cpu: Vec<_> = s.top_processes.iter().collect();
    by_cpu.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
    let mut out: Vec<ProcSample> = by_cpu
        .iter()
        .take(10)
        .map(|p| ProcSample { pid: p.pid as i64, name: p.name.clone(), cpu: p.cpu, mem: p.mem_bytes as i64 })
        .collect();
    let mut by_mem: Vec<_> = s.top_processes.iter().collect();
    by_mem.sort_by(|a, b| b.mem_bytes.cmp(&a.mem_bytes));
    for p in by_mem.iter().take(5) {
        if !out.iter().any(|o| o.pid == p.pid as i64) {
            out.push(ProcSample { pid: p.pid as i64, name: p.name.clone(), cpu: p.cpu, mem: p.mem_bytes as i64 });
        }
    }
    out
}

/// The background service loop (`sift sampler`). Re-reads settings each
/// cycle so interval/retention changes apply without a restart.
pub fn run_daemon(paths: &Paths, once: bool) -> Result<(), crate::envelope::EngineError> {
    let _lock = SamplerLock::try_acquire(paths)
        .ok_or_else(|| crate::envelope::EngineError::new("busy", "another Sift sampler is already recording"))?;
    let mut db = Db::open(&paths.db_file())?;
    let mut collector = Collector::new();
    collector.top_n = 20;
    let mut last_prune = Instant::now() - Duration::from_secs(7200);
    loop {
        let settings = Settings::load(paths);
        let snap = collector.collect();
        let mut acc = Accumulator::default();
        acc.add(&snap);
        if let Some((sample, procs)) = acc.take() {
            if let Err(e) = db.insert_sample(&sample, &procs) {
                eprintln!("sift sampler: {e}");
            }
        }
        if last_prune.elapsed() > Duration::from_secs(3600) {
            let _ = db.prune(settings.retention_days, crate::now_unix());
            last_prune = Instant::now();
        }
        if once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(settings.sample_interval_secs as u64));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::for_home(d.path().to_path_buf(), true);
        let a = SamplerLock::try_acquire(&p).expect("first");
        assert!(SamplerLock::try_acquire(&p).is_none());
        assert!(SamplerLock::is_held(&p));
        drop(a);
        assert!(!SamplerLock::is_held(&p));
        assert!(SamplerLock::try_acquire(&p).is_some());
    }

    #[test]
    fn accumulator_averages() {
        let mut acc = Accumulator::default();
        let mut s = Snapshot::default();
        s.cpu.usage = 10.0;
        acc.add(&s);
        s.cpu.usage = 30.0;
        s.collected_at = 99;
        acc.add(&s);
        let (row, _) = acc.take().unwrap();
        assert_eq!(row.cpu, 20.0);
        assert_eq!(row.ts, 99);
        assert!(acc.is_empty());
    }
}
