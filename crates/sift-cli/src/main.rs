//! `sift` — the engine from the terminal. Human-readable by default,
//! `--json` for one `{ok, data|error}` envelope, `--stream` for NDJSON events.
//! Everything destructive is a dry run unless `--apply` is given.

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use sift_core::cancel::CancelToken;
use sift_core::ctx::Ctx;
use sift_core::envelope::{self, EngineError};
use sift_core::format::{self, bytes, tilde};
use sift_core::removal::{Event, Summary};
use sift_core::{analyze, apps, clean, db, doctor, dupes, installer, optimize, ports, purge, sampler, status};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "sift", version, about = "Clean, uninstall, analyze, optimize and monitor your Linux system")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone, Copy)]
struct Out {
    /// Print one JSON envelope.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Clone)]
struct Act {
    /// Actually remove (default is a dry run).
    #[arg(long)]
    apply: bool,
    /// Delete permanently instead of moving to the Trash.
    #[arg(long)]
    permanent: bool,
    /// Only these reviewed paths: one absolute path per line.
    #[arg(long, value_name = "FILE")]
    plan: Option<PathBuf>,
    /// Include items that are not pre-selected.
    #[arg(long)]
    all: bool,
    /// NDJSON event per line.
    #[arg(long)]
    stream: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Live system status (CPU, memory, disks, network, battery, health).
    Status {
        #[command(flatten)]
        out: Out,
        /// Keep printing one JSON line per interval.
        #[arg(long)]
        watch: bool,
        #[arg(long, default_value_t = 2)]
        interval: u64,
        /// Stop after N frames (with --watch).
        #[arg(long)]
        frames: Option<u64>,
    },
    /// Record history in the background (run by the sift-sampler user service).
    Sampler {
        /// Write one sample and exit.
        #[arg(long)]
        once: bool,
    },
    /// Recorded history and the busiest processes over a window.
    History {
        #[command(flatten)]
        out: Out,
        #[arg(long, default_value_t = 60)]
        minutes: i64,
    },
    /// What Sift has done: scans, cleans, uninstalls.
    Activity {
        #[command(flatten)]
        out: Out,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Caches, logs and other junk (dry run unless --apply).
    Clean {
        #[command(flatten)]
        out: Out,
        #[command(flatten)]
        act: Act,
        /// Also look at admin-only items (APT cache, journal, snaps…).
        #[arg(long)]
        system: bool,
        /// Permanently empty the Trash as part of --apply.
        #[arg(long)]
        empty_trash: bool,
    },
    /// Build artifacts in your projects (node_modules, target/, …).
    Purge {
        #[command(flatten)]
        out: Out,
        #[command(flatten)]
        act: Act,
        /// Where to look (default: settings, else your home).
        #[arg(long = "root")]
        roots: Vec<PathBuf>,
    },
    /// Old installers and archives in Downloads/Desktop.
    Installer {
        #[command(flatten)]
        out: Out,
        #[command(flatten)]
        act: Act,
    },
    /// Safe maintenance tasks.
    Optimize {
        #[command(flatten)]
        out: Out,
        /// Run (default: list what would run).
        #[arg(long)]
        apply: bool,
        /// Only these task ids (default: the pre-selected ones).
        #[arg(long = "task")]
        tasks: Vec<String>,
    },
    /// Installed apps.
    Apps {
        #[command(flatten)]
        out: Out,
    },
    /// Remove an app (id from `sift apps --json`) and, with --leftovers, its data.
    Uninstall {
        #[command(flatten)]
        out: Out,
        app_id: String,
        #[arg(long)]
        apply: bool,
        /// Also move the pre-selected leftover folders to the Trash.
        #[arg(long)]
        leftovers: bool,
    },
    /// What's taking space in a folder.
    Analyze {
        #[command(flatten)]
        out: Out,
        path: Option<PathBuf>,
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Duplicate files.
    Dupes {
        #[command(flatten)]
        out: Out,
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        min_size: Option<u64>,
        #[arg(long)]
        hidden: bool,
    },
    /// Listening TCP/UDP ports and their processes.
    Ports {
        #[command(flatten)]
        out: Out,
    },
    /// Health checks.
    Doctor {
        #[command(flatten)]
        out: Out,
    },
}

fn emit<T: Serialize>(json: bool, command: &str, data: &T, human: impl FnOnce()) {
    if json {
        println!("{}", envelope::ok(command, data));
    } else {
        human();
    }
}

fn fail(json: bool, command: &str, e: &EngineError) -> ExitCode {
    if json {
        println!("{}", envelope::err(command, e));
    } else {
        eprintln!("sift {command}: {}", e.message);
    }
    ExitCode::FAILURE
}

fn stream_or_print(stream: bool) -> impl FnMut(&Event) {
    move |e: &Event| {
        if stream {
            println!("{}", envelope::event_line(e));
            let _ = std::io::stdout().flush();
        } else {
            match e {
                Event::Removed { path, bytes: b, action } => println!("  {action:<7} {:>10}  {path}", bytes(*b)),
                Event::WouldRemove { path, bytes: b } => println!("  would   {:>10}  {path}", bytes(*b)),
                Event::Failed { path, reason } => println!("  FAILED             {path}: {reason}"),
                Event::Refused { path, reason } => println!("  refused            {path}: {reason}"),
                Event::Skipped { path, reason } => println!("  skipped            {path}: {reason}"),
                _ => {}
            }
        }
    }
}

fn print_summary(s: &Summary) {
    println!("\n{}", s.headline());
    if s.failed + s.refused + s.skipped > 0 {
        println!("{} failed, {} refused, {} skipped", s.failed, s.refused, s.skipped);
    }
    if s.dry_run {
        println!("Dry run — nothing was changed. Add --apply to do it.");
    }
}

fn read_plan(p: &PathBuf) -> Result<String, EngineError> {
    let meta = std::fs::symlink_metadata(p)?;
    if !meta.is_file() || meta.len() > 8 * 1024 * 1024 {
        return Err(EngineError::new("invalid", "plan must be a regular file under 8 MiB"));
    }
    Ok(std::fs::read_to_string(p)?)
}

fn plan_paths(text: &str) -> Vec<PathBuf> {
    text.lines().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with('#')).take(4096).map(PathBuf::from).collect()
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ctx = Ctx::detect("cli");
    let cancel = CancelToken::new();
    match cli.cmd {
        Cmd::Status { out, watch, interval, frames } => {
            let mut c = status::Collector::new();
            if watch {
                let mut n = 0;
                loop {
                    let s = c.collect();
                    println!("{}", envelope::event_line(&s));
                    if std::io::stdout().flush().is_err() {
                        break;
                    }
                    n += 1;
                    if frames.map(|f| n >= f).unwrap_or(false) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(interval.max(1)));
                }
                return ExitCode::SUCCESS;
            }
            let s = c.collect();
            emit(out.json, "status", &s, || print_status(&s, &ctx));
        }
        Cmd::Sampler { once } => {
            if let Err(e) = sampler::run_daemon(&ctx.paths, once) {
                return fail(false, "sampler", &e);
            }
        }
        Cmd::History { out, minutes } => {
            let db = match db::Db::open(&ctx.paths.db_file()) {
                Ok(d) => d,
                Err(e) => return fail(out.json, "history", &e),
            };
            let now = sift_core::now_unix();
            let from = now - minutes * 60;
            let samples = db.samples(from, now, 120).unwrap_or_default();
            let top = db.process_usage(from, now, 10, db::UsageMetric::PeakCpu).unwrap_or_default();
            let (count, first, last) = db.sample_stats();
            #[derive(Serialize)]
            struct H<'a> {
                minutes: i64,
                stored_samples: i64,
                first: Option<i64>,
                last: Option<i64>,
                samples: &'a [db::Sample],
                top_processes: &'a [db::ProcUsage],
            }
            let h = H { minutes, stored_samples: count, first, last, samples: &samples, top_processes: &top };
            emit(out.json, "history", &h, || {
                println!("{count} samples stored{}", last.map(|l| format!(", newest {}", format::ago(l, now))).unwrap_or_default());
                if samples.is_empty() {
                    println!("No samples in the last {minutes} minutes. Run `sift sampler` (or keep the app open) to record history.");
                    return;
                }
                let avg = |f: &dyn Fn(&db::Sample) -> f64| samples.iter().map(f).sum::<f64>() / samples.len() as f64;
                println!("Last {minutes} min: CPU avg {:.0}%, memory avg {:.0}%", avg(&|s| s.cpu), avg(&|s| s.mem_pct));
                println!("\nBusiest processes (peak CPU):");
                for p in &top {
                    println!("  {:<28} peak {:>6.1}%  avg {:>5.1}%  mem {}", p.name, p.peak_cpu, p.avg_cpu, bytes(p.peak_mem as u64));
                }
            });
        }
        Cmd::Activity { out, limit } => {
            let db = match db::Db::open(&ctx.paths.db_file()) {
                Ok(d) => d,
                Err(e) => return fail(out.json, "activity", &e),
            };
            let ops = db.operations(limit).unwrap_or_default();
            emit(out.json, "activity", &ops, || {
                if ops.is_empty() {
                    println!("Nothing yet.");
                }
                let now = sift_core::now_unix();
                for o in &ops {
                    println!("{:>14}  {:<11} {:<9} {}", format::ago(o.started, now), o.kind, o.status, o.summary);
                }
            });
        }
        Cmd::Clean { out, act, system, empty_trash } => {
            let items: Vec<clean::CleanItem> = if let Some(p) = &act.plan {
                let text = match read_plan(p) {
                    Ok(t) => t,
                    Err(e) => return fail(out.json, "clean", &e),
                };
                let (items, refused) = clean::items_from_plan(&ctx, &text);
                for (path, why) in refused {
                    eprintln!("  refused  {path}: {why}");
                }
                items
            } else {
                let scan = clean::scan(&ctx, &clean::ScanOptions { include_system: system }, &cancel, &mut |_| {});
                if !act.apply {
                    emit(out.json, "clean", &scan, || print_clean_scan(&scan, &ctx));
                    return ExitCode::SUCCESS;
                }
                scan.items.into_iter().filter(|i| (i.selected || act.all) && i.blocked.is_none()).collect()
            };
            let opts = clean::ApplyOptions {
                dry_run: !act.apply,
                permanent: act.permanent,
                allow_empty_trash: empty_trash,
                allow_root: system && act.apply,
            };
            let items: Vec<clean::CleanItem> =
                items.into_iter().filter(|i| i.kind != clean::ItemKind::EmptyTrash || empty_trash).collect();
            let mut sink = stream_or_print(act.stream);
            let s = clean::apply(&ctx, &items, &opts, &cancel, &mut sink);
            if !act.stream {
                emit(out.json, "clean", &s, || print_summary(&s));
            }
        }
        Cmd::Purge { out, act, roots } => {
            let roots = if roots.is_empty() { ctx.settings.purge_root_paths(&ctx.paths) } else { roots };
            let paths: Vec<PathBuf> = if let Some(p) = &act.plan {
                match read_plan(p) {
                    Ok(t) => plan_paths(&t),
                    Err(e) => return fail(out.json, "purge", &e),
                }
            } else {
                let scan = purge::scan(&ctx, &roots, &cancel, &mut |_| {});
                if !act.apply {
                    emit(out.json, "purge", &scan, || {
                        for a in &scan.items {
                            let mark = if a.selected { "x" } else { " " };
                            println!("[{mark}] {:>10}  {:<22} {}", bytes(a.bytes), a.kind, tilde(&a.path, &ctx.paths.home));
                        }
                        println!("\n{} found, {} pre-selected ([x]); recent projects are left unticked.", bytes(scan.total_bytes), bytes(scan.selected_bytes));
                        println!("Dry run. Add --apply (or --apply --all) to move them to the Trash.");
                    });
                    return ExitCode::SUCCESS;
                }
                scan.items.into_iter().filter(|a| a.selected || act.all).map(|a| a.path).collect()
            };
            let mut sink = stream_or_print(act.stream);
            let s = purge::apply(&ctx, &roots, &paths, !act.apply, act.permanent, &cancel, &mut sink);
            if !act.stream {
                emit(out.json, "purge", &s, || print_summary(&s));
            }
        }
        Cmd::Installer { out, act } => {
            let paths: Vec<PathBuf> = if let Some(p) = &act.plan {
                match read_plan(p) {
                    Ok(t) => plan_paths(&t),
                    Err(e) => return fail(out.json, "installer", &e),
                }
            } else {
                let scan = installer::scan(&ctx, &cancel, &mut |_| {});
                if !act.apply {
                    emit(out.json, "installer", &scan, || {
                        for f in &scan.items {
                            let mark = if f.selected { "x" } else { " " };
                            let note = f.note.as_deref().map(|n| format!("  ({n})")).unwrap_or_default();
                            println!("[{mark}] {:>10}  {:>4}d  {}{note}", bytes(f.bytes), f.age_days, tilde(&f.path, &ctx.paths.home));
                        }
                        println!("\n{} found, {} pre-selected. Dry run; add --apply to move them to the Trash.", bytes(scan.total_bytes), bytes(scan.selected_bytes));
                    });
                    return ExitCode::SUCCESS;
                }
                scan.items.into_iter().filter(|f| f.selected || act.all).map(|f| f.path).collect()
            };
            let mut sink = stream_or_print(act.stream);
            let s = installer::apply(&ctx, &paths, !act.apply, act.permanent, &cancel, &mut sink);
            if !act.stream {
                emit(out.json, "installer", &s, || print_summary(&s));
            }
        }
        Cmd::Optimize { out, apply, tasks } => {
            let plan = optimize::plan(&ctx);
            let ids: Vec<String> = if tasks.is_empty() { plan.iter().filter(|t| t.default_on).map(|t| t.id.clone()).collect() } else { tasks };
            if !apply {
                emit(out.json, "optimize", &plan, || {
                    for t in &plan {
                        let mark = if ids.contains(&t.id) { "x" } else { " " };
                        let admin = if t.needs_root { " [admin]" } else { "" };
                        println!("[{mark}] {:<18} {}{admin}\n      {}", t.id, t.title, t.commands.join(" ; "));
                        for p in t.preview.iter().take(8) {
                            println!("        · {p}");
                        }
                    }
                    println!("\nDry run. `sift optimize --apply` runs the [x] tasks; pick others with --task ID.");
                });
                return ExitCode::SUCCESS;
            }
            let allow_root = plan.iter().any(|t| t.needs_root && ids.contains(&t.id));
            let results = optimize::run(&ctx, &ids, false, allow_root, &mut |e, r| {
                if let (Some(r), false) = (r, out.json) {
                    let mark = if r.ok { "ok " } else { "ERR" };
                    let _ = e;
                    println!("[{mark}] {:<18} {}", r.id, r.output.lines().last().unwrap_or(""));
                }
            });
            if out.json {
                println!("{}", envelope::ok("optimize", &results));
            }
        }
        Cmd::Apps { out } => {
            let list = apps::inventory(&ctx);
            emit(out.json, "apps", &list, || {
                for a in &list {
                    let lock = if a.removable { "" } else { "  (protected)" };
                    println!("{:>10}  {:<9} {:<34} {}{lock}", if a.size > 0 { bytes(a.size) } else { "—".into() }, a.source.label(), a.name, a.id);
                }
                println!("\n{} apps", list.len());
            });
        }
        Cmd::Uninstall { out, app_id, apply, leftovers } => {
            let all = apps::inventory(&ctx);
            let Some(app) = all.iter().find(|a| a.id == app_id || (a.package == app_id && a.source != apps::Source::Local)) else {
                return fail(out.json, "uninstall", &EngineError::new("not_found", format!("no installed app with id {app_id} (see `sift apps`)")));
            };
            let plan = apps::plan_uninstall(&ctx, app, &all);
            if !apply {
                emit(out.json, "uninstall", &plan, || {
                    println!("{} ({})", plan.app.name, plan.app.source.label());
                    if let Some(b) = &plan.blocked {
                        println!("  Blocked: {b}");
                    }
                    for c in &plan.commands {
                        println!("  run: {c}{}", if plan.needs_root { "   [admin]" } else { "" });
                    }
                    if !plan.also_removes.is_empty() {
                        println!("  also removes: {}", plan.also_removes.join(", "));
                    }
                    for l in &plan.leftovers {
                        println!("  leftover [{}] {:>10}  {}  — {}", if l.selected { "x" } else { " " }, bytes(l.bytes), tilde(&l.path, &ctx.paths.home), l.reason);
                    }
                    println!("Dry run. Add --apply (and --leftovers to trash the [x] leftovers).");
                });
                return ExitCode::SUCCESS;
            }
            let chosen: Vec<PathBuf> = if leftovers { plan.leftovers.iter().filter(|l| l.selected).map(|l| l.path.clone()).collect() } else { vec![] };
            let report = apps::uninstall(&ctx, &plan, &chosen, true, &mut stream_or_print(false));
            emit(out.json, "uninstall", &report, || println!("{}", report.message));
            if !report.ok {
                return ExitCode::FAILURE;
            }
        }
        Cmd::Analyze { out, path, depth, limit } => {
            let root = path.unwrap_or_else(|| ctx.paths.home.clone());
            let root = std::fs::canonicalize(&root).unwrap_or(root);
            let progress = analyze::Progress::default();
            let tree = match analyze::scan(&root, &progress, &cancel) {
                Ok(t) => t,
                Err(e) => return fail(out.json, "analyze", &e),
            };
            let entries = analyze::entries(&tree, tree.root(), depth.max(1), limit);
            #[derive(Serialize)]
            struct A<'a> {
                path: &'a PathBuf,
                bytes: u64,
                files: u64,
                errors: u64,
                entries: &'a [analyze::Entry],
            }
            let a = A { path: &tree.root_path, bytes: tree.nodes[0].bytes, files: tree.nodes[0].files, errors: tree.errors, entries: &entries };
            emit(out.json, "analyze", &a, || {
                println!("{}  {} in {} files", tree.root_path.display(), bytes(a.bytes), a.files);
                fn show(es: &[analyze::Entry], indent: usize, total: u64) {
                    for e in es {
                        let pct = if total > 0 { e.bytes as f64 / total as f64 * 100.0 } else { 0.0 };
                        println!("{:indent$}{:>10} {:>5.1}%  {}{}", "", bytes(e.bytes), pct, e.name, if e.is_dir { "/" } else { "" }, indent = indent);
                        show(&e.children, indent + 4, total);
                    }
                }
                show(&entries, 2, a.bytes);
            });
        }
        Cmd::Dupes { out, paths, min_size, hidden } => {
            let roots: Vec<PathBuf> = paths.iter().map(|p| std::fs::canonicalize(p).unwrap_or(p.clone())).collect();
            let opts = dupes::Options { min_size: min_size.unwrap_or(ctx.settings.dupes_min_size), include_hidden: hidden };
            let r = dupes::find(&ctx, &roots, &opts, &dupes::Progress::default(), &cancel);
            emit(out.json, "dupes", &r, || {
                for g in r.groups.iter().take(50) {
                    println!("{} × {} ({} reclaimable)", g.files.len(), bytes(g.size), bytes(g.wasted));
                    for f in &g.files {
                        println!("    {}", tilde(&f.path, &ctx.paths.home));
                    }
                }
                println!("\n{} groups, {} reclaimable, {} files scanned. Read-only — review and remove them in the app.", r.groups.len(), bytes(r.wasted), r.scanned_files);
            });
        }
        Cmd::Ports { out } => {
            let list = ports::list();
            emit(out.json, "ports", &list, || {
                for p in &list {
                    println!(
                        "{:<4} {:>5}  {:<22} {:<8} {:<8} {}",
                        p.proto,
                        p.port,
                        p.address,
                        p.exposure,
                        p.pid.map(|x| x.to_string()).unwrap_or_else(|| "-".into()),
                        p.process.clone().unwrap_or_else(|| format!("({})", p.user))
                    );
                }
            });
        }
        Cmd::Doctor { out } => {
            let snap = status::Collector::new().collect();
            let checks = doctor::run(&ctx, Some(&snap));
            emit(out.json, "doctor", &checks, || {
                for c in &checks {
                    let tag = match c.level {
                        doctor::Level::Fail => "FAIL",
                        doctor::Level::Warn => "WARN",
                        doctor::Level::Info => "info",
                        doctor::Level::Ok => " ok ",
                    };
                    println!("[{tag}] {:<22} {}", c.title, c.summary);
                    if let (Some(h), true) = (&c.hint, c.level != doctor::Level::Ok) {
                        println!("       → {h}");
                    }
                }
            });
        }
    }
    ExitCode::SUCCESS
}

