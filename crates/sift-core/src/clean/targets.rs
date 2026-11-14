//! The Linux clean target table. Every path `clean` may remove at user level
//! is named here; a reviewed plan entry that no row could have produced is
//! refused as `not_a_clean_target`.
//!
//! Patterns are home-relative with `*` wildcards per component. `~/.cache`
//! is "non-essential cached data" by the XDG spec; anything outside it is
//! listed explicitly and only where the folder is a well-known cache.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Browsers,
    Developer,
    Apps,
    Sandboxed,
    Thumbnails,
    Logs,
    Trash,
    System,
}

impl Group {
    pub fn title(&self) -> &'static str {
        match self {
            Group::Browsers => "Browser caches",
            Group::Developer => "Developer caches",
            Group::Apps => "App caches",
            Group::Sandboxed => "Flatpak & Snap app caches",
            Group::Thumbnails => "Thumbnails",
            Group::Logs => "Logs & crash reports",
            Group::Trash => "Trash",
            Group::System => "System (needs admin)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Remove the matched path itself.
    Whole,
    /// Remove everything inside the matched folder, keep the folder.
    Contents,
    /// Remove files inside the folder older than N days.
    OlderThan(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guarding {
    /// Skip while any of these process names runs.
    Procs(&'static [&'static str]),
    /// Path component 2 is a Flatpak app id (`.var/app/<id>/cache`).
    Flatpak,
    /// Path component 1 is a snap name (`snap/<name>/common/.cache`).
    Snap,
    Unguarded,
}

#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub id: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub patterns: &'static [&'static str],
    pub mode: Mode,
    pub default_on: bool,
    pub guarding: Guarding,
    pub note: &'static str,
}

const fn t(
    id: &'static str,
    label: &'static str,
    group: Group,
    patterns: &'static [&'static str],
    mode: Mode,
    default_on: bool,
    guarding: Guarding,
    note: &'static str,
) -> Target {
    Target { id, label, group, patterns, mode, default_on, guarding, note }
}

use Group::*;
use Guarding::*;
use Mode::*;

