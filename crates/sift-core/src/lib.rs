//! Sift engine.
//!
//! Everything Sift knows how to do lives here, with no GUI dependency, so the
//! `sift` CLI, the background sampler and the GTK app all run the same code.
//! The shape follows Burrow's engine: pure parsers fed by injected command
//! runners, one JSON envelope for buffered results, NDJSON events for streams,
//! and a single guarded remover behind every destructive path.

pub mod analyze;
pub mod apps;
pub mod cancel;
pub mod clean;
pub mod ctx;
pub mod db;
pub mod doctor;
pub mod dupes;
pub mod envelope;
pub mod format;
pub mod fsutil;
pub mod installer;
pub mod openfiles;
pub mod ops;
pub mod optimize;
pub mod paths;
pub mod ports;
pub mod privileged;
pub mod purge;
pub mod removal;
pub mod runner;
pub mod safety;
pub mod sampler;
pub mod service;
pub mod settings;
pub mod status;
pub mod trash;
pub mod treemap;

/// Engine name reported in every envelope.
pub const ENGINE: &str = "sift";
/// Engine version (the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Current unix time in seconds.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Effective uid of this process.
pub fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}
