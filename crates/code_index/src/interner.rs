use std::hash::BuildHasher as _;

use hashbrown::HashTable;
use rustc_hash::FxBuildHasher;

/// An interned string. [`Sym::EMPTY`] is the empty string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sym(pub(crate) u32);

impl Sym {
    pub const EMPTY: Sym = Sym(0);

    pub fn is_empty(self) -> bool {
        self == Sym::EMPTY
    }
}

/// Deduplicates names, packages and paths. Every string also gets a character mask that lets the
/// fuzzy matcher reject most names without looking at them.
///
/// Strings are stored back to back in one buffer and looked up through a table of ids, so each
/// string costs its bytes plus about 21 bytes, rather than a separate allocation and a map entry
/// holding a pointer to it (several times that, for the short names that dominate).
pub(crate) struct Interner {
    bytes: String,
    /// `(start, len)` of every string in `bytes`, by id.
    spans: Vec<(u32, u32)>,
    masks: Vec<u64>,
    ids: HashTable<Sym>,
    hasher: FxBuildHasher,
}

impl std::fmt::Debug for Interner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Interner")
            .field("strings", &self.spans.len())
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

impl Default for Interner {
    fn default() -> Self {
        let mut interner = Self {
            bytes: String::new(),
            spans: Vec::new(),
            masks: Vec::new(),
            ids: HashTable::new(),
            hasher: FxBuildHasher,
        };
        let empty = interner.intern("");
        debug_assert_eq!(empty, Sym::EMPTY);
        interner
    }
}

impl Interner {
    pub(crate) fn intern(&mut self, value: &str) -> Sym {
        let hash = self.hasher.hash_one(value);
        let Self {
            bytes,
            spans,
            masks,
            ids,
            hasher,
        } = self;
        let text = |sym: Sym| span_str(bytes, spans[sym.0 as usize]);
        if let Some(sym) = ids.find(hash, |sym| text(*sym) == value) {
            return *sym;
        }
        let sym = Sym(u32::try_from(spans.len()).expect("fewer than 2^32 strings"));
        let start = u32::try_from(bytes.len()).expect("interned strings fit in 4 GB");
        bytes.push_str(value);
        spans.push((start, value.len() as u32));
        masks.push(char_mask(value));
        ids.insert_unique(hash, sym, |sym| {
            hasher.hash_one(span_str(bytes, spans[sym.0 as usize]))
        });
        sym
    }

    pub(crate) fn get(&self, value: &str) -> Option<Sym> {
        let hash = self.hasher.hash_one(value);
        self.ids
            .find(hash, |sym| self.resolve(*sym) == value)
            .copied()
    }

    pub(crate) fn resolve(&self, sym: Sym) -> &str {
        span_str(&self.bytes, self.spans[sym.0 as usize])
    }

    pub(crate) fn mask(&self, sym: Sym) -> u64 {
        self.masks[sym.0 as usize]
    }

    pub(crate) fn len(&self) -> usize {
        self.spans.len()
    }

    /// Releases the spare capacity left by growing, once a build or load is done.
    pub(crate) fn shrink_to_fit(&mut self) {
        let Self {
            bytes,
            spans,
            masks,
            ids,
            hasher,
        } = self;
        bytes.shrink_to_fit();
        spans.shrink_to_fit();
        masks.shrink_to_fit();
        ids.shrink_to_fit(|sym| hasher.hash_one(span_str(bytes, spans[sym.0 as usize])));
    }

    /// Approximate heap bytes held.
    pub(crate) fn heap_bytes(&self) -> usize {
        self.bytes.capacity()
            + self.spans.capacity() * std::mem::size_of::<(u32, u32)>()
            + self.masks.capacity() * std::mem::size_of::<u64>()
            // A hashbrown slot holds the id and a control byte.
            + self.ids.capacity() * (std::mem::size_of::<Sym>() + 1)
    }
}

fn span_str(bytes: &str, (start, len): (u32, u32)) -> &str {
    &bytes[start as usize..(start + len) as usize]
}

/// One bit per lowercase ASCII letter, digit, `_` and `$`, plus one for any other character.
pub(crate) fn char_mask(value: &str) -> u64 {
    value.bytes().fold(0, |mask, byte| mask | char_bit(byte))
}

pub(crate) fn char_bit(byte: u8) -> u64 {
    match byte.to_ascii_lowercase() {
        lower @ b'a'..=b'z' => 1 << (lower - b'a'),
        digit @ b'0'..=b'9' => 1 << (26 + digit - b'0'),
        b'_' => 1 << 36,
        b'$' => 1 << 37,
        _ => 1 << 38,
    }
}
