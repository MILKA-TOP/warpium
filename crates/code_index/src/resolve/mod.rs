//! Go to Definition without a compiler: names are resolved through the scopes a reader would
//! check — local variables, enclosing classes, the file, explicit imports, the package, star
//! imports — and member accesses through receivers whose type is written down nearby. What
//! can't be pinned down falls back to every declaration with the name, marked as such.

mod syntax;

use std::cell::Cell;
use std::collections::HashSet;

use arborium_tree_sitter::{Node, Tree};

use crate::extract::{RawFile, RawSymbol, extract_from_tree, parse, position};
use crate::index::{CodeIndex, SymbolInfo};
use crate::layout::{Placement, place};
use crate::lex::{line_starts, offset_of};
use crate::model::{Language, Origin, Position, SymbolKind, Visibility, flags};

use syntax::{
    EnclosingClass, LocalDecl, Lookup, Receiver, TypeNode, classify, dotted_name,
    enclosing_classes, find_local, property_type, receiver_shape, supertypes, type_path,
};
pub(crate) use syntax::{Want, identifier_name, is_identifier};

/// Most declarations a name-only fallback returns.
const MAX_FALLBACK: usize = 50;
/// Name lookups nest (a receiver's type, a supertype's name); past this depth they stop, which
/// also ends cycles such as a class whose supertype lookup reaches the class again.
const MAX_NESTING: u32 = 6;
/// Supertypes are searched for members only this many lookups deep.
const MAX_SUPERTYPE_NESTING: u32 = 2;

/// A source file as the editor has it (possibly unsaved), for resolving references in it.
#[derive(Clone, Copy, Debug)]
pub struct SourceFile<'a> {
    /// Path relative to the index root, with `/` separators.
    pub rel_path: &'a str,
    pub text: &'a str,
    pub language: Language,
}

/// How a definition was found, from most to least certain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Resolution {
    /// A local variable, parameter or local function.
    Local,
    /// A declaration in the same file or an enclosing class (or its declared supertypes).
    File,
    /// Through an explicit import, or written fully qualified.
    Import,
    /// In the same package.
    Package,
    /// Through a star import.
    StarImport,
    /// A member of the receiver's declared type.
    Receiver,
    /// Only the name matches; the declaration may be another one.
    Name,
}

impl Resolution {
    pub fn label(self) -> &'static str {
        match self {
            Resolution::Local => "local",
            Resolution::File => "file",
            Resolution::Import => "import",
            Resolution::Package => "package",
            Resolution::StarImport => "star import",
            Resolution::Receiver => "receiver type",
            Resolution::Name => "name only",
        }
    }

    /// Whether the declaration was identified rather than guessed by name.
    pub fn is_exact(self) -> bool {
        self != Resolution::Name
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    /// The declaration. Local variables and parameters get kind [`SymbolKind::Property`] (local
    /// functions [`SymbolKind::Function`]) and an empty container.
    pub symbol: SymbolInfo,
    pub resolution: Resolution,
    /// A local variable, parameter or local function rather than an indexed declaration.
    pub local: bool,
    /// The `expect` or `actual` counterpart of a definition listed before it, rather than a
    /// definition in its own right.
    pub counterpart: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefinitionResult {
    /// The name under the cursor.
    pub name: String,
    /// Best first; counterparts follow the definitions they belong to.
    pub definitions: Vec<Definition>,
    /// The cursor is on the name of a declaration (whose usages are the useful answer).
    pub at_declaration: bool,
}

impl DefinitionResult {
    /// The definitions that aren't merely counterparts.
    pub fn primary(&self) -> impl Iterator<Item = &Definition> {
        self.definitions
            .iter()
            .filter(|definition| !definition.counterpart)
    }
}

impl CodeIndex {
    /// The declarations the name at `position` (zero-based line and character column) in `file`
    /// refers to. `None` if there's no identifier there.
    pub fn find_definition(
        &self,
        file: &SourceFile,
        position: Position,
    ) -> Option<DefinitionResult> {
        let parsed = ParsedFile::parse(file)?;
        let resolver = Resolver::new(self, &parsed);
        let offset = offset_of(file.text, &line_starts(file.text), position)?;
        resolver.resolve_at(offset)
    }
}

/// A parsed source file with its header and declarations.
pub(crate) struct ParsedFile<'a> {
    pub rel_path: &'a str,
    pub text: &'a str,
    pub language: Language,
    pub tree: Tree,
    pub raw: RawFile,
    pub placement: Placement,
}

