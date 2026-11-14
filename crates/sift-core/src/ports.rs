//! Listening ports from /proc/net/{tcp,tcp6,udp,udp6}, mapped to owning
//! processes through /proc/[pid]/fd socket inodes (your processes only, as
//! with `ss -p` without root).

use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Port {
    pub proto: String,
    pub address: String,
    pub port: u16,
    pub pid: Option<u32>,
    pub process: Option<String>,
    pub command: Option<String>,
    pub uid: u32,
    pub user: String,
    /// local (loopback only) / network (all or specific external interface)
    pub exposure: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SocketLine {
    pub address: String,
    pub port: u16,
    pub state: u8,
    pub uid: u32,
    pub inode: u64,
    pub loopback: bool,
}

fn parse_addr(hex: &str) -> Option<(String, bool)> {
    match hex.len() {
        8 => {
            // The kernel prints the raw in-memory u32 (network-order bytes
            // read as a little-endian integer on x86/ARM).
            let v = u32::from_str_radix(hex, 16).ok()?;
            let ip = Ipv4Addr::from(v.to_le_bytes());
            Some((ip.to_string(), ip.is_loopback()))
        }
        32 => {
            let mut bytes = [0u8; 16];
            for i in 0..4 {
                let word = u32::from_str_radix(&hex[i * 8..i * 8 + 8], 16).ok()?;
                bytes[i * 4..i * 4 + 4].copy_from_slice(&word.swap_bytes().to_be_bytes());
            }
            let ip = Ipv6Addr::from(bytes);
            let loopback = ip.is_loopback() || ip.to_ipv4_mapped().map(|v| v.is_loopback()).unwrap_or(false);
            let shown = match ip.to_ipv4_mapped() {
                Some(v4) => v4.to_string(),
                None => ip.to_string(),
            };
            Some((shown, loopback))
        }
        _ => None,
    }
}

pub fn parse_proc_net(text: &str) -> Vec<SocketLine> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 10 {
                return None;
            }
            let (addr, port) = f[1].split_once(':')?;
            let (address, loopback) = parse_addr(addr)?;
            Some(SocketLine {
                address,
                port: u16::from_str_radix(port, 16).ok()?,
                state: u8::from_str_radix(f[3], 16).ok()?,
                uid: f[7].parse().ok()?,
                inode: f[9].parse().ok()?,
                loopback,
            })
        })
        .collect()
}

/// socket inode → (pid, name) for processes we can inspect.
fn socket_owners() -> HashMap<u64, (u32, String)> {
    let mut map = HashMap::new();
    let Ok(rd) = fs::read_dir("/proc") else { return map };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(fds) = fs::read_dir(e.path().join("fd")) else { continue };
        let comm = fs::read_to_string(e.path().join("comm")).unwrap_or_default().trim().to_string();
        for fd in fds.flatten() {
            if let Ok(t) = fs::read_link(fd.path()) {
                let t = t.to_string_lossy();
                if let Some(ino) = t.strip_prefix("socket:[").and_then(|r| r.strip_suffix(']')).and_then(|n| n.parse().ok()) {
                    map.entry(ino).or_insert((pid, comm.clone()));
                }
            }
        }
    }
    map
}

pub fn list() -> Vec<Port> {
    let owners = socket_owners();
    let users = crate::status::system::read_users();
    let mut out: Vec<Port> = Vec::new();
    for (file, proto, listen_state) in [("tcp", "TCP", 0x0Au8), ("tcp6", "TCP", 0x0A), ("udp", "UDP", 0x07), ("udp6", "UDP", 0x07)] {
        let text = fs::read_to_string(format!("/proc/net/{file}")).unwrap_or_default();
        for s in parse_proc_net(&text) {
            if s.state != listen_state || s.port == 0 {
                continue;
            }
            let owner = owners.get(&s.inode);
            let entry = Port {
                proto: proto.into(),
                address: s.address.clone(),
                port: s.port,
                pid: owner.map(|o| o.0),
                process: owner.map(|o| o.1.clone()),
                command: owner.and_then(|o| crate::status::procs::cmdline(o.0)),
                uid: s.uid,
                user: users.get(&s.uid).cloned().unwrap_or_else(|| s.uid.to_string()),
                exposure: if s.loopback { "local".into() } else { "network".into() },
            };
            // Dual-stack sockets show up twice; keep one per proto/port/pid/address.
            if !out.iter().any(|p| p.proto == entry.proto && p.port == entry.port && p.pid == entry.pid && p.address == entry.address) {
                out.push(entry);
            }
        }
    }
    out.sort_by(|a, b| a.port.cmp(&b.port).then(a.proto.cmp(&b.proto)));
    out
}

/// Ask a process you own to stop (SIGTERM), or force it (SIGKILL).
pub fn stop_process(pid: u32, force: bool) -> Result<(), crate::envelope::EngineError> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(format!("/proc/{pid}"))
        .map_err(|_| crate::envelope::EngineError::new("missing", "process is no longer running"))?;
    if meta.uid() != crate::current_uid() {
        return Err(crate::envelope::EngineError::new("refused", "that process belongs to another user"));
    }
    if pid <= 1 {
        return Err(crate::envelope::EngineError::new("refused", "refusing to signal init"));
    }
    let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
    // SAFETY: kill(2) with a validated pid and a constant signal.
    let rc = unsafe { libc::kill(pid as libc::pid_t, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(crate::envelope::EngineError::from(std::io::Error::last_os_error()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v4_and_v6() {
        let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:0CEA 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 123456 1 0000000000000000 100 0 0 10 0\n   1: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 222 1 0000000000000000 100 0 0 10 0\n";
        let v = parse_proc_net(tcp);
        assert_eq!(v[0].address, "127.0.0.1");
        assert_eq!(v[0].port, 3306);
        assert!(v[0].loopback);
        assert_eq!(v[0].uid, 1000);
        assert_eq!(v[0].inode, 123456);
        assert_eq!(v[1].address, "0.0.0.0");
        assert_eq!(v[1].port, 22);
        assert!(!v[1].loopback);

        let tcp6 = "header\n   0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 999 1\n   1: 0000000000000000FFFF00000100007F:0050 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 998 1\n";
        let v6 = parse_proc_net(tcp6);
        assert_eq!(v6[0].address, "::1");
        assert_eq!(v6[0].port, 8080);
        assert!(v6[0].loopback);
        assert_eq!(v6[1].address, "127.0.0.1");
        assert!(v6[1].loopback);
    }

    #[test]
    fn refuses_foreign_processes() {
        assert!(stop_process(1, false).is_err());
    }
}
