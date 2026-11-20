//! Code completion without a compiler: the names visible at the cursor, from the same scopes Go to
//! Definition walks, plus members of a receiver whose type is written down nearby and declarations
//! from the rest of the index that would need an import. Research prototype for
//! `docs/warpium/research/code-completion.md`; not wired into the editor.

use std::collections::HashSet;
use std::ops::Range;

use arborium_tree_sitter::Node;

use super::syntax::{Lookup, classify, dotted_name, enclosing_classes, supertypes, type_path};
use super::{
    Candidate, ParsedFile, Resolution, Resolver, SourceFile, TypeNode, Want, base_type_name,
    is_identifier, same_declaration, wants,
};
use crate::extract::join_container;
use crate::fuzzy::{MatchTier, Pattern};
use crate::index::{CodeIndex, SymbolInfo};
use crate::interner::Sym;
use crate::lex::{for_each_identifier, is_keyword, line_starts, offset_of};
use crate::model::{Language, Position, SymbolKind, Visibility, flags};

/// Inserted after the typed prefix so that the name being typed parses as an identifier in its
/// real context (`foo.` alone is a syntax error), as IntelliJ's completion does.
const DUMMY: &str = "WarpiumCompletionDummy";
/// Most names from the rest of the index expanded per query (for imports, or an unknown receiver).
const MAX_GLOBAL_NAMES: usize = 200;
/// Supertypes are followed this many levels for inherited members.
const MAX_SUPERTYPE_DEPTH: u32 = 3;

/// Where a completion comes from, in ranking order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CompletionSource {
    /// A local variable, parameter or local function.
    Local,
    /// A member of the receiver's type or its supertypes.
    Member,
    /// The file or an enclosing class (or its supertypes).
    Scope,
    Import,
    Package,
    StarImport,
    /// An extension function or property declared for the receiver's type.
    Extension,
    /// A top-level declaration that is not imported yet.
    NotImported,
    /// A name already used in the file that the index doesn't declare (a library name).
    Word,
}

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub name: String,
    /// The declaration; `None` for [`CompletionSource::Word`].
    pub symbol: Option<SymbolInfo>,
    pub source: CompletionSource,
    pub tier: MatchTier,
    /// The fully qualified name to import when the item is chosen.
    pub import: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CompletionResult {
    /// The part of the name before the cursor, which the chosen item replaces.
    pub prefix: String,
    pub replace_from: Position,
    /// Completing after `receiver.`.
    pub member_access: bool,
    /// The receiver's type couldn't be found: members are matched by name only.
    pub approximate: bool,
    pub items: Vec<CompletionItem>,
}

impl CodeIndex {
    /// Completions for the name being typed at `position` (zero-based line and character column)
    /// in `file`, best first. `None` where nothing is completed: in a comment or string, or where a
    /// new declaration is being named.
    pub fn complete(
        &self,
        file: &SourceFile,
        position: Position,
        limit: usize,
    ) -> Option<CompletionResult> {
        let starts = line_starts(file.text);
        let cursor = offset_of(file.text, &starts, position)?;
        let start = prefix_start(file.text, cursor);
        let prefix = &file.text[start..cursor];
        if prefix.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let mut text = String::with_capacity(file.text.len() + DUMMY.len());
        text.push_str(&file.text[..cursor]);
        text.push_str(DUMMY);
        // A Java statement being typed has no `;` yet, and without it the name is an error node.
        let rest_of_line = file.text[cursor..].split('\n').next().unwrap_or_default();
        if file.language == Language::Java && rest_of_line.trim().is_empty() {
            text.push(';');
        }
        text.push_str(&file.text[cursor..]);
        let source = SourceFile {
            rel_path: file.rel_path,
            text: &text,
            language: file.language,
        };
        let parsed = ParsedFile::parse(&source)?;
        let id = parsed
            .tree
            .root_node()
            .descendant_for_byte_range(start, start + 1)
            .filter(|node| is_identifier(node.kind()))?;
        let resolver = Resolver::new(self, &parsed);
        let completer = Completer {
            resolver: &resolver,
            pattern: Pattern::new(prefix),
            at: id,
        };
        let (items, member_access, approximate) = match classify(file.language, id, parsed.source())
        {
            Lookup::Member { receiver, .. } => {
                let (items, approximate) = completer.members_of_receiver(receiver);
                (items, true, approximate)
            }
            Lookup::Name(want) => {
                let want = if want == Want::Value { Want::Any } else { want };
                (completer.names_in_scope(want), false, false)
            }
            Lookup::QualifiedType(path) => {
                (completer.nested_types(&path[..path.len() - 1]), true, false)
            }
            Lookup::Qualified(_) | Lookup::Declaration | Lookup::Nothing => return None,
        };
        let column = file.text[starts[position.line as usize]..start]
            .chars()
            .count();
        Some(CompletionResult {
            prefix: prefix.to_owned(),
            replace_from: Position {
                line: position.line,
                column: column as u32,
            },
            member_access,
            approximate,
            items: rank(items, limit),
        })
    }
}

