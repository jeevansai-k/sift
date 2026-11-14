//! Network: /proc/net/dev counters and interface addresses.

use std::collections::HashMap;
use std::ffi::CStr;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;

/// (name, rx bytes, tx bytes)
pub fn parse_net_dev(text: &str) -> Vec<(String, u64, u64)> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let f: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if f.len() < 9 {
                return None;
            }
            Some((name.trim().to_string(), f[0], f[8]))
        })
        .collect()
}

/// Backed by real hardware (not lo, bridges, veth, docker, tun…).
pub fn is_physical(name: &str) -> bool {
    Path::new("/sys/class/net").join(name).join("device").exists()
}

pub fn is_wireless(name: &str) -> bool {
    let base = Path::new("/sys/class/net").join(name);
    base.join("wireless").exists() || base.join("phy80211").exists()
}

/// First IPv4 (else global IPv6) address per interface.
pub fn interface_addresses() -> HashMap<String, String> {
    let mut v4: HashMap<String, String> = HashMap::new();
    let mut v6: HashMap<String, String> = HashMap::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills a linked list we free with freeifaddrs.
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
        return v4;
    }
    let mut cur = ifap;
    while !cur.is_null() {
        // SAFETY: cur is a valid node of the list until freeifaddrs.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() {
            continue;
        }
        let name = unsafe { CStr::from_ptr(ifa.ifa_name) }.to_string_lossy().into_owned();
        let family = unsafe { (*ifa.ifa_addr).sa_family } as i32;
        if family == libc::AF_INET {
            let sin = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
            let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
            v4.entry(name).or_insert_with(|| ip.to_string());
        } else if family == libc::AF_INET6 {
            let sin6 = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
            let ip = Ipv6Addr::from(sin6.sin6_addr.s6_addr);
            let seg0 = ip.segments()[0];
            let link_local = (seg0 & 0xffc0) == 0xfe80;
            if !link_local && !ip.is_loopback() {
                v6.entry(name).or_insert_with(|| ip.to_string());
            }
        }
    }
    unsafe { libc::freeifaddrs(ifap) };
    for (k, v) in v6 {
        v4.entry(k).or_insert(v);
    }
    v4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_dev() {
        let text = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo: 1000 10 0 0 0 0 0 0 1000 10 0 0 0 0 0 0\n wlp0s20f3: 5000 50 0 0 0 0 0 0 7000 70 0 0 0 0 0 0\n";
        let v = parse_net_dev(text);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1], ("wlp0s20f3".to_string(), 5000, 7000));
    }

    #[test]
    fn addresses_include_loopback_v4() {
        let a = interface_addresses();
        assert_eq!(a.get("lo").map(|s| s.as_str()), Some("127.0.0.1"));
    }
}
