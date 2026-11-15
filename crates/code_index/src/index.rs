use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rayon::prelude::*;

use crate::extract::{RawFile, extract};
use crate::fuzzy::{MatchTier, Pattern};
use crate::interner::{Interner, Sym};
use crate::layout::{is_walkable_dir, place};
use crate::model::{FileData, Import, Language, Origin, Position, Symbol, SymbolKind, Visibility};
use crate::scan::{FoundFile, abs_path, rel_path, scan, stat_file};

/// Files larger than this are recorded without symbols: they are almost always generated and
/// would dominate parse time and memory.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Most worker threads a build may use, so indexing never takes over the machine.
const MAX_THREADS: usize = 8;

/// The number of worker threads to index with by default: half the cores, at most 8.
pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|cores| cores.get() / 2)
        .unwrap_or(2)
        .clamp(1, MAX_THREADS)
}

/// An in-memory index of the Kotlin and Java declarations under one root directory.
#[derive(Debug)]
pub struct CodeIndex {
    pub(crate) root: PathBuf,
    pub(crate) interner: Interner,
    /// Slots indexed by file id; `None` slots are free.
    pub(crate) files: Vec<Option<FileData>>,
    pub(crate) free_slots: Vec<u32>,
    pub(crate) by_path: HashMap<String, u32>,
    pub(crate) symbol_count: usize,
}

/// Which declarations a search covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchScope {
    /// Classes, interfaces, objects, enums, annotations, records and type aliases.
    Types,
    /// Every declaration.
    All,
}

/// A declaration with its strings resolved, detached from the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    pub flags: u8,
    pub container: String,
    pub receiver: Option<String>,
    pub package: String,
    /// Path relative to the index root, with `/` separators.
    pub rel_path: String,
    pub language: Language,
    pub origin: Origin,
    pub module: String,
    pub source_set: String,
    pub name_pos: Position,
    pub start_line: u32,
    pub end_line: u32,
}

