//! Find Usages: files that mention the name are picked from the per-file identifier sets,
//! narrowed by what their package and imports let them see, then scanned. Mentions in files that
//! could mean another declaration with the same name are resolved one by one.

use std::collections::HashSet;

use rayon::prelude::*;

use crate::index::{CodeIndex, SymbolInfo, worker_pool};
use crate::layout::place;
use crate::lex::{for_each_identifier, line_starts, position_of};
use crate::model::{FileData, Origin, Position, Symbol, Visibility, flags};
use crate::resolve::{ParsedFile, Resolution, Resolver, SourceFile, same_declaration};

/// Files whose mentions are resolved one by one, at most; past this, mentions in files that see
/// the declaration count as approximate usages without resolving.
const MAX_RESOLVED_FILES: usize = 400;
/// Larger files aren't parsed to resolve mentions (parsing is most of a query's time, about
/// 1 ms per 7 KB); their mentions count as approximate usages when the file sees a target.
const MAX_RESOLVED_FILE_BYTES: u64 = 128 * 1024;
/// Files at most scanned on the calling thread.
const SERIAL_FILES: usize = 16;
/// Longest line text kept per usage.
const MAX_LINE_CHARS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    /// Path relative to the index root, with `/` separators.
    pub rel_path: String,
    /// Where the name starts.
    pub position: Position,
    /// The line, trimmed.
    pub line_text: String,
    pub origin: Origin,
    /// Resolved to the declaration, rather than a matching name in a file that can see it.
    pub exact: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsagesResult {
    pub name: String,
    /// The declarations whose usages these are (`expect`/`actual` counterparts included).
    pub targets: Vec<SymbolInfo>,
    /// Project code first, then tests and generated code; by path and position.
    pub usages: Vec<Usage>,
    /// Files whose text was scanned.
    pub files_scanned: usize,
    /// False when usages were cut at the limit.
    pub complete: bool,
}

/// How a candidate file is searched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scan {
    /// Every mention is a usage; exact when visibility alone identifies the declaration.
    All { exact: bool },
    /// Mentions may mean other declarations; resolve each.
    Resolve,
}

impl CodeIndex {
    /// Usages of the declaration the name at `position` in `file` refers to (or declares).
    /// `file` may hold unsaved text; other files are read from disk. Up to `limit` usages.
    pub fn find_usages(
        &self,
        file: &SourceFile,
        position: Position,
        limit: usize,
        threads: usize,
    ) -> Option<UsagesResult> {
        let definition = self.find_definition(file, position)?;
        let targets: Vec<SymbolInfo> = definition
            .definitions
            .iter()
            .map(|definition| definition.symbol.clone())
            .collect();
        if targets.is_empty() {
            return Some(UsagesResult {
                name: definition.name,
                targets,
                usages: Vec::new(),
                files_scanned: 0,
                complete: true,
            });
        }
        let only_local = definition.definitions.iter().all(|d| d.local)
            || targets.iter().all(|target| {
                target.visibility == Visibility::Private && target.rel_path == file.rel_path
            });
        let search = Search {
            index: self,
            name: &definition.name,
            targets: &targets,
            current: file,
        };

        let mut jobs: Vec<(Job, Scan)> = vec![(Job::Current, Scan::Resolve)];
        if !only_local {
            jobs.extend(search.candidate_files());
        }
        // Past the budget, files that mention a target's class count their mentions
        // approximately and the rest are left out.
        let mut budget = MAX_RESOLVED_FILES;
        jobs.retain_mut(|(job, scan)| {
            if *scan != Scan::Resolve {
                return true;
            }
            if budget > 0 {
                budget -= 1;
                return true;
            }
            *scan = Scan::All { exact: false };
            matches!(job, Job::Indexed(file) if search.mentions_target_class(file))
        });
        let files_scanned = jobs.len();

        let run = |(job, scan): &(Job, Scan)| search.scan(job, *scan);
        let pool = (jobs.len() > SERIAL_FILES)
            .then(|| worker_pool(threads))
            .flatten();
        let mut usages: Vec<Usage> = match &pool {
            Some(pool) => pool.install(|| jobs.par_iter().flat_map_iter(run).collect()),
            None => jobs.iter().flat_map(run).collect(),
        };
        usages.sort_by(|a, b| {
            (a.origin, &a.rel_path, a.position.line, a.position.column).cmp(&(
                b.origin,
                &b.rel_path,
                b.position.line,
                b.position.column,
            ))
        });
        usages.dedup_by(|a, b| a.rel_path == b.rel_path && a.position == b.position);
        let complete = usages.len() <= limit;
        usages.truncate(limit);
        Some(UsagesResult {
            name: definition.name,
            targets,
            usages,
            files_scanned,
            complete,
        })
    }
}

