use crate::interner::Sym;

/// What a declaration is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum SymbolKind {
    Class = 0,
    Interface = 1,
    Object = 2,
    CompanionObject = 3,
    Enum = 4,
    Annotation = 5,
    Record = 6,
    TypeAlias = 7,
    Function = 8,
    Constructor = 9,
    Property = 10,
    EnumEntry = 11,
}

impl SymbolKind {
    pub const ALL: [SymbolKind; 12] = [
        SymbolKind::Class,
        SymbolKind::Interface,
        SymbolKind::Object,
        SymbolKind::CompanionObject,
        SymbolKind::Enum,
        SymbolKind::Annotation,
        SymbolKind::Record,
        SymbolKind::TypeAlias,
        SymbolKind::Function,
        SymbolKind::Constructor,
        SymbolKind::Property,
        SymbolKind::EnumEntry,
    ];

    /// Whether "Go to Class" lists this kind.
    pub fn is_type(self) -> bool {
        match self {
            SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::Object
            | SymbolKind::CompanionObject
            | SymbolKind::Enum
            | SymbolKind::Annotation
            | SymbolKind::Record
            | SymbolKind::TypeAlias => true,
            SymbolKind::Function
            | SymbolKind::Constructor
            | SymbolKind::Property
            | SymbolKind::EnumEntry => false,
        }
    }

    /// A short lowercase label, as Kotlin or Java spells the declaration.
    pub fn label(self) -> &'static str {
        match self {
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Object => "object",
            SymbolKind::CompanionObject => "companion object",
            SymbolKind::Enum => "enum",
            SymbolKind::Annotation => "annotation",
            SymbolKind::Record => "record",
            SymbolKind::TypeAlias => "typealias",
            SymbolKind::Function => "fun",
            SymbolKind::Constructor => "constructor",
            SymbolKind::Property => "property",
            SymbolKind::EnumEntry => "enum entry",
        }
    }

    pub(crate) fn from_u8(value: u8) -> Option<Self> {
        Self::ALL.get(usize::from(value)).copied()
    }
}

/// Declared visibility. Kotlin's default is public; Java's default is package-private.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Visibility {
    Public = 0,
    Internal = 1,
    Protected = 2,
    Private = 3,
    PackagePrivate = 4,
}

impl Visibility {
    pub fn label(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Internal => "internal",
            Visibility::Protected => "protected",
            Visibility::Private => "private",
            Visibility::PackagePrivate => "package-private",
        }
    }

    pub(crate) fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Visibility::Public),
            1 => Some(Visibility::Internal),
            2 => Some(Visibility::Protected),
            3 => Some(Visibility::Private),
            4 => Some(Visibility::PackagePrivate),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Language {
    Kotlin = 0,
    Java = 1,
}

impl Language {
    /// The language of an indexed source file (`.kt`, `.java`).
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "kt" => Some(Language::Kotlin),
            "java" => Some(Language::Java),
            _ => None,
        }
    }

    /// The language of a file navigation works in: indexed sources plus Kotlin scripts
    /// (`.kts`), which aren't indexed but can refer to indexed declarations.
    pub fn of_editable_path(path: &std::path::Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "kts" => Some(Language::Kotlin),
            _ => Self::from_path(path),
        }
    }

    pub(crate) fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Language::Kotlin),
            1 => Some(Language::Java),
            _ => None,
        }
    }
}

/// Where a file's code comes from, in ranking order: project sources first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Origin {
    #[default]
    Main = 0,
    Test = 1,
    Generated = 2,
}

impl Origin {
    pub(crate) fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Origin::Main),
            1 => Some(Origin::Test),
            2 => Some(Origin::Generated),
            _ => None,
        }
    }
}

/// Bit flags on a [`Symbol`].
pub mod flags {
    /// Kotlin `expect` declaration.
    pub const EXPECT: u8 = 1;
    /// Kotlin `actual` declaration.
    pub const ACTUAL: u8 = 1 << 1;
    /// Java `static` member.
    pub const STATIC: u8 = 1 << 2;
    /// Kotlin `data class`.
    pub const DATA: u8 = 1 << 3;
    /// Kotlin `sealed` class or interface.
    pub const SEALED: u8 = 1 << 4;
    /// Kotlin `const val`.
    pub const CONST: u8 = 1 << 5;
    /// Kotlin `override` member, or a Java method annotated `@Override`.
    pub const OVERRIDE: u8 = 1 << 6;
}

/// A position in a file: zero-based line and character column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

/// One declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: Sym,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    pub flags: u8,
    /// Dotted names of the enclosing declarations (`Outer.Inner`), empty at the top level.
    pub container: Sym,
    /// Receiver type of a Kotlin extension function or property, as written.
    pub receiver: Option<Sym>,
    /// Where the declared name starts.
    pub name_pos: Position,
    /// First line of the whole declaration, including annotations and modifiers.
    pub start_line: u32,
    /// Last line of the whole declaration.
    pub end_line: u32,
}

impl Symbol {
    pub fn has_flag(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// An `import` directive, recorded for definition lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    /// The imported path without the trailing `.*`.
    pub path: Sym,
    pub alias: Option<Sym>,
    pub wildcard: bool,
    /// Java `import static`.
    pub is_static: bool,
}

/// A supertype a class, interface or object declares, as written (`Base`, `Outer.Base`,
/// `com.example.Base`), with type arguments left out. Recorded for Go to Implementation and for
/// finding the overrides of a member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Supertype {
    /// Index of the declaring type in [`FileData::symbols`].
    pub symbol: u32,
    /// The last segment of the written name.
    pub name: Sym,
    /// What was written before the last segment (`Outer`, `com.example`), usually empty.
    pub qualifier: Sym,
}

/// The declarations and header of one parsed source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileData {
    /// Path relative to the index root, with `/` separators.
    pub rel_path: String,
    pub language: Language,
    pub origin: Origin,
    /// Gradle module directory relative to the root (`core/data`), empty for the root project.
    pub module: Sym,
    /// Source set (`commonMain`, `main`, `test`, …), empty when the path doesn't follow the
    /// `src/<set>/` convention.
    pub source_set: Sym,
    pub package: Sym,
    pub imports: Vec<Import>,
    pub symbols: Vec<Symbol>,
    /// Declared supertypes of the file's types, in symbol order.
    pub supertypes: Vec<Supertype>,
    /// Every distinct identifier used in the file's code, sorted, for finding usage candidates.
    pub identifiers: Vec<Sym>,
    /// Modification time in nanoseconds since the Unix epoch, for change detection.
    pub mtime_ns: u64,
    pub size: u64,
}
