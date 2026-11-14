//! Activity logging for operations. Logging is best-effort: a broken or
//! locked database never blocks a cleanup, but every removal that *does* get
//! logged carries its exact path, size, action and outcome.

use crate::db::Db;
use crate::paths::Paths;

pub struct OpLog {
    db: Option<Db>,
    id: Option<i64>,
    pub items: i64,
    pub bytes: i64,
    pub failed: i64,
}

impl OpLog {
    pub fn begin(paths: &Paths, kind: &str, dry_run: bool, source: &str) -> OpLog {
        let mut db = Db::open(&paths.db_file()).ok();
        let id = db.as_mut().and_then(|d| d.begin_op(kind, dry_run, source).ok());
        OpLog { db, id, items: 0, bytes: 0, failed: 0 }
    }

    /// A no-op logger (for previews that aren't worth recording).
    pub fn disabled() -> OpLog {
        OpLog { db: None, id: None, items: 0, bytes: 0, failed: 0 }
    }

    pub fn id(&self) -> Option<i64> {
        self.id
    }

    /// Record one path. `status` is `removed`, `trashed`, `failed`, `refused` or `skipped`.
    pub fn record(&mut self, path: &str, bytes: u64, action: &str, status: &str, reason: &str) {
        match status {
            "removed" | "trashed" | "done" => {
                self.items += 1;
                self.bytes += bytes as i64;
            }
            "failed" => self.failed += 1,
            _ => {}
        }
        if let (Some(db), Some(id)) = (self.db.as_mut(), self.id) {
            let _ = db.add_removal(id, path, bytes as i64, action, status, reason);
        }
    }

    pub fn finish(mut self, status: &str, summary: &str) {
        if let (Some(db), Some(id)) = (self.db.as_mut(), self.id) {
            let _ = db.finish_op(id, status, self.items, self.bytes, self.failed, summary);
        }
    }
}