struct Completer<'r, 'a, 'tree> {
    resolver: &'r Resolver<'a>,
    /// `None` when nothing has been typed yet.
    pattern: Option<Pattern>,
    /// The identifier being typed, with [`DUMMY`] appended.
    at: Node<'tree>,
}

impl Completer<'_, '_, '_> {
    fn tier(&self, name: &str) -> Option<MatchTier> {
        if name.contains(DUMMY) {
            return None;
        }
        match &self.pattern {
            None => Some(MatchTier::Prefix),
            Some(pattern) => pattern
                .match_name(name)
                .map(|found| found.tier)
                .filter(|tier| *tier >= MatchTier::WordStart),
        }
    }

    fn item(&self, info: SymbolInfo, source: CompletionSource) -> Option<CompletionItem> {
        let tier = self.tier(&info.name)?;
        let import = (source == CompletionSource::NotImported
            || source == CompletionSource::Extension && !self.is_visible(&info))
        .then(|| info.qualified_name());
        Some(CompletionItem {
            name: info.name.clone(),
            symbol: Some(info),
            source,
            tier,
            import,
        })
    }

    /// `receiver.`: members of the receiver's type and its supertypes, extensions for them, and
    /// the declarations of a package or class written as a dotted name.
    fn members_of_receiver(&self, receiver: Node) -> (Vec<CompletionItem>, bool) {
        let resolver = self.resolver;
        let mut items = Vec::new();
        if let Some(receiver_type) = resolver.receiver_type(receiver, self.at) {
            let mut type_names = HashSet::new();
            let mut seen = HashSet::new();
            for class in &receiver_type.types {
                self.collect_members(
                    &class.info,
                    receiver_type.is_static,
                    0,
                    &mut seen,
                    &mut type_names,
                    &mut items,
                );
            }
            if !receiver_type.is_static {
                self.collect_extensions(&type_names, &mut items);
            }
            return (items, false);
        }
        if let Some(dotted) = dotted_name(receiver, resolver.file.source()) {
            let declared = self.declared_in(&dotted, "", &|name, _| self.tier(name).is_some());
            if !declared.is_empty() {
                for info in declared {
                    if resolver.is_accessible(&info) {
                        items.extend(self.item(info, CompletionSource::Member));
                    }
                }
                return (items, false);
            }
        }
        // Unknown receiver type: members and extensions of any type with the best matching names.
        // Without a typed prefix that would be the whole index.
        if self.pattern.is_none() {
            return (items, true);
        }
        let index = resolver.index;
        for sym in self.best_global_names(&HashSet::new()) {
            for (file, symbol) in index.declarations_named(sym) {
                let is_member = !symbol.container.is_empty() || symbol.receiver.is_some();
                if is_member
                    && !symbol.kind.is_type()
                    && symbol.kind != SymbolKind::Constructor
                    && symbol.visibility != Visibility::Private
                {
                    let info = index.symbol_info(file, symbol);
                    items.extend(self.item(info, CompletionSource::Member));
                }
            }
        }
        (items, true)
    }

    /// Members of `class` (static access lists nested types, companion members, enum entries and
    /// Java statics), then those its supertypes declare.
    fn collect_members(
        &self,
        class: &SymbolInfo,
        is_static: bool,
        depth: u32,
        seen: &mut HashSet<(String, u32, u32)>,
        type_names: &mut HashSet<String>,
        items: &mut Vec<CompletionItem>,
    ) {
        let key = (
            class.rel_path.clone(),
            class.name_pos.line,
            class.name_pos.column,
        );
        if !seen.insert(key) {
            return;
        }
        type_names.insert(class.name.clone());
        let resolver = self.resolver;
        let container = join_container(&class.container, &class.name);
        let is_object = matches!(class.kind, SymbolKind::Object | SymbolKind::CompanionObject);
        let keep = |name: &str, kind: SymbolKind| {
            kind == SymbolKind::CompanionObject || self.tier(name).is_some()
        };
        for info in self.declared_in(&class.package, &container, &keep) {
            if info.kind == SymbolKind::Constructor || !resolver.is_accessible(&info) {
                continue;
            }
            let static_member = info.kind.is_type()
                || info.kind == SymbolKind::EnumEntry
                || info.has_flag(flags::STATIC);
            if is_object || static_member == is_static {
                if info.kind == SymbolKind::CompanionObject {
                    if is_static {
                        let companion = info.clone();
                        self.collect_members(&companion, false, depth, seen, type_names, items);
                    }
                    continue;
                }
                items.extend(self.item(info, CompletionSource::Member));
            }
        }
        if (is_static && !is_object) || depth >= MAX_SUPERTYPE_DEPTH {
            return;
        }
        for supertype in self.supertypes_of(class) {
            self.collect_members(&supertype, false, depth + 1, seen, type_names, items);
        }
    }

    /// The types `class` declares as supertypes, resolved through its file's package and imports.
    fn supertypes_of(&self, class: &SymbolInfo) -> Vec<SymbolInfo> {
        let resolver = self.resolver;
        let index = resolver.index;
        let (written, imports): (Vec<String>, Vec<String>) =
            if class.rel_path == resolver.file.rel_path {
                let raw = &resolver.file.raw;
                let written = raw
                    .symbols
                    .iter()
                    .position(|symbol| symbol.name_pos == class.name_pos)
                    .map(|position| {
                        raw.supertypes
                            .iter()
                            .filter(|(symbol, _)| *symbol as usize == position)
                            .map(|(_, written)| written.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let imports = raw
                    .imports
                    .iter()
                    .map(|import| import_text(&import.path, import.wildcard))
                    .collect();
                (written, imports)
            } else {
                let Some(file) = index
                    .by_path
                    .get(&class.rel_path)
                    .and_then(|slot| index.files[*slot as usize].as_ref())
                else {
                    return Vec::new();
                };
                let Some(position) = file
                    .symbols
                    .iter()
                    .position(|symbol| symbol.name_pos == class.name_pos)
                else {
                    return Vec::new();
                };
                let written = file
                    .supertypes
                    .iter()
                    .filter(|supertype| supertype.symbol as usize == position)
                    .map(|supertype| {
                        join_container(
                            index.interner.resolve(supertype.qualifier),
                            index.interner.resolve(supertype.name),
                        )
                    })
                    .collect();
                let imports = file
                    .imports
                    .iter()
                    .map(|import| import_text(index.interner.resolve(import.path), import.wildcard))
                    .collect();
                (written, imports)
            };

        let mut found = Vec::new();
        for written in written {
            let name = base_type_name(&written);
            let types: Vec<SymbolInfo> = resolver
                .declarations(None, None, name)
                .into_iter()
                .map(|candidate| candidate.info)
                .filter(|info| info.kind.is_type())
                .collect();
            let visible: Vec<SymbolInfo> = types
                .iter()
                .filter(|info| {
                    let qualified = info.qualified_name();
                    qualified == written
                        || info.package == class.package
                        || imports.iter().any(|import| {
                            *import == qualified || *import == format!("{}.*", info.package)
                        })
                })
                .cloned()
                .collect();
            if !visible.is_empty() {
                found.extend(visible);
            } else if types.len() == 1 {
                found.extend(types);
            }
        }
        found
    }

    /// Extension functions and properties declared for any of `type_names`.
    fn collect_extensions(&self, type_names: &HashSet<String>, items: &mut Vec<CompletionItem>) {
        let index = self.resolver.index;
        for file in index.live_files() {
            for symbol in &file.symbols {
                let Some(receiver) = symbol.receiver else {
                    continue;
                };
                if symbol.visibility == Visibility::Private
                    || !type_names.contains(base_type_name(index.interner.resolve(receiver)))
                {
                    continue;
                }
                let info = index.symbol_info(file, symbol);
                items.extend(self.item(info, CompletionSource::Extension));
            }
        }
    }

    /// Whether the current file sees a top-level declaration without a new import.
    fn is_visible(&self, info: &SymbolInfo) -> bool {
        let raw = &self.resolver.file.raw;
        info.package == raw.package
            || raw.imports.iter().any(|import| {
                if import.wildcard {
                    import.path == info.package
                } else {
                    import.path == info.qualified_name()
                }
            })
    }

    /// A plain name: everything in scope (locals, enclosing classes and their supertypes, the
    /// file, imports, the package), then top-level declarations of the index that need an import,
    /// then names the file already uses that the index doesn't know. Until something is typed, the
    /// package, star imports and the rest of the index are left out: they can be thousands.
    fn names_in_scope(&self, want: Want) -> Vec<CompletionItem> {
        let resolver = self.resolver;
        let file = resolver.file;
        let source = file.source();
        let matches = |name: &str| self.tier(name).is_some() && !is_keyword(name, file.language);

        // Names that need the scope walk: locals (any identifier earlier in the enclosing
        // function), the file's own declarations and what the enclosing classes inherit.
        let mut scoped: HashSet<String> = HashSet::new();
        let body = enclosing_function(file.language, self.at);
        let cursor = self.at.start_byte();
        let mut words = Vec::new();
        for_each_identifier(file.text, file.language, |name, offset| {
            if body
                .as_ref()
                .is_some_and(|body| body.contains(&offset) && offset < cursor)
                && matches(name)
            {
                scoped.insert(name.to_owned());
            }
            words.push(name);
        });
        for symbol in &file.raw.symbols {
            if matches(&symbol.name) {
                scoped.insert(symbol.name.clone());
            }
        }
        for class in enclosing_classes(file.language, self.at, source) {
            for type_node in supertypes(file.language, class.node) {
                let path = type_path(TypeNode::Type(type_node), source);
                if path.is_empty() {
                    continue;
                }
                let mut seen = HashSet::new();
                let mut type_names = HashSet::new();
                let mut inherited = Vec::new();
                for supertype in resolver.type_path(&path, self.at).0 {
                    self.collect_members(
                        &supertype.info,
                        false,
                        1,
                        &mut seen,
                        &mut type_names,
                        &mut inherited,
                    );
                }
                scoped.extend(inherited.into_iter().map(|item| item.name));
            }
        }

        let mut items = Vec::new();
        let mut listed: HashSet<String> = HashSet::new();
        for name in &scoped {
            let (candidates, resolution) = resolver.name(name, self.at, want, &|candidate| {
                resolver.is_accessible(&candidate.info)
            });
            let source = match resolution {
                Resolution::Local => CompletionSource::Local,
                Resolution::File => CompletionSource::Scope,
                Resolution::Import => CompletionSource::Import,
                Resolution::Package => CompletionSource::Package,
                Resolution::StarImport => CompletionSource::StarImport,
                Resolution::Receiver => CompletionSource::Member,
                Resolution::Name => continue,
            };
            for Candidate { info, local } in candidates {
                let source = if local {
                    CompletionSource::Local
                } else {
                    source
                };
                items.extend(self.item(info, source));
            }
            listed.insert(name.clone());
        }

        // Imports and the package are classified by where they come from, without a scope walk.
        let mut imported = Vec::new();
        for import in &file.raw.imports {
            if import.wildcard {
                if self.pattern.is_none() {
                    continue;
                }
                for info in self.declared_in(&import.path, "", &|name, _| matches(name)) {
                    if resolver.is_accessible(&info) && wants(info.kind, want) {
                        imported.extend(self.item(info, CompletionSource::StarImport));
                    }
                }
                continue;
            }
            let declared = import.path.rsplit('.').next().unwrap_or_default();
            let visible = import.alias.as_deref().unwrap_or(declared);
            let Some(tier) = self.tier(visible) else {
                continue;
            };
            for candidate in resolver.qualified(&import.path, Some(declared)) {
                if wants(candidate.info.kind, want) {
                    imported.push(CompletionItem {
                        name: visible.to_owned(),
                        symbol: Some(candidate.info),
                        source: CompletionSource::Import,
                        tier,
                        import: None,
                    });
                }
            }
        }
        if self.pattern.is_some() {
            for info in self.declared_in(&file.raw.package, "", &|name, _| matches(name)) {
                if wants(info.kind, want) {
                    imported.extend(self.item(info, CompletionSource::Package));
                }
            }
        }
        imported.retain(|item| !listed.contains(&item.name));
        listed.extend(imported.iter().map(|item| item.name.clone()));
        items.extend(imported);

        if self.pattern.is_none() {
            return items;
        }
        self.collect_not_imported(want, &listed, &mut items);

        let index = resolver.index;
        let declared = |name: &str| {
            index
                .interner
                .get(name)
                .is_some_and(|sym| index.by_name.contains_key(&sym))
        };
        let mut seen_words = HashSet::new();
        for word in words {
            if !listed.contains(word)
                && seen_words.insert(word)
                && matches(word)
                && !declared(word)
                && let Some(tier) = self.tier(word)
            {
                items.push(CompletionItem {
                    name: word.to_owned(),
                    symbol: None,
                    source: CompletionSource::Word,
                    tier,
                    import: None,
                });
            }
        }
        items
    }

    /// Declarations directly in `container` of `package` (in the current file and the index) that
    /// `keep` accepts by name and kind, checked before anything is allocated for them.
    fn declared_in(
        &self,
        package: &str,
        container: &str,
        keep: &dyn Fn(&str, SymbolKind) -> bool,
    ) -> Vec<SymbolInfo> {
        let resolver = self.resolver;
        let raw = &resolver.file.raw;
        let mut found = Vec::new();
        if raw.package == package {
            for symbol in &raw.symbols {
                if symbol.container == container && keep(&symbol.name, symbol.kind) {
                    found.push(resolver.file_candidate(symbol).info);
                }
            }
        }
        let index = resolver.index;
        let (Some(package), Some(container)) =
            (index.interner.get(package), index.interner.get(container))
        else {
            return found;
        };
        for file in index.live_files() {
            if file.package != package || file.rel_path == resolver.file.rel_path {
                continue;
            }
            for symbol in &file.symbols {
                if symbol.container == container
                    && keep(index.interner.resolve(symbol.name), symbol.kind)
                {
                    found.push(index.symbol_info(file, symbol));
                }
            }
        }
        found
    }

    /// Top-level declarations of the index whose names match but aren't in scope.
    fn collect_not_imported(
        &self,
        want: Want,
        in_scope: &HashSet<String>,
        items: &mut Vec<CompletionItem>,
    ) {
        let index = self.resolver.index;
        for sym in self.best_global_names(in_scope) {
            for (file, symbol) in index.declarations_named(sym) {
                if symbol.container.is_empty()
                    && symbol.receiver.is_none()
                    && symbol.visibility != Visibility::Private
                    && wants(symbol.kind, want)
                {
                    let info = index.symbol_info(file, symbol);
                    items.extend(self.item(info, CompletionSource::NotImported));
                }
            }
        }
    }

    /// The best [`MAX_GLOBAL_NAMES`] names declared anywhere in the index (outside `excluded`),
    /// matched only as a prefix or by camel humps from the first word, so a one-letter prefix
    /// stays cheap: only names starting with the same letter are matched at all.
    fn best_global_names(&self, excluded: &HashSet<String>) -> Vec<Sym> {
        let Some(pattern) = &self.pattern else {
            return Vec::new();
        };
        let Some(first) = pattern.first_byte() else {
            return Vec::new();
        };
        let index = self.resolver.index;
        let mut matches = Vec::new();
        for &sym in index.by_name.keys() {
            let name = index.interner.resolve(sym);
            if !name
                .as_bytes()
                .first()
                .is_some_and(|b| b.eq_ignore_ascii_case(&first))
            {
                continue;
            }
            if let Some(found) = pattern.match_name(name)
                && found.tier >= MatchTier::Camel
                && !excluded.contains(name)
            {
                matches.push((found.tier, found.score, name.len(), sym));
            }
        }
        let order = |a: &(MatchTier, i32, usize, Sym), b: &(MatchTier, i32, usize, Sym)| {
            b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2))
        };
        if matches.len() > MAX_GLOBAL_NAMES {
            matches.select_nth_unstable_by(MAX_GLOBAL_NAMES - 1, order);
            matches.truncate(MAX_GLOBAL_NAMES);
        }
        matches.into_iter().map(|(_, _, _, sym)| sym).collect()
    }

    /// `Outer.` or `pkg.Outer.` in a type position: the nested types.
    fn nested_types(&self, qualifier: &[String]) -> Vec<CompletionItem> {
        let resolver = self.resolver;
        let mut items = Vec::new();
        for outer in resolver.type_path(qualifier, self.at).0 {
            let container = join_container(&outer.info.container, &outer.info.name);
            let keep = |name: &str, kind: SymbolKind| kind.is_type() && self.tier(name).is_some();
            for info in self.declared_in(&outer.info.package, &container, &keep) {
                if resolver.is_accessible(&info) {
                    items.extend(self.item(info, CompletionSource::Member));
                }
            }
        }
        items
    }
}

