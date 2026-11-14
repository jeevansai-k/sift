//! Purge: build artifacts in your projects (`node_modules`, `target/`, …).
//! An artifact folder only counts when the project marker that produces it
//! sits next to it, so a random folder called `build` is never offered.

use crate::cancel::CancelToken;
use crate::ctx::Ctx;
use crate::fsutil::{self, wildcard_match};
use crate::ops::OpLog;
use crate::removal::{Event, Remover, Summary};
use serde::Serialize;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub struct Rule {
    /// Artifact folder name (may contain `*`).
    pub name: &'static str,
    /// Any of these siblings marks a real project (`*` allowed). Empty = always.
    pub markers: &'static [&'static str],
    pub kind: &'static str,
    pub default_on: bool,
}

pub const RULES: &[Rule] = &[
    Rule { name: "node_modules", markers: &["package.json"], kind: "Node.js dependencies", default_on: true },
    Rule { name: "target", markers: &["Cargo.toml"], kind: "Rust build output", default_on: true },
    Rule { name: "target", markers: &["pom.xml"], kind: "Maven build output", default_on: true },
    Rule { name: "build", markers: &["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"], kind: "Gradle build output", default_on: true },
    Rule { name: "build", markers: &["CMakeLists.txt", "meson.build"], kind: "C/C++ build output", default_on: true },
    Rule { name: "build", markers: &["pubspec.yaml"], kind: "Flutter build output", default_on: true },
    Rule { name: "build", markers: &["package.json", "setup.py", "pyproject.toml"], kind: "Build output", default_on: true },
    Rule { name: "dist", markers: &["package.json", "setup.py", "pyproject.toml"], kind: "Distribution output", default_on: true },
    Rule { name: "out", markers: &["next.config.js", "next.config.mjs", "next.config.ts"], kind: "Next.js export", default_on: true },
    Rule { name: ".next", markers: &["package.json"], kind: "Next.js cache", default_on: true },
    Rule { name: ".nuxt", markers: &["package.json"], kind: "Nuxt build", default_on: true },
    Rule { name: ".svelte-kit", markers: &["package.json"], kind: "SvelteKit build", default_on: true },
    Rule { name: ".turbo", markers: &["package.json", "turbo.json"], kind: "Turborepo cache", default_on: true },
    Rule { name: ".parcel-cache", markers: &["package.json"], kind: "Parcel cache", default_on: true },
    Rule { name: ".angular", markers: &["angular.json"], kind: "Angular cache", default_on: true },
    Rule { name: ".expo", markers: &["package.json"], kind: "Expo cache", default_on: true },
    Rule { name: ".docusaurus", markers: &["package.json"], kind: "Docusaurus cache", default_on: true },
    Rule { name: "coverage", markers: &["package.json"], kind: "Coverage reports", default_on: true },
    Rule { name: ".nyc_output", markers: &["package.json"], kind: "Coverage data", default_on: true },
    Rule { name: "__pycache__", markers: &[], kind: "Python bytecode", default_on: true },
    Rule { name: ".pytest_cache", markers: &[], kind: "pytest cache", default_on: true },
    Rule { name: ".mypy_cache", markers: &[], kind: "mypy cache", default_on: true },
    Rule { name: ".ruff_cache", markers: &[], kind: "Ruff cache", default_on: true },
    Rule { name: ".tox", markers: &["tox.ini", "pyproject.toml", "setup.cfg"], kind: "tox environments", default_on: true },
    Rule { name: ".nox", markers: &["noxfile.py"], kind: "nox environments", default_on: true },
    Rule { name: ".venv", markers: &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile", "poetry.lock", "uv.lock"], kind: "Python virtualenv", default_on: false },
    Rule { name: "venv", markers: &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile", "poetry.lock", "uv.lock"], kind: "Python virtualenv", default_on: false },
    Rule { name: ".gradle", markers: &["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"], kind: "Gradle project cache", default_on: true },
    Rule { name: "cmake-build-*", markers: &["CMakeLists.txt"], kind: "CLion build output", default_on: true },
    Rule { name: ".cxx", markers: &["build.gradle", "build.gradle.kts"], kind: "Android native build", default_on: true },
    Rule { name: ".dart_tool", markers: &["pubspec.yaml"], kind: "Dart tool cache", default_on: true },
    Rule { name: "zig-cache", markers: &["build.zig"], kind: "Zig cache", default_on: true },
    Rule { name: ".zig-cache", markers: &["build.zig"], kind: "Zig cache", default_on: true },
    Rule { name: "zig-out", markers: &["build.zig"], kind: "Zig output", default_on: true },
    Rule { name: "_build", markers: &["mix.exs", "dune-project", "rebar.config"], kind: "Elixir/OCaml build", default_on: true },
    Rule { name: "deps", markers: &["mix.exs"], kind: "Elixir dependencies", default_on: true },
    Rule { name: ".stack-work", markers: &["stack.yaml"], kind: "Haskell Stack build", default_on: true },
    Rule { name: "dist-newstyle", markers: &["cabal.project", "*.cabal"], kind: "Cabal build", default_on: true },
    Rule { name: "obj", markers: &["*.csproj", "*.fsproj", "*.vbproj"], kind: ".NET intermediate", default_on: true },
    Rule { name: "bin", markers: &["*.csproj", "*.fsproj", "*.vbproj"], kind: ".NET build output", default_on: true },
    Rule { name: ".terraform", markers: &["*.tf"], kind: "Terraform providers", default_on: false },
];

/// Never descend into these (relative to home).
const SKIP_UNDER_HOME: &[&str] = &[
    ".cache", ".local", ".config", ".var", "snap", ".npm", ".cargo", ".rustup", "go", ".nvm", ".gradle", ".m2",
    ".mozilla", ".steam", ".wine", ".pyenv", ".sdkman", ".bun", ".deno", ".vscode", ".vscode-server", ".cursor",
    ".android", ".java", ".dotnet", ".nuget", ".conda", "anaconda3", "miniconda3", ".gem", ".rbenv", ".opam",
];

const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug, Serialize)]
pub struct Artifact {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    pub kind: String,
    pub project: PathBuf,
    pub project_name: String,
    pub bytes: u64,
    pub files: u64,
    /// Most recent activity in the project (unix seconds).
    pub project_mtime: i64,
    pub recent: bool,
    pub selected: bool,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct PurgeScan {
    pub roots: Vec<PathBuf>,
    pub items: Vec<Artifact>,
    pub total_bytes: u64,
    pub selected_bytes: u64,
}

fn sibling_matches(dir: &Path, names: &[String], marker: &str) -> bool {
    if marker.contains('*') {
        names.iter().any(|n| wildcard_match(marker, n))
    } else {
        names.iter().any(|n| n == marker) || dir.join(marker).exists()
    }
}

/// The rule that makes `name` inside `dir` an artifact, if any.
pub fn match_rule<'a>(dir: &Path, sibling_names: &[String], name: &str) -> Option<&'a Rule> {
    RULES.iter().find(|r| {
        wildcard_match(r.name, name) && (r.markers.is_empty() || r.markers.iter().any(|m| sibling_matches(dir, sibling_names, m)))
    })
}

fn project_mtime(project: &Path) -> i64 {
    let mut newest = fs::symlink_metadata(project).map(|m| m.mtime()).unwrap_or(0);
    for f in [".git/index", ".git/HEAD", "package.json", "Cargo.toml", "pyproject.toml", "Cargo.lock", "package-lock.json"] {
        if let Ok(m) = fs::symlink_metadata(project.join(f)) {
            newest = newest.max(m.mtime());
        }
    }
    newest
}

fn names_in(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}

pub fn scan(ctx: &Ctx, roots: &[PathBuf], cancel: &CancelToken, on_item: &mut dyn FnMut(&Artifact)) -> PurgeScan {
    let home = ctx.paths.home.clone();
    let guard = ctx.guard();
    let recent_cutoff = crate::now_unix() - ctx.settings.purge_recent_days as i64 * 86_400;
    let skip: Vec<PathBuf> = SKIP_UNDER_HOME.iter().map(|s| home.join(s)).chain([ctx.paths.home_trash()]).collect();
    let mut scan = PurgeScan { roots: roots.to_vec(), ..Default::default() };

    for root in roots {
        let Ok(root_meta) = fs::symlink_metadata(root) else { continue };
        if !root_meta.is_dir() {
            continue;
        }
        let dev = root_meta.dev();
        let mut stack: Vec<(PathBuf, usize)> = vec![(root.clone(), 0)];
        while let Some((dir, depth)) = stack.pop() {
            if cancel.is_cancelled() {
                break;
            }
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            let entries: Vec<_> = rd.flatten().collect();
            let names: Vec<String> = entries.iter().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            for e in entries {
                let Ok(ft) = e.file_type() else { continue };
                if !ft.is_dir() || ft.is_symlink() {
                    continue;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                let path = e.path();
                if let Some(rule) = match_rule(&dir, &names, &name) {
                    if guard.check_lexical(&path, roots).is_err() {
                        continue;
                    }
                    let du = fsutil::du(&path, Some(cancel));
                    if du.bytes == 0 {
                        continue;
                    }
                    let pm = project_mtime(&dir);
                    let recent = pm >= recent_cutoff;
                    let item = Artifact {
                        id: path.display().to_string(),
                        name: name.clone(),
                        kind: rule.kind.into(),
                        project_name: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                        project: dir.clone(),
                        bytes: du.bytes,
                        files: du.files,
                        project_mtime: pm,
                        recent,
                        selected: rule.default_on && !recent,
                        path,
                    };
                    on_item(&item);
                    scan.items.push(item);
                    continue; // never descend into an artifact
                }
                if depth + 1 >= MAX_DEPTH
                    || name == "node_modules"
                    || name == ".git"
                    || name == "site-packages"
                    || name == "dist-packages"
                    || path.join("pyvenv.cfg").exists()
                {
                    // Dependencies and virtualenv internals aren't your projects.
                    continue;
                }
                if name.starts_with('.') && depth == 0 && dir == home {
                    // Hidden folders at the top of home are app data, not projects.
                    continue;
                }
                if skip.iter().any(|s| path.starts_with(s)) {
                    continue;
                }
                if let Ok(m) = e.metadata() {
                    if m.dev() != dev {
                        continue;
                    }
                }
                stack.push((path, depth + 1));
            }
        }
    }
    scan.items.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    scan.total_bytes = scan.items.iter().map(|i| i.bytes).sum();
    scan.selected_bytes = scan.items.iter().filter(|i| i.selected).map(|i| i.bytes).sum();
    scan
}

/// Re-check a reviewed artifact: it must still be an artifact by the same
/// rules (marker present, inside a root).
pub fn revalidate(path: &Path, roots: &[PathBuf]) -> Result<(), &'static str> {
    if !roots.iter().any(|r| path.starts_with(r) && path != r) {
        return Err("outside the folders Purge scans");
    }
    let dir = path.parent().ok_or("no parent")?;
    let name = path.file_name().ok_or("no name")?.to_string_lossy().into_owned();
    let names = names_in(dir);
    match_rule(dir, &names, &name).map(|_| ()).ok_or("no longer looks like a build artifact (project marker missing)")
}

pub fn apply(
    ctx: &Ctx,
    roots: &[PathBuf],
    paths: &[PathBuf],
    dry_run: bool,
    permanent: bool,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(&Event),
) -> Summary {
    let guard = ctx.guard();
    let trash = ctx.trash();
    let mut log = if dry_run { OpLog::disabled() } else { OpLog::begin(&ctx.paths, "purge", false, &ctx.source) };
    let mut rm = Remover::new(&guard, &trash, permanent, dry_run, &mut log);
    for (i, p) in paths.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        sink(&Event::Progress { done: i, total: paths.len(), current: p.display().to_string() });
        if let Err(why) = revalidate(p, roots) {
            rm.refuse(p, why, sink);
            continue;
        }
        rm.remove(p, roots, false, None, sink);
    }
    let summary = rm.summary.clone();
    sink(&Event::Done(summary.clone()));
    log.finish(if summary.failed > 0 { "partial" } else { "done" }, &summary.headline());
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::runner::FakeRunner;
    use crate::settings::Settings;
    use std::sync::Arc;

