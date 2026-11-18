//! Phase 0 benchmark for the code index. See "Code index" in WARPIUM.md for how to fetch the
//! corpus and run it (`script/warpium/code_index_bench`).
//!
//! ```text
//! cargo run --release -p code_index --example bench -- [--threads N] [--name LABEL] ROOT...
//! ```
//!
//! Each root is measured in a fresh child process, so peak RSS is per root:
//! cold full index, snapshot save, warm start (snapshot load, and load + rescan), query latency
//! (p50/p95/max over a fixed query list plus names sampled from the index), Go to Definition on
//! references sampled from the corpus, Find Usages of sampled declarations, and incremental update
//! after an edit, and Go to Implementation on the same declarations. `--name` labels the output instead of the root path (use it for private
//! projects so their paths stay out of logs).

use std::path::{Path, PathBuf};
use std::time::Duration;

use code_index::{CodeIndex, Language, Position, SearchScope, SourceFile, snapshot_path_in};
use instant::Instant;

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
/// Files references are sampled from, and references taken per file.
const REFERENCE_FILES: usize = 60;
const REFERENCES_PER_FILE: usize = 10;
/// Declarations whose usages are timed, per kind (types, functions).
const USAGE_TARGETS: usize = 25;
const USAGES_LIMIT: usize = 1000;

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
        "files {}  lines {}  symbols {}  strings {}  heap≈{:.1} MB",
        stats.files,
        source_lines(&index),
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

    let references = reference_samples(&index);
    let mut latencies = Vec::new();
    let (mut resolved, mut exact) = (0, 0);
    for (rel_path, text, language, position) in &references {
        let file = SourceFile {
            rel_path,
            text,
            language: *language,
        };
        let started = Instant::now();
        let result = index.find_definition(&file, *position);
        latencies.push(started.elapsed());
        if let Some(result) = result
            && let Some(first) = result.definitions.first()
        {
            resolved += 1;
            exact += usize::from(first.resolution.is_exact());
        }
    }
    println!(
        "definition           {}  ({} refs, {:.0}% resolved, {:.0}% exact)",
        percentiles(&mut latencies),
        references.len(),
        percent(resolved, references.len()),
        percent(exact, references.len())
    );

    let mut latencies = Vec::new();
    let mut usage_counts = Vec::new();
    let mut via_counts = Vec::new();
    let mut implementation_latencies = Vec::new();
    let mut implementation_counts = Vec::new();
    for target in usage_targets(&index) {
        let Ok(text) = std::fs::read_to_string(index.abs_path(&target.rel_path)) else {
            continue;
        };
        let file = SourceFile {
            rel_path: &target.rel_path,
            text: &text,
            language: target.language,
        };
        let started = Instant::now();
        let result = index.find_usages(&file, target.name_pos, USAGES_LIMIT, threads);
        latencies.push(started.elapsed());
        usage_counts.push(result.as_ref().map_or(0, |result| result.usages.len()));
        via_counts.push(result.map_or(0, |result| {
            result.usages.iter().filter(|usage| usage.via).count()
        }));

        let started = Instant::now();
        let result = index.find_implementations(&file, target.name_pos);
        implementation_latencies.push(started.elapsed());
        implementation_counts.push(result.map_or(0, |result| result.implementations.len()));
    }
    usage_counts.sort_unstable();
    via_counts.sort_unstable();
    implementation_counts.sort_unstable();
    println!(
        "usages               {}  ({} declarations, median {} usages, max {})",
        percentiles(&mut latencies),
        usage_counts.len(),
        usage_counts
            .get(usage_counts.len() / 2)
            .copied()
            .unwrap_or(0),
        usage_counts.last().copied().unwrap_or(0)
    );
    println!(
        "  of which via overrides: median {}, max {}",
        median(&via_counts),
        via_counts.last().copied().unwrap_or(0)
    );
    println!(
        "implementations      {}  (median {}, max {})",
        percentiles(&mut implementation_latencies),
        median(&implementation_counts),
        implementation_counts.last().copied().unwrap_or(0)
    );

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

/// Identifier positions from files sampled evenly: names after a dot and capitalized names, the
/// references people jump from most.
fn reference_samples(index: &CodeIndex) -> Vec<(String, String, Language, Position)> {
    let mut paths: Vec<String> = index.stamps().into_keys().collect();
    paths.sort();
    let step = (paths.len() / REFERENCE_FILES).max(1);
    let mut samples = Vec::new();
    for rel_path in paths.iter().step_by(step).take(REFERENCE_FILES) {
        let Some(language) = Language::from_path(Path::new(rel_path)) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(index.abs_path(rel_path)) else {
            continue;
        };
        let mut positions = Vec::new();
        for (line_number, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("import ")
                || trimmed.starts_with("package ")
                || trimmed.starts_with("//")
                || trimmed.starts_with('*')
            {
                continue;
            }
            let chars: Vec<char> = line.chars().collect();
            for (column, c) in chars.iter().enumerate() {
                let starts_word = column == 0
                    || !(chars[column - 1].is_alphanumeric() || chars[column - 1] == '_');
                let after_dot = column > 0 && chars[column - 1] == '.';
                if starts_word && c.is_alphabetic() && (after_dot || c.is_uppercase()) {
                    positions.push(Position {
                        line: line_number as u32,
                        column: column as u32,
                    });
                }
            }
        }
        let step = (positions.len() / REFERENCES_PER_FILE).max(1);
        for position in positions
            .into_iter()
            .step_by(step)
            .take(REFERENCES_PER_FILE)
        {
            samples.push((rel_path.clone(), text.clone(), language, position));
        }
    }
    samples
}

/// Classes and functions sampled evenly from the index, project code only.
fn usage_targets(index: &CodeIndex) -> Vec<code_index::SymbolInfo> {
    let mut targets = Vec::new();
    for (query, scope) in [("e", SearchScope::Types), ("get", SearchScope::All)] {
        let found: Vec<_> = index
            .search(query, scope, 5000)
            .into_iter()
            .map(|result| result.symbol)
            .filter(|symbol| {
                symbol.origin == code_index::Origin::Main
                    && (scope == SearchScope::Types
                        || symbol.kind == code_index::SymbolKind::Function)
            })
            .collect();
        let step = (found.len() / USAGE_TARGETS).max(1);
        targets.extend(found.into_iter().step_by(step).take(USAGE_TARGETS));
    }
    targets
}

fn median(sorted: &[usize]) -> usize {
    sorted.get(sorted.len() / 2).copied().unwrap_or(0)
}

/// Lines of indexed source, read outside any timing.
fn source_lines(index: &CodeIndex) -> usize {
    index
        .stamps()
        .into_keys()
        .filter_map(|rel_path| std::fs::read(index.abs_path(&rel_path)).ok())
        .map(|bytes| bytes.iter().filter(|byte| **byte == b'\n').count())
        .sum()
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
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
