use std::collections::HashMap;
use std::sync::Arc;

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
#[derive(Debug)]
pub(crate) struct Interner {
    strings: Vec<Arc<str>>,
    masks: Vec<u64>,
    ids: HashMap<Arc<str>, Sym>,
}

impl Default for Interner {
    fn default() -> Self {
        let empty: Arc<str> = Arc::from("");
        Self {
            strings: vec![empty.clone()],
            masks: vec![0],
            ids: HashMap::from([(empty, Sym::EMPTY)]),
        }
    }
}

impl Interner {
    pub(crate) fn intern(&mut self, value: &str) -> Sym {
        if let Some(sym) = self.ids.get(value) {
            return *sym;
        }
        let sym = Sym(u32::try_from(self.strings.len()).expect("fewer than 2^32 strings"));
        let value: Arc<str> = Arc::from(value);
        self.masks.push(char_mask(&value));
        self.strings.push(value.clone());
        self.ids.insert(value, sym);
        sym
    }

    pub(crate) fn get(&self, value: &str) -> Option<Sym> {
        self.ids.get(value).copied()
    }

    pub(crate) fn resolve(&self, sym: Sym) -> &str {
        &self.strings[sym.0 as usize]
    }

    pub(crate) fn mask(&self, sym: Sym) -> u64 {
        self.masks[sym.0 as usize]
    }

    pub(crate) fn len(&self) -> usize {
        self.strings.len()
    }

    /// Approximate heap bytes held.
    pub(crate) fn heap_bytes(&self) -> usize {
        let string_bytes: usize = self.strings.iter().map(|s| s.len() + 16).sum();
        string_bytes
            + self.strings.capacity() * std::mem::size_of::<Arc<str>>()
            + self.masks.capacity() * std::mem::size_of::<u64>()
            // A hashbrown slot holds the key, the value and a control byte.
            + self.ids.capacity() * (std::mem::size_of::<(Arc<str>, Sym)>() + 1)
    }
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
