//! Phase 0 benchmark for the code index. See "Code index" in WARPIUM.md for how to fetch the
//! corpus and run it (`script/warpium/code_index_bench`).
//!
//! ```text
//! cargo run --release -p code_index --example bench -- [--threads N] [--name LABEL] ROOT...
//! ```
//!
//! Each root is measured in a fresh child process, so peak RSS is per root:
//! cold full index, snapshot save, warm start (snapshot load, and load + rescan), query latency
//! (p50/p95/max over a fixed query list plus names sampled from the index) and incremental update
//! after an edit. `--name` labels the output instead of the root path (use it for private
//! projects so their paths stay out of logs).

use std::path::{Path, PathBuf};
use std::time::Duration;

use instant::Instant;

use code_index::{CodeIndex, SearchScope, snapshot_path_in};

/// Typical palette queries: abbreviations, prefixes, common words, members and extension
/// receivers. Names sampled from each corpus are added at run time.
const QUERIES: &[&str] = &[
    "a",
    "vm",
    "UVM",
    "MVM",
    "Repo",
    "ViewModel",
    "Activity",
    "Fragment",
    "MainActivity",
    "onCreate",
    "invoke",
    "toString",
    "Dao",
    "Impl",
    "Test",
    "Settings",
    "Account",
    "Message",
    "Util",
    "Helper",
    "Manager",
    "Service",
    "Provider",
    "Module",
    "Factory",
    "Adapter",
    "Screen",
    "State",
    "Event",
    "Builder",
    "Config",
    "Json",
    "Http",
    "get",
    "create",
    "load",
    "Mapper",
    "UseCase",
    "Navigation",
    "Theme",
    "String.",
    "Preview",
    "Exception",
    "Listener",
    "RepoImpl",
    "AcRe",
    "getUN",
    "data.Repo",
];

const SAMPLED_NAMES: usize = 30;
const QUERY_REPEATS: usize = 5;
const EDITS: usize = 20;
const RESULT_LIMIT: usize = 100;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut threads = code_index::default_threads();
    let mut name = None;
    let mut child = false;
    let mut warm_child = false;
    let mut cache = None;
    let mut roots = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--threads" => threads = args.next().and_then(|n| n.parse().ok()).unwrap_or(threads),
            "--name" => name = args.next(),
            "--child" => child = true,
            "--warm-child" => warm_child = true,
            "--cache" => cache = args.next().map(PathBuf::from),
            _ => roots.push(PathBuf::from(arg)),
        }
    }
    if roots.is_empty() {
        eprintln!("usage: bench [--threads N] [--name LABEL] ROOT...");
        std::process::exit(2);
    }
    if child || warm_child {
        let root = std::fs::canonicalize(&roots[0]).expect("root exists");
        let cache = cache.expect("--cache is passed to children");
        if warm_child {
            run_warm(&root, &cache, threads);
        } else {
            run(
                &root,
                &cache,
                name.as_deref().unwrap_or(&root.to_string_lossy()),
                threads,
            );
        }
        return;
    }
    let exe = std::env::current_exe().expect("current exe");
    for root in roots {
        let cache = std::env::temp_dir().join(format!("code-index-bench-{}", std::process::id()));
        for mode in ["--child", "--warm-child"] {
            // A developer tool that respawns itself; the lint is about console windows flashing
            // in the app on Windows.
            #[allow(clippy::disallowed_types)]
            let mut command = std::process::Command::new(&exe);
            command
                .arg(mode)
                .arg("--threads")
                .arg(threads.to_string())
                .arg("--cache")
                .arg(&cache);
            if let Some(name) = &name {
                command.arg("--name").arg(name);
            }
            let status = command.arg(&root).status().expect("spawn child");
            if !status.success() {
                eprintln!("benchmark failed for a root ({status})");
                break;
            }
        }
        let _ = std::fs::remove_dir_all(cache);
    }
}

/// A fresh process that only loads the snapshot, rescans and queries, as the app does on a later
/// launch, so its peak RSS is what the index keeps resident.
fn run_warm(root: &Path, cache: &Path, threads: usize) {
    let rss_before = rss_now();
    let snapshot = snapshot_path_in(cache, root);
    let started = Instant::now();
    let (index, _) = code_index::open(root, Some(&snapshot), threads);
    let warm = started.elapsed();
    for query in queries(&index) {
        std::hint::black_box(index.search(&query, SearchScope::All, RESULT_LIMIT));
    }
    println!(
        "warm process: open {}  RSS before {:.0} MB  peak {:.0} MB  (index ≈ {:.0} MB)",
        ms(warm),
        mb(rss_before),
        mb(peak_rss()),
        mb(peak_rss().saturating_sub(rss_before))
    );
}

