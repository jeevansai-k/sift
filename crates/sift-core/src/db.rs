//! Local history and activity: one SQLite file (WAL) in
//! `~/.local/state/sift/sift.db`, shared by the background sampler, the GUI
//! and the CLI.

use crate::envelope::EngineError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    pub ts: i64,
    pub cpu: f64,
    pub mem_pct: f64,
    pub mem_used: i64,
    pub swap_used: i64,
    pub load1: f64,
    pub disk_free: i64,
    pub disk_used_pct: f64,
    pub disk_read: f64,
    pub disk_write: f64,
    pub net_rx: f64,
    pub net_tx: f64,
    pub gpu: Option<f64>,
    pub cpu_temp: Option<f64>,
    pub gpu_temp: Option<f64>,
    pub battery: Option<f64>,
    pub health: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProcSample {
    pub pid: i64,
    pub name: String,
    pub cpu: f64,
    pub mem: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcUsage {
    pub name: String,
    pub peak_cpu: f64,
    pub avg_cpu: f64,
    pub peak_mem: i64,
    pub samples: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageMetric {
    PeakCpu,
    AvgCpu,
    PeakMem,
}

#[derive(Debug, Clone, Serialize)]
pub struct Operation {
    pub id: i64,
    pub started: i64,
    pub finished: Option<i64>,
    pub kind: String,
    pub dry_run: bool,
    pub status: String,
    pub items: i64,
    pub bytes: i64,
    pub failed: i64,
    pub summary: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Removal {
    pub id: i64,
    pub op_id: i64,
    pub ts: i64,
    pub path: String,
    pub bytes: i64,
    pub action: String,
    pub status: String,
    pub reason: String,
}

pub struct Db {
    conn: Connection,
}

fn dberr(e: rusqlite::Error) -> EngineError {
    EngineError::new("db", e.to_string())
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS samples(
  ts INTEGER PRIMARY KEY, cpu REAL, mem_pct REAL, mem_used INTEGER, swap_used INTEGER, load1 REAL,
  disk_free INTEGER, disk_used_pct REAL, disk_read REAL, disk_write REAL, net_rx REAL, net_tx REAL,
  gpu REAL, cpu_temp REAL, gpu_temp REAL, battery REAL, health INTEGER);
CREATE TABLE IF NOT EXISTS proc_samples(ts INTEGER, pid INTEGER, name TEXT, cpu REAL, mem INTEGER);
CREATE INDEX IF NOT EXISTS proc_samples_ts ON proc_samples(ts);
CREATE TABLE IF NOT EXISTS operations(
  id INTEGER PRIMARY KEY AUTOINCREMENT, started INTEGER, finished INTEGER, kind TEXT, dry_run INTEGER,
  status TEXT, items INTEGER, bytes INTEGER, failed INTEGER, summary TEXT, source TEXT);
CREATE TABLE IF NOT EXISTS removals(
  id INTEGER PRIMARY KEY AUTOINCREMENT, op_id INTEGER, ts INTEGER, path TEXT, bytes INTEGER,
  action TEXT, status TEXT, reason TEXT);
CREATE INDEX IF NOT EXISTS removals_op ON removals(op_id);
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);
";

impl Db {
    pub fn open(path: &Path) -> Result<Db, EngineError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path).map_err(dberr)?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(dberr)?;
        conn.pragma_update(None, "journal_mode", "WAL").map_err(dberr)?;
        conn.pragma_update(None, "synchronous", "NORMAL").map_err(dberr)?;
        conn.execute_batch(SCHEMA).map_err(dberr)?;
        Ok(Db { conn })
    }

    pub fn open_in_memory() -> Result<Db, EngineError> {
        let conn = Connection::open_in_memory().map_err(dberr)?;
        conn.execute_batch(SCHEMA).map_err(dberr)?;
        Ok(Db { conn })
    }

    pub fn insert_sample(&mut self, s: &Sample, procs: &[ProcSample]) -> Result<(), EngineError> {
        let tx = self.conn.transaction().map_err(dberr)?;
        tx.execute(
            "INSERT OR REPLACE INTO samples VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![
                s.ts, s.cpu, s.mem_pct, s.mem_used, s.swap_used, s.load1, s.disk_free, s.disk_used_pct,
                s.disk_read, s.disk_write, s.net_rx, s.net_tx, s.gpu, s.cpu_temp, s.gpu_temp, s.battery, s.health
            ],
        )
        .map_err(dberr)?;
        {
            let mut stmt = tx
                .prepare_cached("INSERT INTO proc_samples VALUES (?1,?2,?3,?4,?5)")
                .map_err(dberr)?;
            for p in procs {
                stmt.execute(params![s.ts, p.pid, p.name, p.cpu, p.mem]).map_err(dberr)?;
            }
        }
        tx.commit().map_err(dberr)
    }

    /// Samples in `[from, to]`, averaged into at most `max_points` buckets
    /// (free disk takes the bucket minimum, health its worst value).
    pub fn samples(&self, from: i64, to: i64, max_points: usize) -> Result<Vec<Sample>, EngineError> {
        let span = (to - from + 1).max(1);
        let n = max_points.max(1) as i64;
        let bucket = ((span + n - 1) / n).max(1);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT ?1 + ((ts - ?1) / ?3) * ?3 AS b, AVG(cpu), AVG(mem_pct), AVG(mem_used), AVG(swap_used), AVG(load1),
                        MIN(disk_free), AVG(disk_used_pct), AVG(disk_read), AVG(disk_write), AVG(net_rx), AVG(net_tx),
                        AVG(gpu), AVG(cpu_temp), AVG(gpu_temp), AVG(battery), MIN(health)
                 FROM samples WHERE ts BETWEEN ?1 AND ?2 GROUP BY b ORDER BY b",
            )
            .map_err(dberr)?;
        let rows = stmt
            .query_map(params![from, to, bucket], |r| {
                Ok(Sample {
                    ts: r.get(0)?,
                    cpu: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    mem_pct: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    mem_used: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0) as i64,
                    swap_used: r.get::<_, Option<f64>>(4)?.unwrap_or(0.0) as i64,
                    load1: r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                    disk_free: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
                    disk_used_pct: r.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
                    disk_read: r.get::<_, Option<f64>>(8)?.unwrap_or(0.0),
                    disk_write: r.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
                    net_rx: r.get::<_, Option<f64>>(10)?.unwrap_or(0.0),
                    net_tx: r.get::<_, Option<f64>>(11)?.unwrap_or(0.0),
                    gpu: r.get(12)?,
                    cpu_temp: r.get(13)?,
                    gpu_temp: r.get(14)?,
                    battery: r.get(15)?,
                    health: r.get::<_, Option<i64>>(16)?.unwrap_or(0),
                })
            })
            .map_err(dberr)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(dberr)
    }

    pub fn latest_sample(&self) -> Option<Sample> {
        let ts: Option<i64> = self.conn.query_row("SELECT MAX(ts) FROM samples", [], |r| r.get(0)).ok().flatten();
        let ts = ts?;
        self.samples(ts, ts, 1).ok()?.into_iter().next()
    }

    /// (row count, first ts, last ts)
    pub fn sample_stats(&self) -> (i64, Option<i64>, Option<i64>) {
        self.conn
            .query_row("SELECT COUNT(*), MIN(ts), MAX(ts) FROM samples", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap_or((0, None, None))
    }

    pub fn process_usage(&self, from: i64, to: i64, limit: usize, metric: UsageMetric) -> Result<Vec<ProcUsage>, EngineError> {
        let order = match metric {
            UsageMetric::PeakCpu => "2",
            UsageMetric::AvgCpu => "3",
            UsageMetric::PeakMem => "4",
        };
        let sql = format!(
            "SELECT name, MAX(cpu), AVG(cpu), MAX(mem), COUNT(*) FROM proc_samples
             WHERE ts BETWEEN ?1 AND ?2 GROUP BY name ORDER BY {order} DESC LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql).map_err(dberr)?;
        let rows = stmt
            .query_map(params![from, to, limit as i64], |r| {
                Ok(ProcUsage {
                    name: r.get(0)?,
                    peak_cpu: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    avg_cpu: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    peak_mem: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    samples: r.get(4)?,
                })
            })
            .map_err(dberr)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(dberr)
    }

    /// Delete history older than `retention_days`. Returns rows removed.
    pub fn prune(&mut self, retention_days: u32, now: i64) -> Result<usize, EngineError> {
        let cutoff = now - retention_days as i64 * 86_400;
        let a = self.conn.execute("DELETE FROM samples WHERE ts < ?1", params![cutoff]).map_err(dberr)?;
        let b = self.conn.execute("DELETE FROM proc_samples WHERE ts < ?1", params![cutoff]).map_err(dberr)?;
        Ok(a + b)
    }

    pub fn clear_history(&mut self) -> Result<(), EngineError> {
        self.conn.execute_batch("DELETE FROM samples; DELETE FROM proc_samples;").map_err(dberr)?;
        let _ = self.conn.execute_batch("VACUUM;");
        Ok(())
    }

    pub fn begin_op(&mut self, kind: &str, dry_run: bool, source: &str) -> Result<i64, EngineError> {
        self.conn
            .execute(
                "INSERT INTO operations(started, kind, dry_run, status, items, bytes, failed, summary, source)
                 VALUES (?1, ?2, ?3, 'running', 0, 0, 0, '', ?4)",
                params![crate::now_unix(), kind, dry_run as i64, source],
            )
            .map_err(dberr)?;
        Ok(self.conn.last_insert_rowid())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_op(&mut self, id: i64, status: &str, items: i64, bytes: i64, failed: i64, summary: &str) -> Result<(), EngineError> {
        self.conn
            .execute(
                "UPDATE operations SET finished=?2, status=?3, items=?4, bytes=?5, failed=?6, summary=?7 WHERE id=?1",
                params![id, crate::now_unix(), status, items, bytes, failed, summary],
            )
            .map_err(dberr)?;
        Ok(())
    }

    pub fn add_removal(&mut self, op_id: i64, path: &str, bytes: i64, action: &str, status: &str, reason: &str) -> Result<(), EngineError> {
        self.conn
            .execute(
                "INSERT INTO removals(op_id, ts, path, bytes, action, status, reason) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![op_id, crate::now_unix(), path, bytes, action, status, reason],
            )
            .map_err(dberr)?;
        Ok(())
    }

    pub fn operations(&self, limit: usize) -> Result<Vec<Operation>, EngineError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, started, finished, kind, dry_run, status, items, bytes, failed, summary, source
                 FROM operations ORDER BY id DESC LIMIT ?1",
            )
            .map_err(dberr)?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(Operation {
                    id: r.get(0)?,
                    started: r.get(1)?,
                    finished: r.get(2)?,
                    kind: r.get(3)?,
                    dry_run: r.get::<_, i64>(4)? != 0,
                    status: r.get(5)?,
                    items: r.get(6)?,
                    bytes: r.get(7)?,
                    failed: r.get(8)?,
                    summary: r.get(9)?,
                    source: r.get(10)?,
                })
            })
            .map_err(dberr)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(dberr)
    }

    pub fn removals(&self, op_id: Option<i64>, limit: usize) -> Result<Vec<Removal>, EngineError> {
        let sql = if op_id.is_some() {
            "SELECT id, op_id, ts, path, bytes, action, status, reason FROM removals WHERE op_id=?1 ORDER BY id LIMIT ?2"
        } else {
            "SELECT id, op_id, ts, path, bytes, action, status, reason FROM removals WHERE ?1 IS NULL ORDER BY id DESC LIMIT ?2"
        };
        let mut stmt = self.conn.prepare(sql).map_err(dberr)?;
        let rows = stmt
            .query_map(params![op_id, limit as i64], |r| {
                Ok(Removal {
                    id: r.get(0)?,
                    op_id: r.get(1)?,
                    ts: r.get(2)?,
                    path: r.get(3)?,
                    bytes: r.get(4)?,
                    action: r.get(5)?,
                    status: r.get(6)?,
                    reason: r.get(7)?,
                })
            })
            .map_err(dberr)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(dberr)
    }

    /// Total bytes reported by finished, non-dry-run operations.
    pub fn reclaimed_total(&self) -> i64 {
        self.conn
            .query_row("SELECT COALESCE(SUM(bytes),0) FROM operations WHERE dry_run=0 AND status!='running'", [], |r| r.get(0))
            .unwrap_or(0)
    }

    pub fn meta_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key=?1", params![key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn meta_set(&mut self, key: &str, value: &str) -> Result<(), EngineError> {
        self.conn
            .execute("INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)", params![key, value])
            .map_err(dberr)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(ts: i64, cpu: f64, free: i64) -> Sample {
        Sample { ts, cpu, disk_free: free, health: 90, ..Default::default() }
    }

    #[test]
    fn samples_roundtrip_and_bucket() {
        let mut db = Db::open_in_memory().unwrap();
        for i in 0..10 {
            db.insert_sample(&s(1000 + i * 10, i as f64 * 10.0, 500 - i), &[ProcSample { pid: 1, name: "a".into(), cpu: i as f64, mem: 10 }])
                .unwrap();
        }
        let all = db.samples(1000, 1090, 1000).unwrap();
        assert_eq!(all.len(), 10);
        assert_eq!(all[3].cpu, 30.0);
        let two = db.samples(1000, 1099, 2).unwrap();
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].cpu, 20.0); // avg of 0,10,20,30,40
        assert_eq!(two[0].disk_free, 496); // min within bucket
        assert_eq!(db.sample_stats().0, 10);
        assert_eq!(db.latest_sample().unwrap().ts, 1090);
    }

    #[test]
    fn process_usage_and_prune() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_sample(&s(100, 1.0, 1), &[
            ProcSample { pid: 1, name: "a".into(), cpu: 50.0, mem: 5 },
            ProcSample { pid: 2, name: "b".into(), cpu: 10.0, mem: 900 },
        ]).unwrap();
        db.insert_sample(&s(200, 1.0, 1), &[ProcSample { pid: 1, name: "a".into(), cpu: 10.0, mem: 5 }]).unwrap();
        let top = db.process_usage(0, 1000, 5, UsageMetric::PeakCpu).unwrap();
        assert_eq!(top[0].name, "a");
        assert_eq!(top[0].peak_cpu, 50.0);
        assert_eq!(top[0].avg_cpu, 30.0);
        let mem = db.process_usage(0, 1000, 5, UsageMetric::PeakMem).unwrap();
        assert_eq!(mem[0].name, "b");
        let removed = db.prune(1, 200 + 86_400 - 50).unwrap();
        assert_eq!(removed, 3); // sample@100 + its two procs
        assert_eq!(db.sample_stats().0, 1);
    }

    #[test]
    fn operations_and_removals() {
        let mut db = Db::open_in_memory().unwrap();
        let id = db.begin_op("clean", false, "test").unwrap();
        db.add_removal(id, "/h/.cache/pip", 1000, "trash", "removed", "").unwrap();
        db.add_removal(id, "/h/.cache/x", 0, "trash", "refused", "protected").unwrap();
        db.finish_op(id, "done", 1, 1000, 1, "moved 1 KB to Trash").unwrap();
        let ops = db.operations(10).unwrap();
        assert_eq!(ops[0].kind, "clean");
        assert_eq!(ops[0].bytes, 1000);
        assert_eq!(db.removals(Some(id), 10).unwrap().len(), 2);
        assert_eq!(db.removals(None, 10).unwrap().len(), 2);
        assert_eq!(db.reclaimed_total(), 1000);
    }
}
