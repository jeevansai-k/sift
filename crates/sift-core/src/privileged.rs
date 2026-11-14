//! Admin actions through `pkexec`.
//!
//! There is no background root helper. A confirmed batch of admin actions
//! runs as ONE `pkexec /bin/sh -c <script>`, so the user sees one password
//! prompt. The script is assembled only from [`RootCommand`]s, which can only
//! be built inside this crate from constant snippets plus tokens that passed
//! [`valid_token`] (package names, snap revisions read from the system) —
//! nothing a user types ever reaches a root shell.

use crate::envelope::EngineError;
use crate::runner::Runner;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RootCommand {
    pub id: String,
    pub title: String,
    /// The exact command line shown to the user before they confirm.
    pub display: String,
    #[serde(skip)]
    script: String,
}

impl RootCommand {
    /// Crate-internal: only engine modules mint root commands.
    pub(crate) fn new(id: &str, title: &str, display: &str, script: &str) -> RootCommand {
        RootCommand { id: id.into(), title: title.into(), display: display.into(), script: script.into() }
    }

    pub fn script(&self) -> &str {
        &self.script
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RootResult {
    pub id: String,
    pub ok: bool,
    pub code: i32,
    pub output: String,
}

/// Package names, snap names, revisions: `[a-z0-9][a-z0-9+.:~-]*`, ≤128 chars.
pub fn valid_token(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"+.:~-".contains(c))
}

const BEGIN: &str = "@@SIFT-BEGIN ";
const END: &str = "@@SIFT-END ";

/// The full shell script for a batch.
pub fn compose_script(cmds: &[RootCommand]) -> String {
    let mut s = String::from("export LC_ALL=C PATH=/usr/sbin:/usr/bin:/sbin:/bin\n");
    for c in cmds {
        s.push_str(&format!("echo '{BEGIN}{}'\n", c.id));
        s.push_str(&format!("( {} ) 2>&1\n", c.script));
        s.push_str(&format!("echo \"{END}{} $?\"\n", c.id));
    }
    s
}

/// Split the batch's combined output back into per-command results.
pub fn parse_output(out: &str, cmds: &[RootCommand]) -> Vec<RootResult> {
    let mut results: Vec<RootResult> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in out.lines() {
        if let Some(id) = line.strip_prefix(BEGIN) {
            current = Some((id.trim().to_string(), String::new()));
        } else if let Some(rest) = line.strip_prefix(END) {
            let mut parts = rest.split_whitespace();
            let id = parts.next().unwrap_or("").to_string();
            let code: i32 = parts.next().and_then(|c| c.parse().ok()).unwrap_or(-1);
            let output = current.take().map(|(_, o)| o).unwrap_or_default();
            results.push(RootResult { id, ok: code == 0, code, output: output.trim_end().to_string() });
        } else if let Some((_, buf)) = current.as_mut() {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    // Commands that never reported (batch aborted) count as failed.
    for c in cmds {
        if !results.iter().any(|r| r.id == c.id) {
            results.push(RootResult { id: c.id.clone(), ok: false, code: -1, output: "did not run".into() });
        }
    }
    results
}

/// Run a confirmed batch with one pkexec prompt.
pub fn run_batch(runner: &dyn Runner, cmds: &[RootCommand]) -> Result<Vec<RootResult>, EngineError> {
    if cmds.is_empty() {
        return Ok(Vec::new());
    }
    if crate::paths::which("pkexec").is_none() {
        return Err(EngineError::new("unavailable", "pkexec is not installed (package policykit-1 / pkexec)"));
    }
    let script = compose_script(cmds);
    let out = runner
        .run("pkexec", &["/bin/sh".into(), "-c".into(), script], Duration::from_secs(3600))
        .map_err(|e| EngineError::new("io", e.to_string()))?;
    match out.code {
        Some(126) => Err(EngineError::new("cancelled", "the password prompt was dismissed")),
        Some(127) => Err(EngineError::new("not_authorized", "not authorized (wrong password or no admin rights)")),
        _ if out.timed_out => Err(EngineError::new("timed_out", "admin batch did not finish within an hour")),
        _ => Ok(parse_output(&out.stdout, cmds)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{CmdOutput, FakeRunner};

    #[test]
    fn tokens() {
        assert!(valid_token("libreoffice-core"));
        assert!(valid_token("g++-13"));
        assert!(valid_token("libc6:amd64"));
        assert!(valid_token("2866"));
        assert!(!valid_token(""));
        assert!(!valid_token("foo bar"));
        assert!(!valid_token("x;rm -rf /"));
        assert!(!valid_token("$(id)"));
        assert!(!valid_token("-rf"));
        assert!(!valid_token("Upper"));
    }

    #[test]
    fn script_and_parse_roundtrip() {
        let cmds = vec![
            RootCommand::new("apt-clean", "APT cache", "apt-get clean", "apt-get clean"),
            RootCommand::new("journal", "Journal", "journalctl --vacuum-time=14d", "journalctl --vacuum-time=14d"),
        ];
        let s = compose_script(&cmds);
        assert!(s.contains("( apt-get clean ) 2>&1"));
        let out = "@@SIFT-BEGIN apt-clean\n@@SIFT-END apt-clean 0\n@@SIFT-BEGIN journal\nVacuuming done, freed 120.0M\n@@SIFT-END journal 0\n";
        let r = parse_output(out, &cmds);
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|x| x.ok));
        assert!(r[1].output.contains("freed"));
    }

    #[test]
    fn dismissed_prompt_is_cancelled() {
        if crate::paths::which("pkexec").is_none() {
            return;
        }
        let cmds = vec![RootCommand::new("t", "t", "true", "true")];
        let fake = FakeRunner::default().with(
            &format!("pkexec /bin/sh -c {}", compose_script(&cmds)),
            CmdOutput { code: Some(126), ..Default::default() },
        );
        let e = run_batch(&fake, &cmds).unwrap_err();
        assert_eq!(e.kind, "cancelled");
    }
}