/// Where the identifier ending at `cursor` starts.
fn prefix_start(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(cursor, |(start, _)| start)
}

/// The byte range of the outermost function, constructor, accessor or initializer around `at`:
/// where its locals can be declared.
fn enclosing_function(language: Language, at: Node) -> Option<Range<usize>> {
    let mut found = None;
    let mut current = at.parent();
    while let Some(node) = current {
        let is_function = match language {
            Language::Kotlin => matches!(
                node.kind(),
                "function_declaration"
                    | "secondary_constructor"
                    | "getter"
                    | "setter"
                    | "anonymous_initializer"
            ),
            Language::Java => matches!(
                node.kind(),
                "method_declaration"
                    | "constructor_declaration"
                    | "compact_constructor_declaration"
                    | "static_initializer"
            ),
        };
        if is_function {
            found = Some(node.byte_range());
        }
        current = node.parent();
    }
    found
}

fn import_text(path: &str, wildcard: bool) -> String {
    if wildcard {
        format!("{path}.*")
    } else {
        path.to_owned()
    }
}

/// Best first: prefix matches before camel-hump and later-word matches, then by source (what's
/// closest in scope first), project code before tests and generated code, then shorter names.
/// Duplicates (a declaration reached twice) are dropped.
fn rank(mut items: Vec<CompletionItem>, limit: usize) -> Vec<CompletionItem> {
    let bucket = |tier: MatchTier| match tier {
        MatchTier::Exact | MatchTier::Prefix => 0,
        MatchTier::Camel => 1,
        MatchTier::WordStart | MatchTier::Fuzzy => 2,
    };
    items.sort_by(|a, b| {
        bucket(a.tier)
            .cmp(&bucket(b.tier))
            .then(a.source.cmp(&b.source))
            .then_with(|| {
                let origin = |item: &CompletionItem| item.symbol.as_ref().map(|s| s.origin);
                origin(a).cmp(&origin(b))
            })
            .then(a.name.len().cmp(&b.name.len()))
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut unique: Vec<CompletionItem> = Vec::with_capacity(items.len().min(limit));
    for item in items {
        let duplicate = unique
            .iter()
            .any(|seen| match (&seen.symbol, &item.symbol) {
                (Some(a), Some(b)) => same_declaration(a, b),
                (None, None) => seen.name == item.name,
                (Some(_), None) | (None, Some(_)) => false,
            });
        if !duplicate {
            unique.push(item);
            if unique.len() == limit {
                break;
            }
        }
    }
    unique
}

#[cfg(test)]
#[path = "complete_tests.rs"]
mod tests;
