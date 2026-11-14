//! Static host facts: hostname, distro, kernel, hardware model, uptime, users.

use std::collections::HashMap;
use std::fs;

fn read_trim(p: &str) -> String {
    fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

pub fn hostname() -> String {
    read_trim("/proc/sys/kernel/hostname")
}

pub fn kernel() -> String {
    read_trim("/proc/sys/kernel/osrelease")
}

/// PRETTY_NAME from os-release.
pub fn os_name() -> String {
    let text = fs::read_to_string("/etc/os-release")
        .or_else(|_| fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    parse_os_release(&text).unwrap_or_else(|| "Linux".into())
}

pub fn parse_os_release(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_string())
}

/// "Dell Inc. Alienware m16 R1" → "Alienware m16 R1"-ish model string.
pub fn model() -> String {
    let vendor = read_trim("/sys/class/dmi/id/sys_vendor");
    let product = read_trim("/sys/class/dmi/id/product_name");
    let family = read_trim("/sys/class/dmi/id/product_family");
    let junk = |s: &str| s.is_empty() || s.eq_ignore_ascii_case("To Be Filled By O.E.M.") || s.eq_ignore_ascii_case("System Product Name");
    if !junk(&product) {
        if product.to_lowercase().contains(&vendor.to_lowercase()) || vendor.is_empty() || vendor.starts_with("Dell") {
            return product;
        }
        return format!("{} {}", vendor.trim_end_matches(" Inc."), product);
    }
    if !junk(&family) {
        return family;
    }
    "Linux PC".into()
}

pub fn uptime_secs() -> u64 {
    read_trim("/proc/uptime")
        .split_whitespace()
        .next()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v as u64)
        .unwrap_or(0)
}

/// uid → user name from /etc/passwd.
pub fn read_users() -> HashMap<u32, String> {
    parse_passwd(&fs::read_to_string("/etc/passwd").unwrap_or_default())
}

pub fn parse_passwd(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            Some((f.get(2)?.parse().ok()?, f.first()?.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(parse_os_release("NAME=\"Pop!_OS\"\nPRETTY_NAME=\"Pop!_OS 24.04 LTS\"\n").as_deref(), Some("Pop!_OS 24.04 LTS"));
        let u = parse_passwd("root:x:0:0:root:/root:/bin/bash\nbaymax:x:1000:1000::/home/baymax:/bin/bash\n");
        assert_eq!(u[&1000], "baymax");
        assert_eq!(u[&0], "root");
    }
}
