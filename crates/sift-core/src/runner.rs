//! Spawning external programs. Everything that runs a command takes a
//! `&dyn Runner`, so tests drive the decision logic with canned output and
//! only [`SystemRunner`] ever touches a real process.

use serde::Serialize;
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default, Serialize)]
pub struct CmdOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl CmdOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }

    pub fn ok(stdout: &str) -> CmdOutput {
        CmdOutput { code: Some(0), stdout: stdout.into(), ..Default::default() }
    }

    /// Last non-empty line of stderr (or stdout) for error messages.
    pub fn brief_error(&self) -> String {
        let pick = if self.stderr.trim().is_empty() { &self.stdout } else { &self.stderr };
        pick.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
    }
}

pub trait Runner: Send + Sync {
    fn run(&self, program: &str, args: &[String], timeout: Duration) -> io::Result<CmdOutput>;
}

/// Convenience wrapper taking `&str` args.
pub fn run(r: &dyn Runner, program: &str, args: &[&str], timeout_secs: u64) -> io::Result<CmdOutput> {
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    r.run(program, &owned, Duration::from_secs(timeout_secs))
}

/// Real process runner: C.UTF-8 locale for parseable output, stdin closed,
/// killed at the deadline.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[String], timeout: Duration) -> io::Result<CmdOutput> {
        let exe = crate::paths::which(program)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{program} not found")))?;
        let mut child = Command::new(exe)
            .args(args)
            .env("LC_ALL", "C.UTF-8")
            .env("LANG", "C.UTF-8")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut out = child.stdout.take().expect("piped stdout");
        let mut err = child.stderr.take().expect("piped stderr");
        let t_out = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = out.read_to_end(&mut b);
            b
        });
        let t_err = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = err.read_to_end(&mut b);
            b
        });
        let start = Instant::now();
        let mut timed_out = false;
        let status = loop {
            if let Some(st) = child.try_wait()? {
                break Some(st);
            }
            if start.elapsed() >= timeout {
                let _ = child.kill();
                timed_out = true;
                break child.wait().ok();
            }
            std::thread::sleep(Duration::from_millis(15));
        };
        let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
        let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
        Ok(CmdOutput { code: status.and_then(|s| s.code()), stdout, stderr, timed_out })
    }
}

/// Canned-output runner for tests: keys are `program arg1 arg2…`.
#[derive(Default)]
pub struct FakeRunner {
    pub responses: std::collections::HashMap<String, CmdOutput>,
    pub calls: std::sync::Mutex<Vec<String>>,
}

impl FakeRunner {
    pub fn with(mut self, cmdline: &str, out: CmdOutput) -> Self {
        self.responses.insert(cmdline.to_string(), out);
        self
    }
}

impl Runner for FakeRunner {
    fn run(&self, program: &str, args: &[String], _timeout: Duration) -> io::Result<CmdOutput> {
        let key = std::iter::once(program.to_string()).chain(args.iter().cloned()).collect::<Vec<_>>().join(" ");
        self.calls.lock().unwrap().push(key.clone());
        self.responses
            .get(&key)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no fake for `{key}`")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_runner_captures_and_times_out() {
        let r = SystemRunner;
        let out = run(&r, "sh", &["-c", "echo hi; echo oops >&2; exit 3"], 5).unwrap();
        assert_eq!(out.stdout.trim(), "hi");
        assert_eq!(out.stderr.trim(), "oops");
        assert_eq!(out.code, Some(3));
        let slow = r.run("sleep", &["5".into()], Duration::from_millis(200)).unwrap();
        assert!(slow.timed_out);
        assert!(!slow.success());
    }
}
