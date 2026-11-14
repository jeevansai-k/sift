//! Register the bundled fonts (Geist, Geist Mono, Cal Sans — SIL OFL 1.1)
//! with fontconfig for this process only, before GTK starts. If anything
//! fails, the stylesheet's fallbacks (Inter, Cantarell, monospace) apply.

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[link(name = "fontconfig")]
extern "C" {
    fn FcConfigAppFontAddFile(config: *mut std::ffi::c_void, file: *const u8) -> i32;
}

const FONTS: &[(&str, &[u8])] = &[
    ("Geist-Regular.ttf", include_bytes!("../../../data/fonts/Geist-Regular.ttf")),
    ("Geist-Medium.ttf", include_bytes!("../../../data/fonts/Geist-Medium.ttf")),
    ("Geist-SemiBold.ttf", include_bytes!("../../../data/fonts/Geist-SemiBold.ttf")),
    ("GeistMono-Regular.ttf", include_bytes!("../../../data/fonts/GeistMono-Regular.ttf")),
    ("GeistMono-Medium.ttf", include_bytes!("../../../data/fonts/GeistMono-Medium.ttf")),
    ("GeistMono-SemiBold.ttf", include_bytes!("../../../data/fonts/GeistMono-SemiBold.ttf")),
    ("GeistMono-Bold.ttf", include_bytes!("../../../data/fonts/GeistMono-Bold.ttf")),
    ("CalSans.ttf", include_bytes!("../../../data/fonts/CalSans.ttf")),
];

pub fn register(cache_dir: &Path) {
    let dir = cache_dir.join("fonts");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (name, bytes) in FONTS {
        let path = dir.join(name);
        let stale = fs::metadata(&path).map(|m| m.len() != bytes.len() as u64).unwrap_or(true);
        if stale && fs::write(&path, bytes).is_err() {
            continue;
        }
        if let Ok(c) = CString::new(path.as_os_str().as_bytes()) {
            // SAFETY: NULL config = the current fontconfig config; c is a valid C string.
            unsafe {
                FcConfigAppFontAddFile(std::ptr::null_mut(), c.as_ptr() as *const u8);
            }
        }
    }
}
