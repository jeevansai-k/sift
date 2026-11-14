//! Disks: mounted filesystems (mountinfo + statvfs) and I/O (diskstats).

use super::Disk;
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct MountEntry {
    pub mount_point: String,
    pub fstype: String,
    pub source: String,
}

const REAL_FS: &[&str] = &[
    "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "zfs", "bcachefs",
    "jfs", "reiserfs", "hfsplus",
];

/// Decode mountinfo's octal escapes (`\040` = space).
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else { continue };
        let l: Vec<&str> = left.split_whitespace().collect();
        let r: Vec<&str> = right.split_whitespace().collect();
        if l.len() < 5 || r.len() < 2 {
            continue;
        }
        out.push(MountEntry { mount_point: unescape(l[4]), fstype: r[0].to_string(), source: unescape(r[1]) });
    }
    out
}

/// Real, user-relevant filesystems: one entry per device (shortest mount point).
pub fn interesting_mounts(entries: &[MountEntry]) -> Vec<MountEntry> {
    let skip_prefix = ["/boot", "/efi", "/snap", "/var/snap", "/var/lib/docker", "/proc", "/sys", "/dev"];
    let mut by_dev: HashMap<String, MountEntry> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for e in entries {
        if !REAL_FS.contains(&e.fstype.as_str()) {
            continue;
        }
        if !(e.source.starts_with("/dev/") || e.fstype == "zfs") {
            continue;
        }
        if skip_prefix.iter().any(|p| e.mount_point == *p || e.mount_point.starts_with(&format!("{p}/"))) {
            continue;
        }
        if e.mount_point.starts_with("/run/") && !e.mount_point.starts_with("/run/media/") {
            continue;
        }
        match by_dev.get(&e.source) {
            Some(prev) if prev.mount_point.len() <= e.mount_point.len() => {}
            Some(_) => {
                by_dev.insert(e.source.clone(), e.clone());
            }
            None => {
                order.push(e.source.clone());
                by_dev.insert(e.source.clone(), e.clone());
            }
        }
    }
    order.into_iter().filter_map(|d| by_dev.remove(&d)).collect()
}

/// (total, used, available-to-user) bytes for the filesystem holding `path`.
pub fn statvfs(path: &Path) -> Option<(u64, u64, u64)> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: zeroed statvfs is a valid out-parameter; c is NUL-terminated.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return None;
    }
    let frsize = st.f_frsize as u64;
    let total = st.f_blocks as u64 * frsize;
    let used = (st.f_blocks as u64).saturating_sub(st.f_bfree as u64) * frsize;
    let avail = st.f_bavail as u64 * frsize;
    Some((total, used, avail))
}

pub fn list_disks(mountinfo: &str) -> Vec<Disk> {
    let mut disks: Vec<Disk> = interesting_mounts(&parse_mountinfo(mountinfo))
        .into_iter()
        .filter_map(|m| {
            let (total, used, free) = statvfs(Path::new(&m.mount_point))?;
            if total == 0 {
                return None;
            }
            let denom = (used + free).max(1);
            Some(Disk {
                removable: m.mount_point.starts_with("/media/") || m.mount_point.starts_with("/run/media/"),
                used_pct: used as f64 / denom as f64 * 100.0,
                mount: m.mount_point,
                device: m.source,
                fstype: m.fstype,
                total,
                used,
                free,
            })
        })
        .collect();
    // "/" first, then by mount point.
    disks.sort_by(|a, b| (a.mount != "/").cmp(&(b.mount != "/")).then(a.mount.cmp(&b.mount)));
    disks
}

/// Whole physical disks (have a backing device; excludes loop, zram, dm, partitions).
pub fn physical_block_devices() -> HashSet<String> {
    let mut set = HashSet::new();
    if let Ok(rd) = fs::read_dir("/sys/block") {
        for e in rd.flatten() {
            if e.path().join("device").exists() {
                set.insert(e.file_name().to_string_lossy().into_owned());
            }
        }
    }
    set
}

/// Sum of sectors (read, written) across the given devices.
pub fn sum_diskstats(text: &str, devices: &HashSet<String>) -> (u64, u64) {
    let mut r = 0u64;
    let mut w = 0u64;
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || !devices.contains(f[2]) {
            continue;
        }
        r += f[5].parse::<u64>().unwrap_or(0);
        w += f[9].parse::<u64>().unwrap_or(0);
    }
    (r, w)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MI: &str = "\
22 1 259:9 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p9 rw,errors=remount-ro
30 22 259:1 / /boot/efi rw,relatime shared:3 - vfat /dev/nvme0n1p1 rw
40 22 0:50 / /proc rw - proc proc rw
41 22 7:3 / /snap/core20/2866 ro,nodev - squashfs /dev/loop3 ro
50 22 8:17 / /media/u/My\\040Disk rw - exfat /dev/sdb1 rw
51 22 259:9 /home /home2 rw - ext4 /dev/nvme0n1p9 rw
";

    #[test]
    fn mounts() {
        let all = parse_mountinfo(MI);
        assert_eq!(all.len(), 6);
        let m = interesting_mounts(&all);
        let points: Vec<&str> = m.iter().map(|e| e.mount_point.as_str()).collect();
        assert_eq!(points, vec!["/", "/media/u/My Disk"]);
    }

    #[test]
    fn diskstats() {
        let text = "259 0 nvme0n1 100 0 2000 0 50 0 4000 0 0 0 0\n259 1 nvme0n1p1 10 0 200 0 5 0 400 0 0 0 0\n7 0 loop0 5 0 50 0 0 0 0 0 0 0 0\n";
        let devs: HashSet<String> = ["nvme0n1".to_string()].into_iter().collect();
        assert_eq!(sum_diskstats(text, &devs), (2000, 4000));
    }

    #[test]
    fn statvfs_root_works() {
        let (t, u, a) = statvfs(Path::new("/")).unwrap();
        assert!(t > 0 && u <= t && a <= t);
    }
}