/// Order matters: the first target covering a path owns it.
pub const TARGETS: &[Target] = &[
    // Browsers — disk caches only; profiles (~/.mozilla, ~/.config/<browser>) are protected.
    t("firefox", "Firefox cache", Browsers, &[".cache/mozilla", "snap/firefox/common/.cache"], Contents, true,
      Procs(&["firefox", "firefox-bin", "firefox-esr"]), "Bookmarks, passwords and history live elsewhere and are not touched"),
    t("chrome", "Google Chrome cache", Browsers, &[".cache/google-chrome"], Contents, true, Procs(&["chrome", "google-chrome"]), ""),
    t("chromium", "Chromium cache", Browsers, &[".cache/chromium", "snap/chromium/common/.cache"], Contents, true,
      Procs(&["chromium", "chromium-browser", "chrome"]), ""),
    t("brave", "Brave cache", Browsers, &[".cache/BraveSoftware"], Contents, true, Procs(&["brave", "brave-browser"]), ""),
    t("vivaldi", "Vivaldi cache", Browsers, &[".cache/vivaldi"], Contents, true, Procs(&["vivaldi-bin", "vivaldi"]), ""),
    t("edge", "Microsoft Edge cache", Browsers, &[".cache/microsoft-edge"], Contents, true, Procs(&["msedge", "microsoft-edge"]), ""),
    t("opera", "Opera cache", Browsers, &[".cache/opera"], Contents, true, Procs(&["opera"]), ""),
    // Developer tools
    t("pip", "pip cache", Developer, &[".cache/pip"], Contents, true, Procs(&["pip", "pip3"]), ""),
    t("uv", "uv cache", Developer, &[".cache/uv"], Contents, true, Procs(&["uv"]), ""),
    t("poetry", "Poetry cache", Developer, &[".cache/pypoetry/cache", ".cache/pypoetry/artifacts"], Whole, true, Procs(&["poetry"]), ""),
    t("npm", "npm cache", Developer, &[".npm/_cacache", ".npm/_npx", ".npm/_logs"], Whole, true, Procs(&["npm", "npx"]), ""),
    t("yarn", "Yarn cache", Developer, &[".cache/yarn"], Contents, true, Procs(&["yarn"]), ""),
    t("pnpm", "pnpm store", Developer, &[".local/share/pnpm/store", ".cache/pnpm"], Whole, false, Procs(&["pnpm"]),
      "Projects link into this store; `pnpm store prune` is the gentler option"),
    t("bun", "Bun cache", Developer, &[".bun/install/cache"], Contents, true, Procs(&["bun"]), ""),
    t("deno", "Deno cache", Developer, &[".cache/deno"], Contents, true, Procs(&["deno"]), ""),
    t("go-build", "Go build cache", Developer, &[".cache/go-build"], Contents, true, Procs(&["go"]), ""),
    t("cargo", "Cargo download cache", Developer, &[".cargo/registry/cache"], Whole, true, Procs(&["cargo"]), ""),
    t("cargo-src", "Cargo extracted sources", Developer, &[".cargo/registry/src", ".cargo/git/checkouts"], Whole, false,
      Procs(&["cargo"]), "Re-extracted or re-cloned on the next build"),
    t("gradle", "Gradle caches", Developer, &[".gradle/caches"], Whole, false, Procs(&["java"]), "Large re-download on the next build"),
    t("maven", "Maven repository", Developer, &[".m2/repository"], Whole, false, Procs(&["java", "mvn"]), "Large re-download on the next build"),
    t("node-gyp", "node-gyp headers", Developer, &[".cache/node-gyp"], Contents, true, Unguarded, ""),
    t("electron-dl", "Electron downloads", Developer, &[".cache/electron", ".cache/electron-builder"], Contents, true, Unguarded, ""),
    t("playwright", "Playwright browsers", Developer, &[".cache/ms-playwright", ".cache/ms-playwright-go"], Contents, false, Unguarded,
      "Browser binaries for tests; large re-download"),
    t("puppeteer", "Puppeteer browsers", Developer, &[".cache/puppeteer"], Contents, false, Unguarded, "Browser binaries for tests"),
    t("huggingface", "Hugging Face models", Developer, &[".cache/huggingface"], Contents, false, Unguarded,
      "Downloaded AI models; re-downloading can take many GB"),
    t("torch", "PyTorch hub cache", Developer, &[".cache/torch"], Contents, false, Unguarded, "Downloaded model weights"),
    t("next-swc", "Next.js SWC cache", Developer, &[".cache/next-swc"], Contents, true, Unguarded, ""),
    t("typescript", "TypeScript cache", Developer, &[".cache/typescript"], Contents, true, Unguarded, ""),
    t("composer", "Composer cache", Developer, &[".cache/composer"], Contents, true, Procs(&["composer"]), ""),
    t("jetbrains", "JetBrains IDE caches", Developer, &[".cache/JetBrains"], Contents, false, Unguarded, "IDEs re-index on next start"),
    t("ccache", "Compiler caches", Developer, &[".cache/ccache", ".ccache", ".cache/sccache"], Contents, false, Unguarded,
      "Speeds up rebuilds; removing it makes the next build slower"),
    // Apps
    t("gl-cache", "GPU shader caches", Apps, &[".nv/GLCache", ".nv/ComputeCache"], Contents, true, Unguarded, "Rebuilt automatically"),
    t("electron-cache", "App web caches", Apps,
      &[".config/*/Cache", ".config/*/Code Cache", ".config/*/GPUCache", ".config/*/DawnCache", ".config/*/DawnGraphiteCache",
        ".config/*/DawnWebGPUCache", ".config/*/GrShaderCache", ".config/*/ShaderCache"],
      Whole, true, Unguarded, "Electron/Chromium HTTP and shader caches (VS Code, Slack, Discord, …)"),
    t("app-cache", "App cache", Apps, &[".cache/*"], Whole, true, Unguarded, ""),
    // Sandboxed
    t("flatpak-cache", "Flatpak app cache", Sandboxed, &[".var/app/*/cache"], Contents, true, Flatpak, ""),
    t("snap-cache", "Snap app cache", Sandboxed, &["snap/*/common/.cache"], Contents, true, Snap, ""),
    // Thumbnails
    t("thumbnails", "Thumbnail cache", Thumbnails, &[".cache/thumbnails", ".thumbnails"], Contents, true, Unguarded,
      "File managers recreate thumbnails as needed"),
    // Logs
    t("xsession-old", "Old session log", Logs, &[".xsession-errors.old"], Whole, true, Unguarded, ""),
    t("xorg-logs", "Old Xorg logs", Logs, &[".local/share/xorg"], OlderThan(7), true, Unguarded, ""),
];

/// Names under `~/.cache` the generic `app-cache` row never offers: owned by
/// a more specific row above, or not safe to drop wholesale.
pub const APP_CACHE_EXCLUDES: &[&str] = &[
    // owned by specific rows
    "mozilla", "google-chrome", "chromium", "BraveSoftware", "vivaldi", "microsoft-edge", "opera", "pip", "uv",
    "pypoetry", "yarn", "pnpm", "deno", "go-build", "node-gyp", "electron", "electron-builder", "ms-playwright",
    "ms-playwright-go", "puppeteer", "huggingface", "torch", "next-swc", "typescript", "composer", "JetBrains",
    "ccache", "sccache", "thumbnails",
    // never wholesale
    "sift", "dconf", "flatpak", "tracker", "tracker3", "evolution", "keyring", "gnome-keyring", "ibus", "libvirt",
    "gnome-boxes", "containers", "docker", "kwallet", "doc",
];

pub fn find(id: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|t| t.id == id)
}

pub fn app_cache_excluded(name: &str) -> bool {
    APP_CACHE_EXCLUDES.contains(&name) || name.starts_with("keyring-") || name.starts_with('.')
}