    #[test]
    fn finds_marked_artifacts_only() {
        let d = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(d.path()).unwrap();
        let mk = |rel: &str| {
            let p = home.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, vec![1u8; 5000]).unwrap();
        };
        mk("code/web/package.json");
        mk("code/web/node_modules/react/index.js");
        mk("code/web/node_modules/react/node_modules/x/y.js");
        mk("code/rs/Cargo.toml");
        mk("code/rs/target/debug/app");
        mk("code/py/mod/__pycache__/a.pyc");
        mk("Documents/build/notes.txt"); // no marker → not an artifact
        mk(".cache/x/node_modules/a.js"); // skipped area
        let ctx = Ctx::with(Paths::for_home(home.clone(), true), Settings::default(), Arc::new(FakeRunner::default()));
        let roots = vec![home.clone()];
        let s = scan(&ctx, &roots, &CancelToken::new(), &mut |_| {});
        let mut names: Vec<String> = s.items.iter().map(|i| i.path.strip_prefix(&home).unwrap().display().to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["code/py/mod/__pycache__", "code/rs/target", "code/web/node_modules"]);
        // Fresh projects are listed but not pre-selected.
        assert!(s.items.iter().all(|i| i.recent && !i.selected));

        let paths: Vec<PathBuf> = s.items.iter().map(|i| i.path.clone()).collect();
        let bogus = home.join("Documents/build");
        let mut all = paths.clone();
        all.push(bogus.clone());
        let sum = apply(&ctx, &roots, &all, false, false, &CancelToken::new(), &mut |_| {});
        assert_eq!(sum.removed, 3);
        assert_eq!(sum.refused, 1);
        assert!(bogus.exists());
        assert!(!home.join("code/web/node_modules").exists());
        assert!(home.join("code/web/package.json").exists());
    }
}
