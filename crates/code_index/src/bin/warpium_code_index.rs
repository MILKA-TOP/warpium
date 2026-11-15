//! Command-line access to the Warpium code index.
//!
//! ```text
//! warpium-code-index mcp    [--root DIR]                  # MCP server on stdio
//! warpium-code-index index  [--root DIR]                  # build or refresh, save, print stats
//! warpium-code-index search [--root DIR] [--types] QUERY  # print the best matches
//! ```
//!
//! The root defaults to the current directory. Snapshots are shared with the app (same cache
//! path per root).

use instant::Instant;
use std::path::PathBuf;
use std::process::ExitCode;

use code_index::{SearchScope, default_snapshot_path, default_threads, mcp};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return usage();
    };
    let mut root = None;
    let mut types_only = false;
    let mut limit = 20;
    let mut rest = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--types" => types_only = true,
            "--limit" => limit = args.next().and_then(|n| n.parse().ok()).unwrap_or(limit),
            _ => rest.push(arg),
        }
    }
    let root = match root.map_or_else(std::env::current_dir, Ok) {
        Ok(root) => canonicalize_or_keep(root),
        Err(err) => {
            eprintln!("Can't read the current directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let snapshot = default_snapshot_path(&root);

    match command.as_str() {
        "mcp" => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            match mcp::Server::new(root, snapshot).serve(stdin.lock(), stdout.lock()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("MCP transport error: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        "index" => {
            let started = Instant::now();
            let (index, report) = code_index::open(&root, snapshot.as_deref(), default_threads());
            let stats = index.stats();
            let snapshot_load = report.loaded_in.map_or_else(
                || "no snapshot".to_owned(),
                |loaded| format!("snapshot loaded in {} ms", loaded.as_millis()),
            );
            println!(
                "{} files, {} symbols in {:.0?} ({snapshot_load}, parsed {}, removed {})",
                stats.files,
                stats.symbols,
                started.elapsed(),
                report.update.files_parsed,
                report.update.files_removed
            );
            if let Some(snapshot) = snapshot
                && let Err(err) = index.save(&snapshot)
            {
                eprintln!("Couldn't save {}: {err}", snapshot.display());
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        "search" if !rest.is_empty() => {
            let (index, _) = code_index::open(&root, snapshot.as_deref(), default_threads());
            let scope = if types_only {
                SearchScope::Types
            } else {
                SearchScope::All
            };
            for result in index.search(&rest.join(" "), scope, limit) {
                let symbol = result.symbol;
                println!(
                    "{:?}\t{} {}\t{}\t{}:{}",
                    result.tier,
                    symbol.kind.label(),
                    symbol.name,
                    symbol.location(),
                    symbol.rel_path,
                    symbol.name_pos.line + 1
                );
            }
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}

fn canonicalize_or_keep(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: warpium-code-index <mcp | index | search QUERY> [--root DIR] [--types] [--limit N]"
    );
    ExitCode::FAILURE
}