enum Job<'a> {
    /// The file the query came from, with its possibly unsaved text.
    Current,
    Indexed(&'a FileData),
}

struct Search<'a> {
    index: &'a CodeIndex,
    name: &'a str,
    targets: &'a [SymbolInfo],
    current: &'a SourceFile<'a>,
}

impl<'a> Search<'a> {
    /// Indexed files (other than the current one) that mention the name and may mean a target.
    fn candidate_files(&self) -> Vec<(Job<'a>, Scan)> {
        let Some(name) = self.index.interner.get(self.name) else {
            return Vec::new();
        };
        let others: Vec<(&FileData, &Symbol)> = self
            .index
            .declarations_named(name)
            .filter(|(file, symbol)| {
                !self.targets.iter().any(|target| {
                    target.rel_path == file.rel_path && target.name_pos == symbol.name_pos
                })
            })
            .collect();
        let declaring_other: HashSet<&str> = others
            .iter()
            .map(|(file, _)| file.rel_path.as_str())
            .collect();
        let is_member = self.targets.iter().all(|target| {
            !target.container.is_empty() && target.receiver.is_none() && !target.kind.is_type()
        });

        let mut jobs = Vec::new();
        for file in self.index.live_files() {
            if file.rel_path == self.current.rel_path
                || file.identifiers.binary_search(&name).is_err()
            {
                continue;
            }
            let declares_other = declaring_other.contains(file.rel_path.as_str());
            let is_target_file = self
                .targets
                .iter()
                .any(|target| target.rel_path == file.rel_path);
            let sees_target = self.sees_any_target(file);
            let scan = if is_target_file {
                Scan::Resolve
            } else if is_member {
                if others.is_empty() {
                    // A unique name: receivers whose type the file never names still reach it.
                    Scan::All { exact: sees_target }
                } else if self.mentions_target_class(file) {
                    Scan::Resolve
                } else {
                    // Resolving a member to this declaration takes its class's name in the file
                    // (a receiver type, a supertype); elsewhere it would only match by name.
                    continue;
                }
            } else if declares_other {
                Scan::Resolve
            } else if !sees_target {
                continue;
            } else if others
                .iter()
                .any(|(other, symbol)| self.sees(file, &self.index.symbol_info(other, symbol)))
            {
                Scan::Resolve
            } else {
                Scan::All { exact: true }
            };
            if self
                .targets
                .iter()
                .all(|target| !self.may_access(file, target))
            {
                continue;
            }
            let scan = match scan {
                Scan::Resolve if file.size > MAX_RESOLVED_FILE_BYTES => {
                    if !sees_target {
                        continue;
                    }
                    Scan::All { exact: false }
                }
                Scan::Resolve | Scan::All { .. } => scan,
            };
            jobs.push((Job::Indexed(file), scan));
        }
        // The parse budget goes to the files most likely to hold usages first.
        jobs.sort_by_key(|(job, _)| match job {
            Job::Current => false,
            Job::Indexed(file) => !self.mentions_target_class(file),
        });
        jobs
    }

    /// Whether `file` uses the name of a target's enclosing class (a member target) or of the
    /// target itself.
    fn mentions_target_class(&self, file: &FileData) -> bool {
        self.targets.iter().any(|target| {
            let class = target
                .container
                .rsplit('.')
                .next()
                .filter(|class| !class.is_empty());
            self.index
                .interner
                .get(class.unwrap_or(&target.name))
                .is_some_and(|class| file.identifiers.binary_search(&class).is_ok())
        })
    }

    fn sees_any_target(&self, file: &FileData) -> bool {
        self.targets.iter().any(|target| self.sees(file, target))
    }

    /// Whether visibility rules let `file` use `symbol` at all.
    fn may_access(&self, file: &FileData, symbol: &SymbolInfo) -> bool {
        match symbol.visibility {
            Visibility::Private => file.rel_path == symbol.rel_path,
            Visibility::Internal => self.index.interner.resolve(file.module) == symbol.module,
            Visibility::Public | Visibility::Protected | Visibility::PackagePrivate => true,
        }
    }

