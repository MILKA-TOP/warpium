//! Find Usages: files that mention the name are picked from the per-file identifier sets,
//! narrowed by what their package and imports let them see, then scanned. Mentions in files that
//! could mean another declaration with the same name are resolved one by one.

use std::collections::HashSet;

use rayon::prelude::*;

use crate::extract::join_container;
use crate::hierarchy::sort_and_dedup;
use crate::index::{CodeIndex, SymbolInfo, worker_pool};
use crate::interner::Sym;
use crate::layout::place;
use crate::lex::{for_each_identifier, line_starts, position_of};
use crate::model::{FileData, Language, Origin, Position, Symbol, Visibility, flags};
use crate::resolve::{ParsedFile, Resolution, Resolver, SourceFile, same_declaration};

/// Most overriding members and counterparts whose usages a query also lists.
const MAX_VIA_TARGETS: usize = 200;
/// Overrides are looked up for at most this many targets.
const MAX_OVERRIDDEN_TARGETS: usize = 4;
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
    /// A usage of an overriding member or an `expect`/`actual` counterpart (one of
    /// [`UsagesResult::via`]) rather than of the declaration itself.
    pub via: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsagesResult {
    pub name: String,
    /// The declarations whose usages these are.
    pub targets: Vec<SymbolInfo>,
    /// Members overriding a member target, and the `expect`/`actual` counterparts of the
    /// targets, whose usages are listed too, marked [`Usage::via`], as the JetBrains IDEs list
    /// usages "via implementations".
    pub via: Vec<SymbolInfo>,
    /// Usages of the targets, then those of [`UsagesResult::via`]; within each, project code
    /// first, then tests and generated code; by path and position.
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
        let mut targets: Vec<SymbolInfo> = definition
            .primary()
            .map(|definition| definition.symbol.clone())
            .collect();
        if targets.is_empty() {
            return Some(UsagesResult {
                name: definition.name,
                targets,
                via: Vec::new(),
                usages: Vec::new(),
                files_scanned: 0,
                complete: true,
            });
        }
        let only_local = definition.definitions.iter().all(|d| d.local)
            || targets.iter().all(|target| {
                target.visibility == Visibility::Private && target.rel_path == file.rel_path
            });
        let mut via: Vec<SymbolInfo> = definition
            .definitions
            .iter()
            .filter(|definition| definition.counterpart)
            .map(|definition| definition.symbol.clone())
            .collect();
        // A name matched by many declarations (an unresolved call) is too vague for overrides.
        if !only_local && targets.len() <= MAX_OVERRIDDEN_TARGETS {
            for target in targets.iter().filter(|target| !target.kind.is_type()) {
                let room = MAX_VIA_TARGETS.saturating_sub(via.len());
                if room == 0 {
                    break;
                }
                via.extend(self.overriding_members(target, room).0);
            }
        }
        via.retain(|found| !targets.iter().any(|target| same_declaration(target, found)));
        sort_and_dedup(&mut via);
        via.truncate(MAX_VIA_TARGETS);
        let primary = targets.len();
        targets.extend(via.iter().cloned());
        let search = Search {
            index: self,
            name: &definition.name,
            target_classes: targets
                .iter()
                .map(|target| {
                    let class = target
                        .container
                        .rsplit('.')
                        .next()
                        .filter(|class| !class.is_empty());
                    self.interner.get(class.unwrap_or(&target.name))
                })
                .collect(),
            targets: &targets,
            primary,
            target_reach: targets.iter().map(Reach::of).collect(),
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
            (
                a.via,
                a.origin,
                &a.rel_path,
                a.position.line,
                a.position.column,
            )
                .cmp(&(
                    b.via,
                    b.origin,
                    &b.rel_path,
                    b.position.line,
                    b.position.column,
                ))
        });
        usages.dedup_by(|a, b| a.rel_path == b.rel_path && a.position == b.position);
        let complete = usages.len() <= limit;
        usages.truncate(limit);
        targets.truncate(primary);
        Some(UsagesResult {
            name: definition.name,
            targets,
            via,
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
    /// The declarations searched for: the query's own first, then [`UsagesResult::via`].
    targets: &'a [SymbolInfo],
    /// How many of `targets` are the query's own.
    primary: usize,
    /// Per target, the name a file must use to resolve a mention to it: its enclosing class
    /// for a member, else its own name (`None` when no file uses it).
    target_classes: Vec<Option<Sym>>,
    target_reach: Vec<Reach>,
    current: &'a SourceFile<'a>,
}

/// What a file needs to see a declaration, precomputed once per query.
struct Reach {
    rel_path: String,
    package: String,
    module: String,
    visibility: Visibility,
    fqn: String,
    /// For a member or nested type: its top-level class, and that with a trailing dot.
    top_fqn: Option<(String, String)>,
}

impl Reach {
    fn of(symbol: &SymbolInfo) -> Self {
        let top_fqn = symbol
            .container
            .split('.')
            .next()
            .filter(|top| !top.is_empty())
            .map(|top| {
                let top = join_container(&symbol.package, top);
                let prefix = format!("{top}.");
                (top, prefix)
            });
        Self {
            rel_path: symbol.rel_path.clone(),
            package: symbol.package.clone(),
            module: symbol.module.clone(),
            visibility: symbol.visibility,
            fqn: symbol.qualified_name(),
            top_fqn,
        }
    }
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
        let other_reach: Vec<Reach> = others
            .iter()
            .map(|(file, symbol)| Reach::of(&self.index.symbol_info(file, symbol)))
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
                    if file.size <= MAX_RESOLVED_FILE_BYTES && self.mentions_both_hierarchies(file)
                    {
                        // Naming both the declaration's class and an overriding subtype's, the
                        // file may call either; only resolving tells `DbRepo().load()` (via the
                        // override) from `repo.load()` on the interface.
                        Scan::Resolve
                    } else {
                        // A unique name: receivers whose type the file never names still reach
                        // it.
                        Scan::All { exact: sees_target }
                    }
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
            } else if other_reach.iter().any(|other| self.sees(file, other)) {
                Scan::Resolve
            } else {
                Scan::All { exact: true }
            };
            if self
                .target_reach
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
        jobs.sort_by_cached_key(|(job, _)| match job {
            Job::Current => false,
            Job::Indexed(file) => !self.mentions_target_class(file),
        });
        jobs
    }

    /// Whether `file` uses the name of a target's enclosing class (a member target) or of the
    /// target itself.
    fn mentions_target_class(&self, file: &FileData) -> bool {
        self.mentions_class_of(file, &self.target_classes)
    }

    /// [`Self::mentions_target_class`] for the query's own targets only.
    fn mentions_primary_class(&self, file: &FileData) -> bool {
        self.mentions_class_of(file, &self.target_classes[..self.primary])
    }

    /// Whether `file` names the class of one of the query's own targets and that of one of
    /// [`UsagesResult::via`] (an overriding subtype; `expect`/`actual` counterparts share the
    /// name, so they don't count).
    fn mentions_both_hierarchies(&self, file: &FileData) -> bool {
        let (primary, via) = self.target_classes.split_at(self.primary);
        let distinct_via: Vec<Option<Sym>> = via
            .iter()
            .filter(|class| class.is_some() && !primary.contains(class))
            .copied()
            .collect();
        !distinct_via.is_empty()
            && self.mentions_class_of(file, primary)
            && self.mentions_class_of(file, &distinct_via)
    }

    fn mentions_class_of(&self, file: &FileData, classes: &[Option<Sym>]) -> bool {
        classes
            .iter()
            .flatten()
            .any(|class| file.identifiers.binary_search(class).is_ok())
    }

    /// Whether `target` (an index into `targets`) is one of [`UsagesResult::via`].
    fn is_via(&self, target: usize) -> bool {
        target >= self.primary
    }

    fn sees_any_target(&self, file: &FileData) -> bool {
        self.target_reach
            .iter()
            .any(|target| self.sees(file, target))
    }

    /// Whether visibility rules let `file` use the declaration at all.
    fn may_access(&self, file: &FileData, reach: &Reach) -> bool {
        match reach.visibility {
            Visibility::Private => file.rel_path == reach.rel_path,
            Visibility::Internal => self.index.interner.resolve(file.module) == reach.module,
            Visibility::Public | Visibility::Protected | Visibility::PackagePrivate => true,
        }
    }

    /// Whether `file` can name the declaration (or, for a member or nested type, its top-level
    /// class) without qualification: same file or package, or imported.
    fn sees(&self, file: &FileData, reach: &Reach) -> bool {
        if file.rel_path == reach.rel_path {
            return true;
        }
        if !self.may_access(file, reach) {
            return false;
        }
        let interner = &self.index.interner;
        if interner.resolve(file.package) == reach.package {
            return true;
        }
        let names_top_class = |path: &str| {
            reach
                .top_fqn
                .as_ref()
                .is_some_and(|(top, prefix)| path == top || path.starts_with(prefix.as_str()))
        };
        file.imports.iter().any(|import| {
            let path = interner.resolve(import.path);
            if import.wildcard {
                // `p.*` brings in the package's top-level declarations, nested ones through them.
                path == reach.package || names_top_class(path)
            } else {
                path == reach.fqn || names_top_class(path)
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
        // Mentions that are the indexed declarations of the name (overrides, overloads, the
        // targets themselves) are never usages. Dropping them first spares parsing files that
        // only declare the name, such as the implementations of an interface. Only when the file
        // is unchanged since it was indexed, so positions still match.
        if let Job::Indexed(file) = job
            && file.size == text.len() as u64
            && let Some(name) = self.index.interner.get(self.name)
        {
            let declared: Vec<Position> = file
                .symbols
                .iter()
                .filter(|symbol| symbol.name == name)
                .map(|symbol| symbol.name_pos)
                .collect();
            if !declared.is_empty() {
                offsets.retain(|offset| !declared.contains(&position_of(&text, &starts, *offset)));
                if offsets.is_empty() {
                    return Vec::new();
                }
            }
        }
        // In files resolved one by one the declaration a mention resolves to decides; elsewhere
        // a mention counts as a usage of an override only where the file never names the
        // classes of the query's own targets.
        let via_by_file = match job {
            Job::Current => false,
            Job::Indexed(file) => {
                self.primary < self.targets.len() && !self.mentions_primary_class(file)
            }
        };
        let usage = |offset: usize, exact: bool, via: bool| {
            let position = position_of(&text, &starts, offset);
            Usage {
                rel_path: rel_path.clone(),
                position,
                line_text: line_text(&text, &starts, position.line),
                origin,
                exact,
                via,
            }
        };
        // A mention that looks like a declaration or a named argument may not be a usage at all
        // (parameters and locals aren't indexed); resolve the file instead of counting it.
        let scan = match scan {
            Scan::All { .. }
                if offsets.iter().any(|offset| {
                    may_declare_or_label(&text, *offset, self.name.len(), language)
                }) =>
            {
                Scan::Resolve
            }
            Scan::All { .. } | Scan::Resolve => scan,
        };
        match scan {
            Scan::All { exact } => offsets
                .into_iter()
                .map(|offset| usage(offset, exact, via_by_file))
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
                        // What the mention resolves to decides over its `expect`/`actual`
                        // counterparts, and the query's own targets over overrides. A receiver
                        // resolves to an override only when its type is written nearby (a
                        // constructor call, a typed or constructed local, parameter or property);
                        // otherwise (`makeRepo().load()`, chained calls) the mention resolves by
                        // name, where the query's own declaration wins, so it is listed as a
                        // direct usage: there is no type inference to do better.
                        let (target, hit) = result
                            .definitions
                            .iter()
                            .filter_map(|definition| {
                                let target = self.targets.iter().position(|target| {
                                    is_same_target(&definition.symbol, target)
                                })?;
                                Some((target, definition))
                            })
                            .min_by_key(|(target, definition)| {
                                (definition.counterpart, self.is_via(*target))
                            })?;
                        let ambiguous =
                            hit.resolution == Resolution::Name && result.primary().count() > 1;
                        if ambiguous && !plausible {
                            return None;
                        }
                        Some(usage(
                            offset,
                            hit.resolution.is_exact() && !ambiguous,
                            self.is_via(target),
                        ))
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

/// Whether the mention at `offset` reads like a declaration (`val x`, `x: Type`, `Type x`) or a
/// named argument (`f(x = 1)`), judging by the code around it.
fn may_declare_or_label(text: &str, offset: usize, len: usize, language: Language) -> bool {
    let before = text[..offset].trim_end();
    let after = text[offset + len..].trim_start();
    let previous_word = before
        .rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .next()
        .unwrap_or_default();
    let assigned = after.starts_with('=') && !after.starts_with("==");
    match language {
        Language::Kotlin => {
            assigned
                || (after.starts_with(':') && !after.starts_with("::"))
                || matches!(
                    previous_word,
                    "val" | "var" | "fun" | "class" | "object" | "interface" | "typealias"
                )
        }
        Language::Java => {
            const NOT_TYPES: &[&str] = &[
                "return",
                "new",
                "throw",
                "case",
                "else",
                "instanceof",
                "extends",
                "implements",
                "import",
                "package",
                "static",
                "assert",
                "yield",
            ];
            let after_type = before.ends_with(['>', ']'])
                || (!previous_word.is_empty()
                    && before.ends_with(previous_word)
                    && !NOT_TYPES.contains(&previous_word));
            assigned || after_type
        }
    }
}

#[cfg(test)]
#[path = "usages_tests.rs"]
mod tests;