fn print_status(s: &status::Snapshot, ctx: &Ctx) {
    let _ = ctx;
    println!("{} · {} · {} · up {}", s.host, s.os, s.model, format::duration(s.uptime_secs));
    println!("Health {} {} — {}", s.health.score, s.health.label, s.health.message);
    println!(
        "CPU     {:>5.1}%  load {:.2} {:.2} {:.2}  {} cores{}",
        s.cpu.usage,
        s.cpu.load1,
        s.cpu.load5,
        s.cpu.load15,
        s.cpu.logical,
        s.cpu.temp_c.map(|t| format!("  {t:.0}°C")).unwrap_or_default()
    );
    println!(
        "Memory  {:>5.1}%  {} / {}  swap {}  pressure {}",
        s.memory.used_pct,
        bytes(s.memory.used),
        bytes(s.memory.total),
        bytes(s.memory.swap_used),
        s.memory.pressure
    );
    for g in &s.gpus {
        println!(
            "GPU     {}  {}{}",
            g.usage.map(|u| format!("{u:>5.1}%")).unwrap_or_else(|| format!("{:>6}", g.state)),
            g.name,
            g.temp_c.map(|t| format!("  {t:.0}°C")).unwrap_or_default()
        );
    }
    for d in &s.disks {
        println!("Disk    {:<16} {} free of {} ({:.0}% used)", d.mount, bytes(d.free), bytes(d.total), d.used_pct);
    }
    println!("I/O     read {}  write {}", format::rate(s.disk_io.read_bps), format::rate(s.disk_io.write_bps));
    println!("Net     ↓ {}  ↑ {}", format::rate(s.net_rx_bps), format::rate(s.net_tx_bps));
    for b in &s.batteries {
        println!(
            "Battery {:.0}% {}{}{}",
            b.percent,
            b.status,
            b.health_pct.map(|h| format!("  health {h:.0}%")).unwrap_or_default(),
            b.time_left_secs.map(|t| format!("  {} left", format::duration(t))).unwrap_or_default()
        );
    }
    if !s.thermal.fans.is_empty() {
        let fans: Vec<String> = s.thermal.fans.iter().map(|f| format!("{} {} rpm", f.label, f.rpm)).collect();
        println!("Fans    {}", fans.join(", "));
    }
    println!("\nTop processes:");
    for p in s.top_processes.iter().take(8) {
        println!("  {:>7} {:<24} {:>6.1}%  {}", p.pid, p.name, p.cpu, bytes(p.mem_bytes));
    }
}

fn print_clean_scan(scan: &clean::CleanScan, ctx: &Ctx) {
    let mut group: Option<clean::Group> = None;
    for i in &scan.items {
        if group != Some(i.group) {
            group = Some(i.group);
            println!("\n{}", i.group.title());
        }
        let mark = if i.selected { "x" } else { " " };
        let size = if i.approx { format!("≤{}", bytes(i.bytes)) } else { bytes(i.bytes) };
        let place = i.path.as_ref().map(|p| tilde(p, &ctx.paths.home)).or(i.command.clone()).unwrap_or_default();
        let extra = i.blocked.as_deref().map(|b| format!("  — skipped: {b}")).unwrap_or_default();
        println!("  [{mark}] {size:>10}  {:<34} {place}{extra}", i.label);
    }
    println!(
        "\n{} reclaimable, {} pre-selected ([x]). Dry run — add --apply to move the [x] items to the Trash{}.",
        bytes(scan.total_bytes),
        bytes(scan.selected_bytes),
        if scan.items.iter().any(|i| i.group == clean::Group::System) { "; admin items need --system --apply --all or a reviewed plan" } else { "" }
    );
}
