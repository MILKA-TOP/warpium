//! The type hierarchy, from the supertypes each class, interface and object declares: subtypes of
//! a type and the members that override a member, for Go to Implementation and for the "via
//! implementations" usages of Find Usages.
//!
//! Nothing is precomputed: a query scans the per-file supertype lists (a few entries per type)
//! once per inheritance level, which takes well under a millisecond on 100k-symbol projects and
//! keeps no reverse map in memory.

use std::collections::HashSet;

use crate::extract::join_container;
use crate::index::{CodeIndex, SymbolInfo};
use crate::interner::Sym;
use crate::model::{FileData, Supertype, SymbolKind, Visibility, flags};
use crate::resolve::{SourceFile, same_declaration};

/// Most subtypes, overriding members or `actual`s a query collects.
pub(crate) const MAX_IMPLEMENTATIONS: usize = 500;
/// Inheritance levels followed below the starting type.
const MAX_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImplementationsResult {
    /// The name under the cursor.
    pub name: String,
    /// The declarations whose implementations these are.
    pub targets: Vec<SymbolInfo>,
    /// Subtypes of a type, members overriding a member, and the `actual`s of an `expect`
    /// declaration. Project code first, then by path and line.
    pub implementations: Vec<SymbolInfo>,
    /// False when implementations were cut at the limit.
    pub complete: bool,
}

/// A type whose subtypes are looked for, with what a supertype reference needs to mean it.
struct TypeKey {
    name: Sym,
    package: String,
    /// Enclosing declarations and name (`Outer.Inner`).
    path: String,
    /// Package and path.
    fqn: String,
    rel_path: String,
    /// The index holds no other type with this name, so a reference whose import can't be
    /// checked (a Java same-package type, a nested type seen through its outer class) means it.
    unique: bool,
}

impl CodeIndex {
    /// Implementations of the declaration the name at `position` in `file` refers to (or
    /// declares): subtypes of a type, overrides of a member, `actual`s of an `expect`.
    pub fn find_implementations(
        &self,
        file: &SourceFile,
        position: crate::model::Position,
    ) -> Option<ImplementationsResult> {
        let definition = self.find_definition(file, position)?;
        let targets: Vec<SymbolInfo> = definition
            .primary()
            .filter(|definition| !definition.local)
            .map(|definition| definition.symbol.clone())
            .collect();
        let mut implementations = Vec::new();
        let mut complete = true;
        for target in &targets {
            let (found, all) = if target.kind.is_type() {
                self.subtypes(target, MAX_IMPLEMENTATIONS)
            } else {
                self.overriding_members(target, MAX_IMPLEMENTATIONS)
            };
            implementations.extend(found);
            complete &= all;
            if target.has_flag(flags::EXPECT) {
                implementations.extend(
                    definition
                        .definitions
                        .iter()
                        .filter(|d| d.counterpart && d.symbol.has_flag(flags::ACTUAL))
                        .map(|d| d.symbol.clone()),
                );
            }
        }
        implementations.retain(|found| !targets.iter().any(|t| same_declaration(t, found)));
        sort_and_dedup(&mut implementations);
        if implementations.len() > MAX_IMPLEMENTATIONS {
            implementations.truncate(MAX_IMPLEMENTATIONS);
            complete = false;
        }
        Some(ImplementationsResult {
            name: definition.name,
            targets,
            implementations,
            complete,
        })
    }

    /// The types that extend or implement `class`, directly or through other project types, up
    /// to `limit`. The flag is false when the limit cut the list.
    pub fn subtypes(&self, class: &SymbolInfo, limit: usize) -> (Vec<SymbolInfo>, bool) {
        let Some(key) = self.type_key(
            &class.name,
            &class.package,
            &class.container,
            &class.rel_path,
        ) else {
            return (Vec::new(), true);
        };
        let (found, complete) = self.subtypes_of(key, Some(class), limit);
        let infos = found
            .into_iter()
            .filter_map(|(slot, index)| {
                let file = self.files[slot as usize].as_ref()?;
                Some(self.symbol_info(file, file.symbols.get(index as usize)?))
            })
            .collect();
        (infos, complete)
    }

    /// The members of subtypes that override `member` (a function or property of a class or
    /// interface), up to `limit`. Kotlin members need `override`; Java methods only need the name,
    /// since `@Override` is optional, so overloads count too.
    pub fn overriding_members(&self, member: &SymbolInfo, limit: usize) -> (Vec<SymbolInfo>, bool) {
        if !matches!(member.kind, SymbolKind::Function | SymbolKind::Property)
            || member.container.is_empty()
            || member.visibility == Visibility::Private
            || member.has_flag(flags::STATIC)
        {
            return (Vec::new(), true);
        }
        let Some(name) = self.interner.get(&member.name) else {
            return (Vec::new(), true);
        };
        let (owner_container, owner_name) = member
            .container
            .rsplit_once('.')
            .unwrap_or(("", member.container.as_str()));
        let Some(owner) = self.type_key(
            owner_name,
            &member.package,
            owner_container,
            &member.rel_path,
        ) else {
            return (Vec::new(), true);
        };
        // Overrides are at most one per subtype (overloads aside), so the subtype limit bounds
        // them too.
        let (subtypes, mut complete) = self.subtypes_of(owner, None, limit);
        let mut found = Vec::new();
        for (slot, index) in subtypes {
            let Some(file) = self.files[slot as usize].as_ref() else {
                continue;
            };
            let Some(subtype) = file.symbols.get(index as usize) else {
                continue;
            };
            let container = join_container(
                self.interner.resolve(subtype.container),
                self.interner.resolve(subtype.name),
            );
            let Some(container) = self.interner.get(&container) else {
                continue;
            };
            for symbol in &file.symbols {
                if symbol.name == name
                    && symbol.container == container
                    && symbol.kind == member.kind
                    && (file.language == crate::model::Language::Java
                        || symbol.has_flag(flags::OVERRIDE))
                {
                    found.push(self.symbol_info(file, symbol));
                }
            }
            if found.len() >= limit {
                complete = false;
                break;
            }
        }
        (found, complete)
    }