impl SymbolInfo {
    pub fn has_flag(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    /// The fully qualified name: package, enclosing declarations and name.
    pub fn qualified_name(&self) -> String {
        [
            self.package.as_str(),
            self.container.as_str(),
            self.name.as_str(),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(".")
    }

    /// The package and enclosing declarations, which is where the symbol lives.
    pub fn location(&self) -> String {
        [self.package.as_str(), self.container.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(".")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub symbol: SymbolInfo,
    pub tier: MatchTier,
    /// Byte offsets in [`SymbolInfo::name`] of the matched query characters.
    pub matched_indices: Vec<usize>,
}

/// Parsed changes, ready to be applied with [`CodeIndex::apply`]. Preparing does the slow work
/// (walking and parsing) without access to the index, so a shared index stays readable meanwhile.
#[derive(Debug, Default)]
pub struct PendingUpdate {
    parsed: Vec<(FoundFile, RawFile)>,
    /// Relative paths of removed files or directories.
    removed: Vec<String>,
    /// After a full rescan, every relative path found on disk; other indexed files are dropped.
    present: Option<HashSet<String>>,
}

impl PendingUpdate {
    pub fn is_empty(&self) -> bool {
        self.parsed.is_empty() && self.removed.is_empty() && self.present.is_none()
    }

    pub fn parsed_files(&self) -> usize {
        self.parsed.len()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdateStats {
    pub files_parsed: usize,
    pub files_removed: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexStats {
    pub files: usize,
    pub symbols: usize,
    pub strings: usize,
    /// Approximate heap bytes held by the index.
    pub heap_bytes: usize,
}

impl CodeIndex {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            interner: Interner::default(),
            files: Vec::new(),
            free_slots: Vec::new(),
            by_path: HashMap::new(),
            symbol_count: 0,
        }
    }

    /// Indexes every source file under `root`.
    pub fn build(root: impl Into<PathBuf>, threads: usize) -> Self {
        let mut index = Self::new(root);
        index.refresh(threads);
        index
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `(mtime, size)` of every indexed file, keyed by relative path, for [`refresh_in_batches`].
    pub fn stamps(&self) -> HashMap<String, (u64, u64)> {
        self.live_files()
            .map(|file| (file.rel_path.clone(), (file.mtime_ns, file.size)))
            .collect()
    }

    /// Rescans the root and reparses files whose size or modification time changed.
    pub fn refresh(&mut self, threads: usize) -> UpdateStats {
        let root = self.root.clone();
        let stamps = self.stamps();
        let mut stats = UpdateStats::default();
        refresh_in_batches(&root, &stamps, threads, |batch| {
            let batch_stats = self.apply(batch);
            stats.files_parsed += batch_stats.files_parsed;
            stats.files_removed += batch_stats.files_removed;
            true
        });
        stats
    }

    /// Reindexes the given changed, added or removed paths (files or directories).
    pub fn apply_changes(&mut self, paths: &[PathBuf], threads: usize) -> UpdateStats {
        let update = prepare_changes(&self.root, paths, threads);
        self.apply(update)
    }

    pub fn apply(&mut self, update: PendingUpdate) -> UpdateStats {
        let mut stats = UpdateStats::default();
        if let Some(present) = &update.present {
            let gone: Vec<String> = self
                .by_path
                .keys()
                .filter(|path| !present.contains(path.as_str()))
                .cloned()
                .collect();
            for path in gone {
                stats.files_removed += usize::from(self.remove_file(&path));
            }
        }
        stats.files_removed += self.remove_paths(&update.removed);
        for (found, raw) in update.parsed {
            self.insert_file(found, raw);
            stats.files_parsed += 1;
        }
        stats
    }

    pub fn stats(&self) -> IndexStats {
        let mut heap_bytes = self.interner.heap_bytes()
            + self.files.capacity() * std::mem::size_of::<Option<FileData>>()
            + self.by_path.capacity() * (std::mem::size_of::<(String, u32)>() + 1);
        for file in self.live_files() {
            heap_bytes += file.rel_path.capacity() * 2
                + file.symbols.capacity() * std::mem::size_of::<Symbol>()
                + file.imports.capacity() * std::mem::size_of::<Import>();
        }
        IndexStats {
            files: self.by_path.len(),
            symbols: self.symbol_count,
            strings: self.interner.len(),
            heap_bytes,
        }
    }

    /// The best matches for `query`, best first. Names are matched in tiers (see
    /// [`crate::fuzzy`]); within a tier, project sources rank above tests and generated code.
    ///
    /// A query with dots (`data.UserRepo`, `String.shout`) matches its last segment against
    /// names and the rest against the package, enclosing declarations or extension receiver.
    pub fn search(&self, query: &str, scope: SearchScope, limit: usize) -> Vec<SearchResult> {
        let query = query.trim();
        let (qualifier, name_query) = match query.rsplit_once('.') {
            Some((qualifier, name)) => (Some(qualifier), name),
            None => (None, query),
        };
        let qualifier = qualifier
            .map(|qualifier| qualifier.to_ascii_lowercase().into_bytes())
            .filter(|qualifier| !qualifier.is_empty());
        // `Outer.` or `String.` alone lists everything the qualifier matches.
        let pattern = Pattern::new(name_query);
        if pattern.is_none() && qualifier.is_none() {
            return Vec::new();
        }

        // Per-name match cache: 0 = not computed, 1 = no match, otherwise an encoded match.
        let mut memo = vec![0u32; self.interner.len()];
        let mut candidates: Vec<Candidate> = Vec::new();
        for (slot, file) in self.files.iter().enumerate() {
            let Some(file) = file else { continue };
            for (index, symbol) in file.symbols.iter().enumerate() {
                if scope == SearchScope::Types && !symbol.kind.is_type() {
                    continue;
                }
                let encoded = match memo[symbol.name.0 as usize] {
                    0 => {
                        let name = self.interner.resolve(symbol.name);
                        let encoded = match &pattern {
                            None => encode_match(MatchTier::Prefix, -(name.len() as i32)),
                            Some(pattern) if pattern.may_match(self.interner.mask(symbol.name)) => {
                                pattern
                                    .match_name(name)
                                    .map_or(1, |found| encode_match(found.tier, found.score))
                            }
                            Some(_) => 1,
                        };
                        memo[symbol.name.0 as usize] = encoded;
                        encoded
                    }
                    cached => cached,
                };
                if encoded == 1 {
                    continue;
                }
                if let Some(qualifier) = &qualifier {
                    let matches = if pattern.is_some() {
                        self.qualifier_matches(qualifier, file, symbol)
                    } else {
                        self.qualifier_is_scope(qualifier, file, symbol)
                    };
                    if !matches {
                        continue;
                    }
                }
                candidates.push(Candidate {
                    encoded,
                    origin: file.origin,
                    name_len: self.interner.resolve(symbol.name).len() as u32,
                    slot: slot as u32,
                    index: index as u32,
                });
            }
        }

        let order = |a: &Candidate, b: &Candidate| {
            b.tier()
                .cmp(&a.tier())
                .then(a.origin.cmp(&b.origin))
                .then(b.score().cmp(&a.score()))
                .then(a.name_len.cmp(&b.name_len))
                .then_with(|| self.rel_path_of(a.slot).cmp(self.rel_path_of(b.slot)))
                .then(a.index.cmp(&b.index))
        };
        if candidates.len() > limit && limit > 0 {
            candidates.select_nth_unstable_by(limit - 1, order);
            candidates.truncate(limit);
        }
        candidates.sort_unstable_by(order);
        candidates.truncate(limit);
        candidates
            .into_iter()
            .map(|candidate| {
                let file = self.files[candidate.slot as usize]
                    .as_ref()
                    .expect("candidate slots are live");
                let symbol = &file.symbols[candidate.index as usize];
                let info = self.symbol_info(file, symbol);
                SearchResult {
                    matched_indices: pattern
                        .as_ref()
                        .map(|pattern| pattern.matched_indices(&info.name))
                        .unwrap_or_default(),
                    symbol: info,
                    tier: candidate.tier(),
                }
            })
            .collect()
    }

    /// The `expect` declaration and `actual` implementations sharing `symbol`'s fully qualified
    /// name, in other source sets. Empty unless `symbol` is itself `expect` or `actual`.
    pub fn expect_actual_counterparts(&self, symbol: &SymbolInfo) -> Vec<SymbolInfo> {
        use crate::model::flags::{ACTUAL, EXPECT};
        if !symbol.has_flag(EXPECT | ACTUAL) {
            return Vec::new();
        }
        let (Some(name), Some(package), Some(container)) = (
            self.interner.get(&symbol.name),
            self.interner.get(&symbol.package),
            self.interner.get(&symbol.container),
        ) else {
            return Vec::new();
        };
        let mut counterparts = Vec::new();
        for file in self.live_files() {
            if file.package != package {
                continue;
            }
            for candidate in &file.symbols {
                let is_self =
                    file.rel_path == symbol.rel_path && candidate.name_pos == symbol.name_pos;
                if candidate.name == name
                    && candidate.container == container
                    && candidate.has_flag(EXPECT | ACTUAL)
                    && !is_self
                {
                    counterparts.push(self.symbol_info(file, candidate));
                }
            }
        }
        counterparts.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        counterparts
    }

    /// Every declaration in the file at `rel_path`, in source order.
    pub fn file_symbols(&self, rel_path: &str) -> Vec<SymbolInfo> {
        let Some(file) = self
            .by_path
            .get(rel_path)
            .and_then(|slot| self.files[*slot as usize].as_ref())
        else {
            return Vec::new();
        };
        file.symbols
            .iter()
            .map(|symbol| self.symbol_info(file, symbol))
            .collect()
    }

    /// The package and imports of the file at `rel_path`.
    pub fn file_header(&self, rel_path: &str) -> Option<(String, Vec<String>)> {
        let file = self.files[*self.by_path.get(rel_path)? as usize].as_ref()?;
        let imports = file
            .imports
            .iter()
            .map(|import| {
                let mut text = self.interner.resolve(import.path).to_owned();
                if import.wildcard {
                    text.push_str(".*");
                }
                if let Some(alias) = import.alias {
                    text.push_str(" as ");
                    text.push_str(self.interner.resolve(alias));
                }
                text
            })
            .collect();
        Some((self.interner.resolve(file.package).to_owned(), imports))
    }

    pub fn abs_path(&self, rel_path: &str) -> PathBuf {
        abs_path(&self.root, rel_path)
    }

    pub(crate) fn live_files(&self) -> impl Iterator<Item = &FileData> {
        self.files.iter().flatten()
    }

    fn rel_path_of(&self, slot: u32) -> &str {
        self.files[slot as usize]
            .as_ref()
            .map(|file| file.rel_path.as_str())
            .unwrap_or_default()
    }

    fn qualifier_matches(&self, qualifier: &[u8], file: &FileData, symbol: &Symbol) -> bool {
        let haystacks = [
            Some(self.interner.resolve(file.package)),
            Some(self.interner.resolve(symbol.container)),
            symbol
                .receiver
                .map(|receiver| self.interner.resolve(receiver)),
        ];
        let mut remaining = qualifier;
        for haystack in haystacks.into_iter().flatten() {
            for byte in haystack.bytes() {
                if let Some((first, rest)) = remaining.split_first()
                    && byte.to_ascii_lowercase() == *first
                {
                    remaining = rest;
                }
            }
            // A dot in the qualifier may span the package/container boundary.
            if let Some((b'.', rest)) = remaining.split_first() {
                remaining = rest;
            }
        }
        remaining.is_empty()
    }

    /// Whether `qualifier` (lowercase) names the symbol's enclosing declaration, extension
    /// receiver or package exactly, or as a dotted suffix: what `Outer.` alone lists.
    fn qualifier_is_scope(&self, qualifier: &[u8], file: &FileData, symbol: &Symbol) -> bool {
        let names_scope = |scope: &str| {
            let scope = scope.as_bytes();
            scope.len() >= qualifier.len()
                && scope[scope.len() - qualifier.len()..].eq_ignore_ascii_case(qualifier)
                && (scope.len() == qualifier.len()
                    || scope[scope.len() - qualifier.len() - 1] == b'.')
        };
        let receiver = symbol.receiver.map(|receiver| {
            let receiver = self.interner.resolve(receiver);
            receiver
                .split('<')
                .next()
                .unwrap_or(receiver)
                .trim_end_matches('?')
        });
        names_scope(self.interner.resolve(symbol.container))
            || receiver.is_some_and(names_scope)
            || (symbol.container.is_empty()
                && symbol.receiver.is_none()
                && names_scope(self.interner.resolve(file.package)))
    }

    pub(crate) fn symbol_info(&self, file: &FileData, symbol: &Symbol) -> SymbolInfo {
        let resolve = |sym: Sym| self.interner.resolve(sym).to_owned();
        SymbolInfo {
            name: resolve(symbol.name),
            kind: symbol.kind,
            visibility: symbol.visibility,
            flags: symbol.flags,
            container: resolve(symbol.container),
            receiver: symbol.receiver.map(resolve),
            package: resolve(file.package),
            rel_path: file.rel_path.clone(),
            language: file.language,
            origin: file.origin,
            module: resolve(file.module),
            source_set: resolve(file.source_set),
            name_pos: symbol.name_pos,
            start_line: symbol.start_line,
            end_line: symbol.end_line,
        }
    }

    /// Removes files, and every file under the paths that are directories, in one pass over the
    /// index. Returns how many files were removed.
    pub(crate) fn remove_paths(&mut self, rel_paths: &[String]) -> usize {
        let mut removed = 0;
        let mut dirs = HashSet::new();
        for rel_path in rel_paths {
            if self.remove_file(rel_path) {
                removed += 1;
            } else {
                dirs.insert(rel_path.as_str());
            }
        }
        if dirs.is_empty() {
            return removed;
        }
        let under: Vec<String> = self
            .by_path
            .keys()
            .filter(|path| {
                path.match_indices('/')
                    .any(|(slash, _)| dirs.contains(&path[..slash]))
            })
            .cloned()
            .collect();
        removed + under.iter().filter(|path| self.remove_file(path)).count()
    }

    fn remove_file(&mut self, rel_path: &str) -> bool {
        let Some(slot) = self.by_path.remove(rel_path) else {
            return false;
        };
        if let Some(file) = self.files[slot as usize].take() {
            self.symbol_count -= file.symbols.len();
        }
        self.free_slots.push(slot);
        true
    }

    pub(crate) fn insert_file_data(&mut self, file: FileData) {
        self.remove_file(&file.rel_path.clone());
        self.symbol_count += file.symbols.len();
        let rel_path = file.rel_path.clone();
        let slot = match self.free_slots.pop() {
            Some(slot) => {
                self.files[slot as usize] = Some(file);
                slot
            }
            None => {
                self.files.push(Some(file));
                (self.files.len() - 1) as u32
            }
        };
        self.by_path.insert(rel_path, slot);
    }

    fn insert_file(&mut self, found: FoundFile, raw: RawFile) {
        let Some(language) = Language::from_path(Path::new(&found.rel_path)) else {
            return;
        };
        let placement = place(&found.rel_path);
        let interner = &mut self.interner;
        let symbols = raw
            .symbols
            .into_iter()
            .map(|symbol| Symbol {
                name: interner.intern(&symbol.name),
                kind: symbol.kind,
                visibility: symbol.visibility,
                flags: symbol.flags,
                container: interner.intern(&symbol.container),
                receiver: symbol.receiver.map(|receiver| interner.intern(&receiver)),
                name_pos: symbol.name_pos,
                start_line: symbol.start_line,
                end_line: symbol.end_line,
            })
            .collect();
        let imports = raw
            .imports
            .into_iter()
            .map(|import| Import {
                path: interner.intern(&import.path),
                alias: import.alias.map(|alias| interner.intern(&alias)),
                wildcard: import.wildcard,
                is_static: import.is_static,
            })
            .collect();
        let file = FileData {
            module: interner.intern(&placement.module),
            source_set: interner.intern(&placement.source_set),
            package: interner.intern(&raw.package),
            rel_path: found.rel_path,
            language,
            origin: placement.origin,
            imports,
            symbols,
            mtime_ns: found.mtime_ns,
            size: found.size,
        };
        self.insert_file_data(file);
    }
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    encoded: u32,
    origin: Origin,
    name_len: u32,
    slot: u32,
    index: u32,
}

impl Candidate {
    fn tier(&self) -> MatchTier {
        match self.encoded >> 24 {
            5 => MatchTier::Exact,
            4 => MatchTier::Prefix,
            3 => MatchTier::Camel,
            2 => MatchTier::WordStart,
            _ => MatchTier::Fuzzy,
        }
    }

    fn score(&self) -> i32 {
        (self.encoded & 0x00FF_FFFF) as i32 - SCORE_OFFSET
    }
}

const SCORE_OFFSET: i32 = 1 << 23;

fn encode_match(tier: MatchTier, score: i32) -> u32 {
    let score = (score + SCORE_OFFSET).clamp(0, 0x00FF_FFFF) as u32;
    ((tier as u32) << 24) | score
}

/// Files parsed per [`PendingUpdate`] batch of a rescan. Batching bounds how many raw parse
/// results exist at once, which is most of the memory a cold build needs.
const REFRESH_BATCH: usize = 512;

/// Walks `root` and parses every file that is new or whose `(mtime, size)` differs from `stamps`,
/// handing the results to `apply` in batches. The first batch also drops indexed files that are
/// gone. Applying a batch only takes a moment, so a shared index can be locked per batch.
/// `apply` returns whether to go on; returning `false` cancels the remaining batches.
pub fn refresh_in_batches(
    root: &Path,
    stamps: &HashMap<String, (u64, u64)>,
    threads: usize,
    mut apply: impl FnMut(PendingUpdate) -> bool,
) {
    let found = scan(root, threads);
    let mut present = Some(found.iter().map(|file| file.rel_path.clone()).collect());
    let to_parse: Vec<FoundFile> = found
        .into_iter()
        .filter(|file| stamps.get(&file.rel_path) != Some(&(file.mtime_ns, file.size)))
        .collect();
    let pool = (to_parse.len() > SERIAL_PARSE_LIMIT)
        .then(|| worker_pool(threads))
        .flatten();
    let mut batches = to_parse.chunks(REFRESH_BATCH).peekable();
    if batches.peek().is_none() {
        apply(PendingUpdate {
            present,
            ..Default::default()
        });
        return;
    }
    for batch in batches {
        let go_on = apply(PendingUpdate {
            parsed: parse_all(root, batch.to_vec(), pool.as_ref()),
            removed: Vec::new(),
            present: present.take(),
        });
        if !go_on {
            return;
        }
    }
}

/// Parses the files among `paths` that exist (walking directories) and marks the rest removed.
pub fn prepare_changes(root: &Path, paths: &[PathBuf], threads: usize) -> PendingUpdate {
    let mut to_parse = Vec::new();
    let mut removed = Vec::new();
    for path in paths {
        if path.is_dir() {
            let walker = WalkBuilder::new(path)
                .hidden(true)
                .require_git(false)
                .filter_entry(|entry| {
                    !entry
                        .file_type()
                        .is_some_and(|file_type| file_type.is_dir())
                        || entry.file_name().to_str().is_some_and(is_walkable_dir)
                })
                .build();
            to_parse.extend(
                walker
                    .flatten()
                    .filter_map(|entry| stat_file(root, entry.path())),
            );
        } else if let Some(found) = stat_file(root, path) {
            to_parse.push(found);
        } else if !path.exists()
            && let Some(rel_path) = rel_path(root, path)
        {
            removed.push(rel_path);
        }
    }
    let pool = (to_parse.len() > SERIAL_PARSE_LIMIT)
        .then(|| worker_pool(threads))
        .flatten();
    PendingUpdate {
        parsed: parse_all(root, to_parse, pool.as_ref()),
        removed,
        present: None,
    }
}

/// Up to this many files are parsed on the calling thread.
const SERIAL_PARSE_LIMIT: usize = 4;

/// A bounded pool for one build or update, dropped (with its threads) afterwards.
fn worker_pool(threads: usize) -> Option<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads.clamp(1, MAX_THREADS))
        .thread_name(|i| format!("code-index-{i}"))
        .build()
        .inspect_err(|err| {
            log::warn!("[Code index] Couldn't start worker threads, parsing serially: {err}");
        })
        .ok()
}

fn parse_all(
    root: &Path,
    files: Vec<FoundFile>,
    pool: Option<&rayon::ThreadPool>,
) -> Vec<(FoundFile, RawFile)> {
    let parse = |file: FoundFile| {
        let raw = parse_file(root, &file).unwrap_or_default();
        (file, raw)
    };
    match pool {
        Some(pool) if files.len() > SERIAL_PARSE_LIMIT => {
            pool.install(|| files.into_par_iter().map(parse).collect())
        }
        _ => files.into_iter().map(parse).collect(),
    }
}

fn parse_file(root: &Path, file: &FoundFile) -> Option<RawFile> {
    if file.size > MAX_FILE_BYTES {
        return None;
    }
    let language = Language::from_path(Path::new(&file.rel_path))?;
    let source = std::fs::read(abs_path(root, &file.rel_path)).ok()?;
    extract(language, &source)
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
