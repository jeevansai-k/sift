//! freedesktop.org Trash specification (v1.0).
//!
//! Items on the home filesystem go to `$XDG_DATA_HOME/Trash`; items on other
//! mounts go to `$topdir/.Trash/$uid` (if the admin created a sticky
//! `.Trash`) or `$topdir/.Trash-$uid`. Every trashed item gets a `.trashinfo`
//! so the file manager can restore it. Sift never copies across filesystems
//! to "trash" something: that would silently become a slow permanent delete.

use crate::cancel::CancelToken;
use crate::envelope::EngineError;
use crate::fsutil;
use crate::paths::Paths;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct Trashed {
    pub original: PathBuf,
    pub trash_file: PathBuf,
    pub info_file: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Trash {
    home_trash: PathBuf,
    uid: u32,
}

impl Trash {
    pub fn new(paths: &Paths) -> Trash {
        Trash { home_trash: paths.home_trash(), uid: crate::current_uid() }
    }

    pub fn home_trash(&self) -> &Path {
        &self.home_trash
    }

    /// Move `path` to the appropriate trash can.
    pub fn trash(&self, path: &Path) -> Result<Trashed, EngineError> {
        let meta = fs::symlink_metadata(path)
            .map_err(|_| EngineError::new("missing", format!("{} no longer exists", path.display())))?;
        let name = path
            .file_name()
            .ok_or_else(|| EngineError::new("refused", "cannot trash a filesystem root"))?
            .to_owned();
        let (trash_dir, topdir) = self.trash_dir_for(path, meta.dev())?;
        let files = trash_dir.join("files");
        let info = trash_dir.join("info");
        for d in [&trash_dir, &files, &info] {
            ensure_private_dir(d)?;
        }

        // Reserve a unique name by creating the .trashinfo exclusively.
        let date = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        let recorded = match &topdir {
            Some(top) => path.strip_prefix(top).unwrap_or(path).to_path_buf(),
            None => path.to_path_buf(),
        };
        let body = format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            percent_encode(recorded.as_os_str().as_bytes()),
            date
        );
        let base = name.to_string_lossy().into_owned();
        let mut chosen: Option<(String, PathBuf)> = None;
        for n in 1..10_000u32 {
            let candidate = if n == 1 { base.clone() } else { numbered(&base, n) };
            let info_path = info.join(format!("{candidate}.trashinfo"));
            if fs::symlink_metadata(files.join(&candidate)).is_ok() {
                continue;
            }
            match OpenOptions::new().write(true).create_new(true).open(&info_path) {
                Ok(mut f) => {
                    f.write_all(body.as_bytes())?;
                    f.sync_all().ok();
                    chosen = Some((candidate, info_path));
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        let (candidate, info_path) =
            chosen.ok_or_else(|| EngineError::new("io", "could not find a free name in the Trash"))?;
        let dest = files.join(&candidate);
        let mut result = fs::rename(path, &dest);
        if let Err(e) = &result {
            // Moving a read-only directory needs write permission on it.
            if e.kind() == io::ErrorKind::PermissionDenied && meta.is_dir() && fsutil::make_dir_writable(path) {
                result = fs::rename(path, &dest);
            }
        }
        if let Err(e) = result {
            let _ = fs::remove_file(&info_path);
            return Err(if e.raw_os_error() == Some(libc::EXDEV) {
                EngineError::new("cross_device", "item is on a different filesystem from its Trash")
            } else {
                e.into()
            });
        }
        Ok(Trashed { original: path.to_path_buf(), trash_file: dest, info_file: info_path })
    }

    /// Pick the trash can for an item on device `dev`.
    fn trash_dir_for(&self, path: &Path, dev: u64) -> Result<(PathBuf, Option<PathBuf>), EngineError> {
        ensure_private_dir(&self.home_trash)?;
        let home_dev = fs::metadata(&self.home_trash)?.dev();
        if home_dev == dev {
            return Ok((self.home_trash.clone(), None));
        }
        let top = mount_top(path, dev);
        // $topdir/.Trash/$uid — only if .Trash is a real sticky directory.
        let shared = top.join(".Trash");
        if let Ok(m) = fs::symlink_metadata(&shared) {
            if m.is_dir() && m.permissions().mode() & 0o1000 != 0 {
                let d = shared.join(self.uid.to_string());
                if ensure_private_dir(&d).is_ok() {
                    return Ok((d, Some(top)));
                }
            }
        }
        let own = top.join(format!(".Trash-{}", self.uid));
        match ensure_private_dir(&own) {
            Ok(()) => Ok((own, Some(top))),
            Err(_) => Err(EngineError::new(
                "cross_device",
                format!("no usable Trash on the filesystem mounted at {}", top.display()),
            )),
        }
    }

    /// Total allocated size of the home Trash.
    pub fn size(&self) -> fsutil::DuInfo {
        let files = self.home_trash.join("files");
        if !files.exists() {
            return fsutil::DuInfo::default();
        }
        let mut info = fsutil::du(&files, None);
        // Don't count the files/ directory inode itself.
        info.dirs = info.dirs.saturating_sub(1);
        info
    }

    /// Number of top-level items in the home Trash.
    pub fn count(&self) -> usize {
        fs::read_dir(self.home_trash.join("files")).map(|r| r.count()).unwrap_or(0)
    }

    /// Permanently delete everything in the home Trash. Returns (bytes, items).
    pub fn empty(&self, cancel: Option<&CancelToken>) -> Result<(u64, u64), EngineError> {
        let files = self.home_trash.join("files");
        let info = self.home_trash.join("info");
        let mut bytes = 0u64;
        let mut items = 0u64;
        if let Ok(rd) = fs::read_dir(&files) {
            for e in rd.flatten() {
                if cancel.map(|c| c.is_cancelled()).unwrap_or(false) {
                    break;
                }
                let p = e.path();
                let size = fsutil::du(&p, None).bytes;
                if fsutil::remove_permanently(&p).is_ok() {
                    bytes += size;
                    items += 1;
                    let mut info_name = e.file_name();
                    info_name.push(".trashinfo");
                    let _ = fs::remove_file(info.join(info_name));
                }
            }
        }
        // Orphaned .trashinfo files and the optional size cache.
        if let Ok(rd) = fs::read_dir(&info) {
            for e in rd.flatten() {
                let stem = e.file_name().to_string_lossy().trim_end_matches(".trashinfo").to_string();
                if fs::symlink_metadata(files.join(&stem)).is_err() {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
        let _ = fs::remove_file(self.home_trash.join("directorysizes"));
        let expunged = self.home_trash.join("expunged");
        if expunged.exists() {
            let _ = fsutil::remove_permanently(&expunged);
        }
        Ok((bytes, items))
    }
}

fn ensure_private_dir(d: &Path) -> Result<(), EngineError> {
    match fs::symlink_metadata(d) {
        Ok(m) if m.file_type().is_symlink() => {
            Err(EngineError::new("refused", format!("{} is a symlink", d.display())))
        }
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(EngineError::new("refused", format!("{} is not a directory", d.display()))),
        Err(_) => {
            fs::DirBuilder::new().recursive(true).mode(0o700).create(d)?;
            Ok(())
        }
    }
}

/// The mount point containing `path` (walk up while the device stays the same).
fn mount_top(path: &Path, dev: u64) -> PathBuf {
    let mut top = path.parent().unwrap_or(Path::new("/")).to_path_buf();
    while let Some(parent) = top.parent() {
        match fs::metadata(parent) {
            Ok(m) if m.dev() == dev => top = parent.to_path_buf(),
            _ => break,
        }
    }
    top
}

fn numbered(base: &str, n: u32) -> String {
    // "report.pdf" -> "report.2.pdf"; dotfiles and extensionless get a suffix.
    match base.rfind('.') {
        Some(i) if i > 0 => format!("{}.{}{}", &base[..i], n, &base[i..]),
        _ => format!("{base}.{n}"),
    }
}

/// RFC 2396 escaping as the spec requires for `Path=`.
pub fn percent_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trashes_with_trashinfo_and_unique_names() {
        let d = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(d.path()).unwrap();
        let paths = Paths::for_home(home.clone(), true);
        let t = Trash::new(&paths);
        fs::create_dir_all(home.join(".cache/a b")).unwrap();
        fs::write(home.join(".cache/a b/f"), b"hello").unwrap();
        let r = t.trash(&home.join(".cache/a b")).unwrap();
        assert!(!home.join(".cache/a b").exists());
        assert!(r.trash_file.join("f").exists());
        let info = fs::read_to_string(&r.info_file).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="));
        assert!(info.contains("/.cache/a%20b\n"));
        assert!(info.contains("DeletionDate="));

        fs::create_dir_all(home.join(".cache/a b")).unwrap();
        let r2 = t.trash(&home.join(".cache/a b")).unwrap();
        assert_ne!(r.trash_file, r2.trash_file);
        assert_eq!(t.count(), 2);

        let (bytes, items) = t.empty(None).unwrap();
        assert_eq!(items, 2);
        assert!(bytes > 0);
        assert_eq!(t.count(), 0);
        assert_eq!(fs::read_dir(home.join(".local/share/Trash/info")).unwrap().count(), 0);
    }

    #[test]
    fn trashes_read_only_directory() {
        let d = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(d.path()).unwrap();
        let paths = Paths::for_home(home.clone(), true);
        let ro = home.join("go/pkg/mod/x@v1");
        fs::create_dir_all(&ro).unwrap();
        fs::write(ro.join("f.go"), b"package x").unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
        let r = Trash::new(&paths).trash(&ro).unwrap();
        assert!(r.trash_file.join("f.go").exists());
    }

    #[test]
    fn numbering_keeps_extension() {
        assert_eq!(numbered("report.pdf", 2), "report.2.pdf");
        assert_eq!(numbered(".bashrc", 3), ".bashrc.3");
        assert_eq!(numbered("cache2", 2), "cache2.2");
        assert_eq!(percent_encode(b"/home/u/a b%"), "/home/u/a%20b%25");
    }
}