    /// `(file slot, symbol index)` of the subtypes of `start`, breadth first. `start_info`, when
    /// it is indexed, is never listed (a cycle through broken code).
    fn subtypes_of(
        &self,
        start: TypeKey,
        start_info: Option<&SymbolInfo>,
        limit: usize,
    ) -> (Vec<(u32, u32)>, bool) {
        let mut found = Vec::new();
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        if let Some(info) = start_info
            && let Some(slot) = self.by_path.get(&info.rel_path)
            && let Some(file) = self.files[*slot as usize].as_ref()
            && let Some(index) = file
                .symbols
                .iter()
                .position(|s| s.name_pos == info.name_pos)
        {
            seen.insert((*slot, index as u32));
        }
        let mut frontier = vec![start];
        for _ in 0..MAX_DEPTH {
            if frontier.is_empty() {
                break;
            }
            let names: HashSet<Sym> = frontier.iter().map(|target| target.name).collect();
            let mut next = Vec::new();
            for (slot, file) in self.files.iter().enumerate() {
                let Some(file) = file else { continue };
                for supertype in &file.supertypes {
                    let key = (slot as u32, supertype.symbol);
                    // The name check comes first: it rules out nearly every entry.
                    if !names.contains(&supertype.name)
                        || seen.contains(&key)
                        || !frontier.iter().any(|target| {
                            target.name == supertype.name && self.refers_to(file, supertype, target)
                        })
                    {
                        continue;
                    }
                    let Some(subtype) = file.symbols.get(supertype.symbol as usize) else {
                        continue;
                    };
                    seen.insert(key);
                    found.push(key);
                    if found.len() >= limit {
                        return (found, false);
                    }
                    let container = self.interner.resolve(subtype.container);
                    if let Some(key) = self.type_key(
                        self.interner.resolve(subtype.name),
                        self.interner.resolve(file.package),
                        container,
                        &file.rel_path,
                    ) {
                        next.push(key);
                    }
                }
            }
            frontier = next;
        }
        (found, true)
    }

    fn type_key(
        &self,
        name: &str,
        package: &str,
        container: &str,
        rel_path: &str,
    ) -> Option<TypeKey> {
        let name_sym = self.interner.get(name)?;
        let types_named = self
            .declarations_named(name_sym)
            .filter(|(_, symbol)| symbol.kind.is_type())
            .take(2)
            .count();
        let path = join_container(container, name);
        Some(TypeKey {
            name: name_sym,
            package: package.to_owned(),
            fqn: join_container(package, &path),
            path,
            rel_path: rel_path.to_owned(),
            unique: types_named <= 1,
        })
    }

    /// Whether the supertype `supertype` written in `file` can mean `target` (whose simple name
    /// it has): written qualified as a suffix of the target's name, or in a file that sees the
    /// target through its package or imports.
    fn refers_to(&self, file: &FileData, supertype: &Supertype, target: &TypeKey) -> bool {
        let qualifier = self.interner.resolve(supertype.qualifier);
        if !qualifier.is_empty() {
            let written = format!("{qualifier}.{}", self.interner.resolve(supertype.name));
            return target.fqn == written
                || target
                    .fqn
                    .strip_suffix(written.as_str())
                    .is_some_and(|prefix| prefix.ends_with('.'));
        }
        if file.rel_path == target.rel_path {
            return true;
        }
        let top_level = target.path.len() == self.interner.resolve(target.name).len();
        let package = self.interner.resolve(file.package);
        if top_level && package == target.package {
            return true;
        }
        let outer = target.fqn.rsplit_once('.').map(|(outer, _)| outer);
        let imported = file.imports.iter().any(|import| {
            let path = self.interner.resolve(import.path);
            if import.wildcard {
                (top_level && path == target.package) || Some(path) == outer
            } else {
                import.alias.is_none() && path == target.fqn
            }
        });
        imported || target.unique
    }
}

/// Project code first, then by path and line, without repeats.
pub(crate) fn sort_and_dedup(symbols: &mut Vec<SymbolInfo>) {
    symbols.sort_by(|a, b| {
        (a.origin, &a.rel_path, a.name_pos.line).cmp(&(b.origin, &b.rel_path, b.name_pos.line))
    });
    symbols.dedup_by(|a, b| same_declaration(a, b));
}

#[cfg(test)]
#[path = "hierarchy_tests.rs"]
mod tests;