fn run(root: &Path, cache: &Path, label: &str, threads: usize) {
    println!("== {label} ({threads} threads)");
    let rss_before = rss_now();

    let started = Instant::now();
    let index = CodeIndex::build(root, threads);
    let cold = started.elapsed();
    let stats = index.stats();
    let peak_after_cold = peak_rss();
    println!(
        "files {}  symbols {}  strings {}  heap≈{:.1} MB",
        stats.files,
        stats.symbols,
        stats.strings,
        mb(stats.heap_bytes as u64)
    );
    println!("cold full index      {}", ms(cold));

    let snapshot = snapshot_path_in(cache, root);
    let started = Instant::now();
    index.save(&snapshot).expect("save snapshot");
    let save = started.elapsed();
    let snapshot_bytes = std::fs::metadata(&snapshot).map(|m| m.len()).unwrap_or(0);
    println!(
        "snapshot save        {}  ({:.1} MB)",
        ms(save),
        mb(snapshot_bytes)
    );
    let rss_after_cold = rss_now();
    drop(index);

    let started = Instant::now();
    let loaded = CodeIndex::load(&snapshot, root).expect("load snapshot");
    let load = started.elapsed();
    drop(loaded);
    let started = Instant::now();
    let (mut index, report) = code_index::open(root, Some(&snapshot), threads);
    let warm = started.elapsed();
    println!("warm start (load)    {}", ms(load));
    println!(
        "warm start + rescan  {}  (reparsed {} files)",
        ms(warm),
        report.update.files_parsed
    );

    let queries = queries(&index);
    for (scope, label) in [(SearchScope::Types, "class"), (SearchScope::All, "symbol")] {
        let mut latencies = Vec::new();
        for _ in 0..QUERY_REPEATS {
            for query in &queries {
                let started = Instant::now();
                let results = index.search(query, scope, RESULT_LIMIT);
                latencies.push(started.elapsed());
                std::hint::black_box(results);
            }
        }
        println!(
            "query {label:<7}        {}  ({} queries)",
            percentiles(&mut latencies),
            queries.len()
        );
    }

    let mut updates = Vec::new();
    for path in edit_targets(&index) {
        let original = std::fs::read(&path).expect("read file");
        let mut edited = original.clone();
        edited.extend_from_slice(b"\nclass CodeIndexBenchEdit\n");
        std::fs::write(&path, &edited).expect("write edit");
        let started = Instant::now();
        let stats = index.apply_changes(std::slice::from_ref(&path), threads);
        updates.push(started.elapsed());
        std::fs::write(&path, &original).expect("restore file");
        index.apply_changes(std::slice::from_ref(&path), threads);
        assert_eq!(stats.files_parsed, 1, "edited file was reparsed");
    }
    println!("update after edit    {}", percentiles(&mut updates));

    let peak = peak_rss();
    println!(
        "RSS  before {:.0} MB  after cold build {:.0} MB  peak {:.0} MB (cold-build peak {:.0} MB)",
        mb(rss_before),
        mb(rss_after_cold),
        mb(peak),
        mb(peak_after_cold)
    );
}

/// The fixed list plus names sampled evenly from the index, and their camel-hump initials.
fn queries(index: &CodeIndex) -> Vec<String> {
    let mut queries: Vec<String> = QUERIES.iter().map(|q| (*q).to_owned()).collect();
    let classes = index.search("e", SearchScope::Types, 5000);
    let step = (classes.len() / SAMPLED_NAMES).max(1);
    for result in classes.iter().step_by(step).take(SAMPLED_NAMES) {
        let name = &result.symbol.name;
        queries.push(name.clone());
        let initials: String = name.chars().filter(char::is_ascii_uppercase).collect();
        if initials.len() >= 2 {
            queries.push(initials);
        }
    }
    queries
}

fn edit_targets(index: &CodeIndex) -> Vec<PathBuf> {
    let mut paths: Vec<String> = index
        .stamps()
        .into_keys()
        .filter(|path| path.ends_with(".kt") && !path.contains("/build/"))
        .collect();
    paths.sort();
    let step = (paths.len() / EDITS).max(1);
    paths
        .iter()
        .step_by(step)
        .take(EDITS)
        .map(|path| index.abs_path(path))
        .collect()
}

fn percentiles(latencies: &mut [Duration]) -> String {
    if latencies.is_empty() {
        return "n/a".to_owned();
    }
    latencies.sort();
    let at = |p: f64| latencies[((latencies.len() - 1) as f64 * p).round() as usize];
    format!(
        "p50 {}  p95 {}  max {}",
        ms(at(0.5)),
        ms(at(0.95)),
        ms(latencies[latencies.len() - 1])
    )
}

fn ms(duration: Duration) -> String {
    format!("{:.2} ms", duration.as_secs_f64() * 1000.0)
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Peak resident set size of this process in bytes.
fn peak_rss() -> u64 {
    #[cfg(target_os = "linux")]
    {
        proc_status_kb("VmHWM:") * 1024
    }
    #[cfg(not(target_os = "linux"))]
    {
        // SAFETY: getrusage only writes into the zeroed struct we pass.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        // macOS reports bytes.
        usage.ru_maxrss as u64
    }
}

/// Current resident set size in bytes (peak on platforms without a cheap current reading).
fn rss_now() -> u64 {
    #[cfg(target_os = "linux")]
    {
        proc_status_kb("VmRSS:") * 1024
    }
    #[cfg(not(target_os = "linux"))]
    {
        peak_rss()
    }
}

#[cfg(target_os = "linux")]
fn proc_status_kb(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with(field))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kb| kb.parse().ok())
        })
        .unwrap_or(0)
}