impl<'a> ParsedFile<'a> {
    pub(crate) fn parse(file: &SourceFile<'a>) -> Option<Self> {
        let tree = parse(file.language, file.text.as_bytes())?;
        let raw = extract_from_tree(file.language, &tree, file.text.as_bytes());
        Some(Self {
            rel_path: file.rel_path,
            text: file.text,
            language: file.language,
            tree,
            raw,
            placement: place(file.rel_path),
        })
    }

    fn source(&self) -> &'a [u8] {
        self.text.as_bytes()
    }
}

/// A declaration found for a name, before ranking.
#[derive(Clone, Debug)]
struct Candidate {
    info: SymbolInfo,
    local: bool,
}

pub(crate) struct Resolver<'a> {
    index: &'a CodeIndex,
    file: &'a ParsedFile<'a>,
    nesting: Cell<u32>,
}

/// Counts one level of [`Resolver::nesting`] while alive.
struct Nested<'r>(&'r Cell<u32>);

impl Drop for Nested<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(index: &'a CodeIndex, file: &'a ParsedFile<'a>) -> Self {
        Self {
            index,
            file,
            nesting: Cell::new(0),
        }
    }

    fn nest(&self) -> Option<Nested<'_>> {
        let depth = self.nesting.get();
        (depth < MAX_NESTING).then(|| {
            self.nesting.set(depth + 1);
            Nested(&self.nesting)
        })
    }

    /// Resolves the identifier at (or just before) the byte `offset`.
    pub(crate) fn resolve_at(&self, offset: usize) -> Option<DefinitionResult> {
        let root = self.file.tree.root_node();
        let at = |offset: usize| {
            root.descendant_for_byte_range(offset, offset)
                .filter(|node| is_identifier(node.kind()))
        };
        let id = at(offset).or_else(|| offset.checked_sub(1).and_then(at))?;
        self.resolve_identifier(id)
    }

    pub(crate) fn resolve_identifier(&self, id: Node) -> Option<DefinitionResult> {
        let source = self.file.source();
        let name = identifier_name(id, source).to_owned();
        let lookup = classify(self.file.language, id, source);
        let at_declaration = matches!(lookup, Lookup::Declaration);
        let (candidates, resolution) = match lookup {
            Lookup::Nothing => return None,
            Lookup::Declaration => (self.declaration_at(id), Resolution::File),
            Lookup::Qualified(fqn) => (self.qualified(&fqn, None), Resolution::Import),
            Lookup::Name(want) => self.name(&name, id, want, &|_| true),
            Lookup::QualifiedType(path) => self.type_path(&path, id),
            Lookup::Member { receiver, want } => self.member(receiver, &name, want, id),
        };
        Some(DefinitionResult {
            name,
            definitions: self.finish(candidates, resolution),
            at_declaration,
        })
    }

    /// The declaration whose name is `id`: from the file's declarations, else a local.
    fn declaration_at(&self, id: Node) -> Vec<Candidate> {
        let source = self.file.source();
        let name_pos = position(id, source);
        let declared = self
            .file
            .raw
            .symbols
            .iter()
            .find(|symbol| symbol.name_pos == name_pos)
            .map(|symbol| self.file_candidate(symbol));
        match declared {
            Some(candidate) => vec![candidate],
            None => vec![self.local_candidate(id, false)],
        }
    }

    /// Looks `name` up in the scopes around `at`, innermost first, and returns the first scope's
    /// matches that `accept` keeps.
    fn name(
        &self,
        name: &str,
        at: Node,
        want: Want,
        accept: &dyn Fn(&Candidate) -> bool,
    ) -> (Vec<Candidate>, Resolution) {
        let Some(_nested) = self.nest() else {
            return (Vec::new(), Resolution::Name);
        };
        let source = self.file.source();
        let keep = |candidates: Vec<Candidate>| -> Vec<Candidate> {
            candidates
                .into_iter()
                .filter(|candidate| candidate.local || wants(candidate.info.kind, want))
                .filter(|candidate| accept(candidate))
                .collect()
        };

        if want != Want::Type
            && let Some(local) = find_local(self.file.language, name, at, source)
            && (want == Want::Call || !local.is_function)
        {
            let candidates = keep(vec![self.local_decl_candidate(&local)]);
            if !candidates.is_empty() {
                return (candidates, Resolution::Local);
            }
        }

        let classes = enclosing_classes(self.file.language, at, source);
        for depth in (0..classes.len()).rev() {
            let container = container_path(&classes[..=depth]);
            let mut found = keep(self.file_members(&container, name));
            if found.is_empty() {
                found = keep(self.companion_members(&container, name));
            }
            if found.is_empty() && self.nesting.get() <= MAX_SUPERTYPE_NESTING {
                found = keep(self.supertype_members(&classes[depth], name, at));
            }
            if !found.is_empty() {
                return (found, Resolution::File);
            }
        }

        let found = keep(self.file_members("", name));
        if !found.is_empty() {
            return (found, Resolution::File);
        }

        let mut imported = Vec::new();
        for import in self
            .file
            .raw
            .imports
            .iter()
            .filter(|import| !import.wildcard)
        {
            let visible_name = import
                .alias
                .as_deref()
                .unwrap_or_else(|| import.path.rsplit('.').next().unwrap_or_default());
            if visible_name == name {
                // An alias names the imported declaration, whatever it's called.
                let declared = import.path.rsplit('.').next().unwrap_or_default();
                imported.extend(self.qualified(&import.path, Some(declared)));
            }
        }
        let found = keep(imported);
        if !found.is_empty() {
            return (found, Resolution::Import);
        }

        let found = keep(self.declarations(Some(&self.file.raw.package), Some(""), name));
        if !found.is_empty() {
            return (found, Resolution::Package);
        }

        let mut starred = Vec::new();
        for import in self
            .file
            .raw
            .imports
            .iter()
            .filter(|import| import.wildcard)
        {
            starred.extend(self.declarations(Some(&import.path), Some(""), name));
            for class in self.qualified(&import.path, None) {
                if class.info.kind.is_type() {
                    starred.extend(self.members_of(&class.info, name));
                }
            }
        }
        let found = keep(starred);
        if !found.is_empty() {
            return (found, Resolution::StarImport);
        }

        let mut everywhere = keep(self.declarations(None, None, name));
        everywhere.retain(|candidate| self.is_accessible(&candidate.info));
        everywhere.truncate(MAX_FALLBACK * 4);
        (everywhere, Resolution::Name)
    }

    /// A qualified type: the first segment in scope (or a package prefix), then nested types.
    fn type_path(&self, path: &[String], at: Node) -> (Vec<Candidate>, Resolution) {
        let fully_qualified = self.qualified(&path.join("."), None);
        if fully_qualified.iter().any(|c| c.info.kind.is_type()) {
            return (fully_qualified, Resolution::Import);
        }
        let Some((first, rest)) = path.split_first() else {
            return (Vec::new(), Resolution::Name);
        };
        let (mut types, resolution) = self.name(first, at, Want::Type, &|_| true);
        for segment in rest {
            types = types
                .iter()
                .flat_map(|outer| self.members_of(&outer.info, segment))
                .filter(|candidate| candidate.info.kind.is_type())
                .collect();
            if types.is_empty() {
                // `Outer.Inner` where `Outer` couldn't be found: any type named `Inner`.
                return self.name(path.last().expect("non-empty"), at, Want::Type, &|_| true);
            }
        }
        (types, resolution)
    }

    /// `receiver.name`: members of the receiver's type when it can be found, else a fully
    /// qualified name, else every member or extension with the name.
    fn member(
        &self,
        receiver: Node,
        name: &str,
        want: Want,
        at: Node,
    ) -> (Vec<Candidate>, Resolution) {
        let Some(_nested) = self.nest() else {
            return (Vec::new(), Resolution::Name);
        };
        // `com.example.Foo.bar`: a qualified name, unless its head is a name in scope.
        if let Some(dotted) = dotted_name(receiver, self.file.source()) {
            let qualified: Vec<Candidate> = self
                .qualified(&format!("{dotted}.{name}"), None)
                .into_iter()
                .filter(|candidate| wants(candidate.info.kind, want))
                .collect();
            let head = dotted.split('.').next().unwrap_or_default();
            if !qualified.is_empty() && self.name(head, at, Want::Value, &|_| true).0.is_empty() {
                return (qualified, Resolution::Import);
            }
        }

        if let Some(receiver_type) = self.receiver_type(receiver, at) {
            let mut found: Vec<Candidate> = receiver_type
                .types
                .iter()
                .flat_map(|class| {
                    let mut members = self.members_of(&class.info, name);
                    if receiver_type.is_static {
                        members.extend(self.companion_members_of(&class.info, name));
                    }
                    members
                })
                .filter(|candidate| wants(candidate.info.kind, want))
                .collect();
            if found.is_empty() {
                let type_names: HashSet<&str> = receiver_type
                    .types
                    .iter()
                    .map(|class| class.info.name.as_str())
                    .collect();
                let (extensions, resolution) = self.name(name, at, want, &|candidate| {
                    candidate
                        .info
                        .receiver
                        .as_deref()
                        .is_some_and(|receiver| type_names.contains(base_type_name(receiver)))
                });
                if !extensions.is_empty() && resolution != Resolution::Name {
                    return (extensions, resolution);
                }
            } else {
                found.dedup_by(|a, b| same_declaration(&a.info, &b.info));
                return (found, Resolution::Receiver);
            }
        }

        let (extensions, resolution) = self.name(name, at, want, &|candidate| {
            candidate.info.receiver.is_some()
        });
        if !extensions.is_empty() && resolution != Resolution::Name {
            return (extensions, resolution);
        }
        let mut everywhere: Vec<Candidate> = self
            .declarations(None, None, name)
            .into_iter()
            .filter(|candidate| {
                (!candidate.info.container.is_empty() || candidate.info.receiver.is_some())
                    && wants(candidate.info.kind, want)
                    && self.is_accessible(&candidate.info)
            })
            .collect();
        everywhere.truncate(MAX_FALLBACK * 4);
        (everywhere, Resolution::Name)
    }

    fn receiver_type(&self, receiver: Node, at: Node) -> Option<ReceiverType> {
        let source = self.file.source();
        let language = self.file.language;
        match receiver_shape(language, receiver) {
            Receiver::Identifier(identifier) => {
                let name = identifier_name(identifier, source);
                if let Some(local) = find_local(language, name, identifier, source) {
                    let type_node = local.type_node?;
                    return self.instance_of(type_node, at);
                }
                let classes = enclosing_classes(language, identifier, source);
                for class in classes.iter().rev() {
                    if let Some(type_node) = property_type(language, class.node, name, source) {
                        return self.instance_of(type_node, at);
                    }
                }
                if let Some(type_node) =
                    property_type(language, self.file.tree.root_node(), name, source)
                {
                    return self.instance_of(type_node, at);
                }
                let (candidates, _) = self.name(name, identifier, Want::Value, &|_| true);
                let types: Vec<Candidate> = candidates
                    .into_iter()
                    .filter(|candidate| candidate.info.kind.is_type())
                    .collect();
                (!types.is_empty()).then_some(ReceiverType {
                    types,
                    is_static: true,
                })
            }
            Receiver::This => {
                let classes = enclosing_classes(language, receiver, source);
                let container = container_path(&classes[..classes.len().saturating_sub(1)]);
                let class = classes.last()?;
                let types: Vec<Candidate> = self
                    .file
                    .raw
                    .symbols
                    .iter()
                    .filter(|symbol| symbol.container == container && symbol.name == class.name)
                    .map(|symbol| self.file_candidate(symbol))
                    .collect();
                (!types.is_empty()).then_some(ReceiverType {
                    types,
                    is_static: false,
                })
            }
            Receiver::Member { receiver, name } => {
                let (members, _) =
                    self.member(receiver, identifier_name(name, source), Want::Value, at);
                let types: Vec<Candidate> = members
                    .into_iter()
                    .filter(|candidate| candidate.info.kind.is_type())
                    .collect();
                (!types.is_empty()).then_some(ReceiverType {
                    types,
                    is_static: true,
                })
            }
            Receiver::Call(callee) => {
                let (candidates, _) =
                    self.name(identifier_name(callee, source), callee, Want::Call, &|_| {
                        true
                    });
                let types: Vec<Candidate> = candidates
                    .into_iter()
                    .filter(|candidate| candidate.info.kind.is_type())
                    .collect();
                (!types.is_empty()).then_some(ReceiverType {
                    types,
                    is_static: false,
                })
            }
            Receiver::New(type_node) => self.instance_of(TypeNode::Type(type_node), at),
            Receiver::Other => None,
        }
    }

    fn instance_of(&self, type_node: TypeNode, at: Node) -> Option<ReceiverType> {
        let path = type_path(type_node, self.file.source());
        if path.is_empty() {
            return None;
        }
        let (types, _) = self.type_path(&path, at);
        let types: Vec<Candidate> = types
            .into_iter()
            .filter(|candidate| candidate.info.kind.is_type())
            .collect();
        (!types.is_empty()).then_some(ReceiverType {
            types,
            is_static: false,
        })
    }

    /// Members named `name` of the supertypes `class` declares.
    fn supertype_members(&self, class: &EnclosingClass, name: &str, at: Node) -> Vec<Candidate> {
        let source = self.file.source();
        supertypes(self.file.language, class.node)
            .into_iter()
            .flat_map(|type_node| {
                let path = type_path(TypeNode::Type(type_node), source);
                if path.is_empty() {
                    return Vec::new();
                }
                let (types, _) = self.type_path(&path, at);
                types
                    .iter()
                    .filter(|candidate| candidate.info.kind.is_type())
                    .flat_map(|supertype| self.members_of(&supertype.info, name))
                    .collect()
            })
            .collect()
    }

    /// Members of the type `class` named `name`, including nested types.
    fn members_of(&self, class: &SymbolInfo, name: &str) -> Vec<Candidate> {
        let container = join(&class.container, &class.name);
        self.declarations(Some(&class.package), Some(&container), name)
    }

    /// Members named `name` of the companion object of `class`.
    fn companion_members_of(&self, class: &SymbolInfo, name: &str) -> Vec<Candidate> {
        let container = join(&class.container, &class.name);
        self.companions(&class.package, &container)
            .into_iter()
            .flat_map(|companion| {
                self.declarations(
                    Some(&class.package),
                    Some(&join(&container, &companion.info.name)),
                    name,
                )
            })
            .collect()
    }

    fn companions(&self, package: &str, container: &str) -> Vec<Candidate> {
        let mut found = Vec::new();
        self.for_each_declaration(Some(package), Some(container), None, &mut |candidate| {
            if candidate.info.kind == SymbolKind::CompanionObject {
                found.push(candidate);
            }
        });
        found
    }

    /// Declarations of the file itself named `name` directly in `container`.
    fn file_members(&self, container: &str, name: &str) -> Vec<Candidate> {
        self.file
            .raw
            .symbols
            .iter()
            .filter(|symbol| symbol.container == container && symbol.name == name)
            .map(|symbol| self.file_candidate(symbol))
            .collect()
    }

    /// Members named `name` of the file's companion objects in `container`.
    fn companion_members(&self, container: &str, name: &str) -> Vec<Candidate> {
        self.file
            .raw
            .symbols
            .iter()
            .filter(|symbol| {
                symbol.container == container && symbol.kind == SymbolKind::CompanionObject
            })
            .flat_map(|companion| self.file_members(&join(container, &companion.name), name))
            .collect()
    }

    /// Declarations matching a fully qualified name: a package then a declaration path, tried for
    /// every split. With `declared_name`, only declarations with that name (alias imports).
    fn qualified(&self, fqn: &str, declared_name: Option<&str>) -> Vec<Candidate> {
        let segments: Vec<&str> = fqn.split('.').collect();
        let Some((name, path)) = segments.split_last() else {
            return Vec::new();
        };
        if declared_name.is_some_and(|declared| declared != *name) {
            return Vec::new();
        }
        let mut found = Vec::new();
        for split in (0..=path.len()).rev() {
            let package = path[..split].join(".");
            let container = path[split..].join(".");
            found.extend(self.declarations(Some(&package), Some(&container), name));
            if !found.is_empty() {
                break;
            }
        }
        found
    }

    /// Declarations named `name` (or of any name when empty), optionally only in `package` and
    /// directly in `container`, from the file itself and the rest of the index.
    fn declarations(
        &self,
        package: Option<&str>,
        container: Option<&str>,
        name: &str,
    ) -> Vec<Candidate> {
        let mut found = Vec::new();
        let name = (!name.is_empty()).then_some(name);
        self.for_each_declaration(package, container, name, &mut |candidate| {
            found.push(candidate);
        });
        found
    }

    fn for_each_declaration(
        &self,
        package: Option<&str>,
        container: Option<&str>,
        name: Option<&str>,
        found: &mut dyn FnMut(Candidate),
    ) {
        let raw = &self.file.raw;
        if package.is_none_or(|package| package == raw.package) {
            for symbol in &raw.symbols {
                if container.is_none_or(|container| container == symbol.container)
                    && name.is_none_or(|name| name == symbol.name)
                {
                    found(self.file_candidate(symbol));
                }
            }
        }
        let interner = &self.index.interner;
        let lookup = |value: Option<&str>| match value {
            None => Some(None),
            Some(value) => interner.get(value).map(Some),
        };
        let (Some(package), Some(container), Some(name)) =
            (lookup(package), lookup(container), lookup(name))
        else {
            return;
        };
        let mut consider = |file: &crate::model::FileData, symbol: &crate::model::Symbol| {
            if package.is_none_or(|package| package == file.package)
                && container.is_none_or(|container| container == symbol.container)
                && file.rel_path != self.file.rel_path
            {
                found(Candidate {
                    info: self.index.symbol_info(file, symbol),
                    local: false,
                });
            }
        };
        match name {
            Some(name) => {
                for (file, symbol) in self.index.declarations_named(name) {
                    consider(file, symbol);
                }
            }
            None => {
                for file in self.index.live_files() {
                    for symbol in &file.symbols {
                        consider(file, symbol);
                    }
                }
            }
        }
    }

    /// Whether code in this file may use `symbol` (private declarations of other files can't).
    fn is_accessible(&self, symbol: &SymbolInfo) -> bool {
        symbol.rel_path == self.file.rel_path || symbol.visibility != Visibility::Private
    }

    fn file_candidate(&self, symbol: &RawSymbol) -> Candidate {
        Candidate {
            info: SymbolInfo {
                name: symbol.name.clone(),
                kind: symbol.kind,
                visibility: symbol.visibility,
                flags: symbol.flags,
                container: symbol.container.clone(),
                receiver: symbol.receiver.clone(),
                package: self.file.raw.package.clone(),
                rel_path: self.file.rel_path.to_owned(),
                language: self.file.language,
                origin: self.file.placement.origin,
                module: self.file.placement.module.clone(),
                source_set: self.file.placement.source_set.clone(),
                name_pos: symbol.name_pos,
                start_line: symbol.start_line,
                end_line: symbol.end_line,
            },
            local: false,
        }
    }

    fn local_decl_candidate(&self, local: &LocalDecl) -> Candidate {
        self.local_candidate(local.name, local.is_function)
    }

    fn local_candidate(&self, name: Node, is_function: bool) -> Candidate {
        let source = self.file.source();
        let declaration = name.parent().unwrap_or(name);
        let mut candidate = self.file_candidate(&RawSymbol {
            name: identifier_name(name, source).to_owned(),
            kind: if is_function {
                SymbolKind::Function
            } else {
                SymbolKind::Property
            },
            visibility: Visibility::Private,
            flags: 0,
            container: String::new(),
            receiver: None,
            name_pos: position(name, source),
            start_line: declaration.start_position().row as u32,
            end_line: declaration.end_position().row as u32,
        });
        candidate.local = true;
        candidate
    }

    /// Ranks the candidates, drops duplicates and groups `expect`/`actual` declarations.
    fn finish(&self, candidates: Vec<Candidate>, resolution: Resolution) -> Vec<Definition> {
        let mut candidates = candidates;
        // A constructor call names the class; secondary constructors are noise next to it.
        if candidates.iter().any(|c| c.info.kind.is_type()) {
            candidates.retain(|c| c.info.kind != SymbolKind::Constructor);
        }
        candidates.sort_by_cached_key(|candidate| self.rank(&candidate.info));
        let mut unique: Vec<Candidate> = Vec::new();
        for candidate in candidates {
            if !unique
                .iter()
                .any(|seen| same_declaration(&seen.info, &candidate.info))
            {
                unique.push(candidate);
            }
        }

        let mut definitions: Vec<Definition> = Vec::new();
        for candidate in unique {
            if definitions
                .iter()
                .any(|definition| same_declaration(&definition.symbol, &candidate.info))
            {
                continue;
            }
            let resolution = if candidate.local {
                Resolution::Local
            } else {
                resolution
            };
            if candidate.info.has_flag(flags::EXPECT | flags::ACTUAL) {
                let mut group = vec![candidate.info.clone()];
                group.extend(self.counterparts(&candidate.info));
                group.sort_by_cached_key(|member| self.rank(member));
                let primary = self.primary_of(&group);
                definitions.push(Definition {
                    symbol: group[primary].clone(),
                    resolution,
                    local: false,
                    counterpart: false,
                });
                for (index, member) in group.into_iter().enumerate() {
                    if index != primary
                        && !definitions
                            .iter()
                            .any(|d| same_declaration(&d.symbol, &member))
                    {
                        definitions.push(Definition {
                            symbol: member,
                            resolution,
                            local: false,
                            counterpart: true,
                        });
                    }
                }
            } else {
                definitions.push(Definition {
                    symbol: candidate.info,
                    resolution,
                    local: candidate.local,
                    counterpart: false,
                });
            }
            if definitions.len() >= MAX_FALLBACK {
                break;
            }
        }
        definitions
    }

    /// In an `expect`/`actual` group: the `actual` of the reference's own source set, else the
    /// `expect` declaration, else the best ranked.
    fn primary_of(&self, group: &[SymbolInfo]) -> usize {
        let own_set = |symbol: &SymbolInfo| {
            symbol.module == self.file.placement.module
                && symbol.source_set == self.file.placement.source_set
        };
        group
            .iter()
            .position(|symbol| symbol.has_flag(flags::ACTUAL) && own_set(symbol))
            .or_else(|| {
                group
                    .iter()
                    .position(|symbol| symbol.has_flag(flags::EXPECT))
            })
            .unwrap_or(0)
    }

    fn counterparts(&self, symbol: &SymbolInfo) -> Vec<SymbolInfo> {
        let mut found = Vec::new();
        self.for_each_declaration(
            Some(&symbol.package),
            Some(&symbol.container),
            Some(&symbol.name),
            &mut |candidate| {
                if candidate.info.has_flag(flags::EXPECT | flags::ACTUAL)
                    && candidate.info.kind == symbol.kind
                    && !same_declaration(&candidate.info, symbol)
                {
                    found.push(candidate.info);
                }
            },
        );
        found
    }

    /// Sort key: declarations the reference can see first (KMP source sets), then project code
    /// over tests (unless the reference is a test), the reference's module, and path order.
    fn rank(&self, symbol: &SymbolInfo) -> (bool, bool, bool, Origin, String, u32) {
        let here = &self.file.placement;
        let same_module = symbol.module == here.module;
        let hidden_platform_set = same_module
            && symbol.source_set != here.source_set
            && is_platform_main_set(&symbol.source_set)
            && !here
                .source_set
                .starts_with(symbol.source_set.trim_end_matches("Main"));
        let origin_mismatch = here.origin == Origin::Main && symbol.origin == Origin::Test;
        (
            hidden_platform_set,
            origin_mismatch,
            !same_module,
            symbol.origin,
            symbol.rel_path.clone(),
            symbol.name_pos.line,
        )
    }
}