    /// Whether `file` can name `symbol` (or, for a member, its top-level class) without
    /// qualification: same file or package, or imported.
    fn sees(&self, file: &FileData, symbol: &SymbolInfo) -> bool {
        if file.rel_path == symbol.rel_path {
            return true;
        }
        if !self.may_access(file, symbol) {
            return false;
        }
        let interner = &self.index.interner;
        if interner.resolve(file.package) == symbol.package {
            return true;
        }
        let top_level = symbol
            .container
            .split('.')
            .next()
            .filter(|top| !top.is_empty());
        let fqn = symbol.qualified_name();
        let top_fqn = top_level.map(|top| join(&symbol.package, top));
        file.imports.iter().any(|import| {
            let path = interner.resolve(import.path);
            if import.wildcard {
                path == symbol.package && top_level.is_none()
                    || top_fqn
                        .as_deref()
                        .is_some_and(|top| path == top || path.starts_with(&format!("{top}.")))
            } else {
                path == fqn
                    || top_fqn
                        .as_deref()
                        .is_some_and(|top| path == top || path.starts_with(&format!("{top}.")))
            }
        })
    }

    fn scan(&self, job: &Job, scan: Scan) -> Vec<Usage> {
        let (rel_path, text, language, origin) = match job {
            Job::Current => (
                self.current.rel_path.to_owned(),
                self.current.text.to_owned(),
                self.current.language,
                place(self.current.rel_path).origin,
            ),
            Job::Indexed(file) => {
                let Ok(text) = std::fs::read_to_string(self.index.abs_path(&file.rel_path)) else {
                    return Vec::new();
                };
                (file.rel_path.clone(), text, file.language, file.origin)
            }
        };
        let mut offsets = Vec::new();
        for_each_identifier(&text, language, |identifier, offset| {
            if identifier == self.name {
                offsets.push(offset);
            }
        });
        if offsets.is_empty() {
            return Vec::new();
        }
        let starts = line_starts(&text);
        let usage = |offset: usize, exact: bool| {
            let position = position_of(&text, &starts, offset);
            Usage {
                rel_path: rel_path.clone(),
                position,
                line_text: line_text(&text, &starts, position.line),
                origin,
                exact,
            }
        };
        match scan {
            Scan::All { exact } => offsets
                .into_iter()
                .map(|offset| usage(offset, exact))
                .filter(|usage| !self.is_target_declaration(usage))
                .collect(),
            Scan::Resolve => {
                let source = SourceFile {
                    rel_path: &rel_path,
                    text: &text,
                    language,
                };
                let Some(parsed) = ParsedFile::parse(&source) else {
                    return Vec::new();
                };
                let resolver = Resolver::new(self.index, &parsed);
                // A name-only match among many same-named declarations is noise unless the file
                // at least names the target's class.
                let plausible = match job {
                    Job::Current => true,
                    Job::Indexed(file) => self.mentions_target_class(file),
                };
                offsets
                    .into_iter()
                    .filter_map(|offset| {
                        let result = resolver.resolve_at(offset)?;
                        if result.at_declaration {
                            return None;
                        }
                        let hit = result.definitions.iter().find(|definition| {
                            self.targets
                                .iter()
                                .any(|target| is_same_target(&definition.symbol, target))
                        })?;
                        let ambiguous =
                            hit.resolution == Resolution::Name && result.primary().count() > 1;
                        if ambiguous && !plausible {
                            return None;
                        }
                        Some(usage(offset, hit.resolution.is_exact() && !ambiguous))
                    })
                    .collect()
            }
        }
    }

    fn is_target_declaration(&self, usage: &Usage) -> bool {
        self.targets
            .iter()
            .any(|target| target.rel_path == usage.rel_path && target.name_pos == usage.position)
    }
}

/// The same declaration, also when one side comes from unsaved text whose positions differ.
fn is_same_target(found: &SymbolInfo, target: &SymbolInfo) -> bool {
    same_declaration(found, target)
        || (found.rel_path == target.rel_path
            && found.name == target.name
            && found.kind == target.kind
            && found.container == target.container
            && found.flags & (flags::EXPECT | flags::ACTUAL)
                == target.flags & (flags::EXPECT | flags::ACTUAL))
}

fn line_text(text: &str, starts: &[usize], line: u32) -> String {
    let start = starts[line as usize];
    let end = starts
        .get(line as usize + 1)
        .map_or(text.len(), |next| next.saturating_sub(1));
    let line = text[start..end].trim();
    if line.chars().count() > MAX_LINE_CHARS {
        let mut cut: String = line.chars().take(MAX_LINE_CHARS).collect();
        cut.push('…');
        cut
    } else {
        line.to_owned()
    }
}

fn join(package: &str, name: &str) -> String {
    if package.is_empty() {
        name.to_owned()
    } else {
        format!("{package}.{name}")
    }
}

#[cfg(test)]
#[path = "usages_tests.rs"]
mod tests;