struct ReceiverType {
    types: Vec<Candidate>,
    /// The receiver names the type itself (`Foo.bar`), not an instance.
    is_static: bool,
}

/// A KMP platform source set (`androidMain`, `iosMain`, …), whose declarations other source sets
/// of the module can't see.
fn is_platform_main_set(source_set: &str) -> bool {
    source_set.ends_with("Main") && source_set != "commonMain" && source_set != "main"
}

fn wants(kind: SymbolKind, want: Want) -> bool {
    match want {
        Want::Type => kind.is_type(),
        Want::Call => match kind {
            SymbolKind::Function
            | SymbolKind::Constructor
            | SymbolKind::Property
            | SymbolKind::Class
            | SymbolKind::Record
            | SymbolKind::Annotation
            | SymbolKind::TypeAlias
            | SymbolKind::Object => true,
            SymbolKind::Interface
            | SymbolKind::CompanionObject
            | SymbolKind::Enum
            | SymbolKind::EnumEntry => false,
        },
        Want::Value => !matches!(kind, SymbolKind::Function | SymbolKind::Constructor),
    }
}

/// Whether two infos are the same declaration (same file and name position).
pub(crate) fn same_declaration(a: &SymbolInfo, b: &SymbolInfo) -> bool {
    a.rel_path == b.rel_path && a.name_pos == b.name_pos
}

fn container_path(classes: &[EnclosingClass]) -> String {
    classes
        .iter()
        .map(|class| class.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn join(container: &str, name: &str) -> String {
    if container.is_empty() {
        name.to_owned()
    } else {
        format!("{container}.{name}")
    }
}

/// `List<Foo>?` → `List`, `kotlin.String` → `String`.
fn base_type_name(type_text: &str) -> &str {
    let without_arguments = type_text.split('<').next().unwrap_or(type_text);
    without_arguments
        .trim_end_matches('?')
        .rsplit('.')
        .next()
        .unwrap_or(without_arguments)
        .trim()
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
