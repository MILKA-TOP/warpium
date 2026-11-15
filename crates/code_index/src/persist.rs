//! The on-disk snapshot of an index, loaded for a fast warm start.
//!
//! Layout: `MAGIC`, format version (u32 LE), the root path, a table of every string the index
//! references (deduplicated, so a symbol stores string ids), the files with their symbols and
//! imports, and `END_MAGIC`. Integers are LEB128 varints.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::index::CodeIndex;
use crate::interner::{Interner, Sym};
use crate::model::{FileData, Import, Language, Origin, Position, Symbol, SymbolKind, Visibility};

const MAGIC: &[u8; 4] = b"WPCI";
const END_MAGIC: &[u8; 4] = b"END!";
/// Bump whenever the layout or the extraction rules change, so stale snapshots are rebuilt.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("I/O error on the index snapshot")]
    Io(#[from] std::io::Error),
    #[error("index snapshot has format version {found}, expected {FORMAT_VERSION}")]
    VersionMismatch { found: u32 },
    #[error("index snapshot is for another root directory")]
    RootMismatch,
    #[error("index snapshot is corrupt: {0}")]
    Corrupt(&'static str),
}

impl CodeIndex {
    /// Writes a snapshot to `path` atomically (through a temporary file in the same directory).
    pub fn save(&self, path: &Path) -> Result<(), PersistError> {
        let bytes = self.encode();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_path = path.with_extension(format!("tmp-{}", std::process::id()));
        let result = std::fs::File::create(&tmp_path)
            .and_then(|mut file| file.write_all(&bytes).and_then(|()| file.sync_all()))
            .and_then(|()| std::fs::rename(&tmp_path, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
        Ok(result?)
    }

    /// Loads a snapshot written by [`CodeIndex::save`] for the index root `root`.
    pub fn load(path: &Path, root: &Path) -> Result<Self, PersistError> {
        let bytes = std::fs::read(path)?;
        Self::decode(&bytes, root)
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        // Only strings still referenced are written, which also drops ones left over from
        // edited or deleted files.
        let mut table = StringTable::default();
        for file in self.live_files() {
            table.add(file.module, &self.interner);
            table.add(file.source_set, &self.interner);
            table.add(file.package, &self.interner);
            for import in &file.imports {
                table.add(import.path, &self.interner);
                if let Some(alias) = import.alias {
                    table.add(alias, &self.interner);
                }
            }
            for symbol in &file.symbols {
                table.add(symbol.name, &self.interner);
                table.add(symbol.container, &self.interner);
                if let Some(receiver) = symbol.receiver {
                    table.add(receiver, &self.interner);
                }
            }
        }

        let mut out = Vec::with_capacity(64 + self.symbol_count * 12);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        write_str(&mut out, &self.root.to_string_lossy());
        write_varint(&mut out, table.strings.len() as u64);
        for string in &table.strings {
            write_str(&mut out, string);
        }
        write_varint(&mut out, self.by_path.len() as u64);
        for file in self.live_files() {
            write_str(&mut out, &file.rel_path);
            out.push(file.language as u8);
            out.push(file.origin as u8);
            write_varint(&mut out, table.id(file.module));
            write_varint(&mut out, table.id(file.source_set));
            write_varint(&mut out, table.id(file.package));
            write_varint(&mut out, file.mtime_ns);
            write_varint(&mut out, file.size);
            write_varint(&mut out, file.imports.len() as u64);
            for import in &file.imports {
                write_varint(&mut out, table.id(import.path));
                write_varint(&mut out, import.alias.map_or(0, |alias| table.id(alias) + 1));
                out.push(u8::from(import.wildcard) | (u8::from(import.is_static) << 1));
            }
            write_varint(&mut out, file.symbols.len() as u64);
            for symbol in &file.symbols {
                write_varint(&mut out, table.id(symbol.name));
                out.push(symbol.kind as u8);
                out.push(symbol.visibility as u8);
                out.push(symbol.flags);
                write_varint(&mut out, table.id(symbol.container));
                write_varint(
                    &mut out,
                    symbol.receiver.map_or(0, |receiver| table.id(receiver) + 1),
                );
                write_varint(&mut out, u64::from(symbol.name_pos.line));
                write_varint(&mut out, u64::from(symbol.name_pos.column));
                // Lines are stored relative to the name, which keeps them to one byte.
                write_varint(
                    &mut out,
                    u64::from(symbol.name_pos.line.saturating_sub(symbol.start_line)),
                );
                write_varint(
                    &mut out,
                    u64::from(symbol.end_line.saturating_sub(symbol.name_pos.line)),
                );
            }
        }
        out.extend_from_slice(END_MAGIC);
        out
    }

    pub(crate) fn decode(bytes: &[u8], root: &Path) -> Result<Self, PersistError> {
        let mut reader = Reader { bytes, pos: 0 };
        if reader.take(4)? != MAGIC {
            return Err(PersistError::Corrupt("bad magic"));
        }
        let version = u32::from_le_bytes(
            reader
                .take(4)?
                .try_into()
                .map_err(|_| PersistError::Corrupt("bad version"))?,
        );
        if version != FORMAT_VERSION {
            return Err(PersistError::VersionMismatch { found: version });
        }
        if Path::new(reader.str()?) != root {
            return Err(PersistError::RootMismatch);
        }

        let string_count = reader.len()?;
        let mut interner = Interner::default();
        let mut syms = Vec::with_capacity(string_count);
        for _ in 0..string_count {
            syms.push(interner.intern(reader.str()?));
        }
        let sym = |id: u64| -> Result<Sym, PersistError> {
            syms.get(id as usize)
                .copied()
                .ok_or(PersistError::Corrupt("string id out of range"))
        };
        let optional_sym = |id: u64| -> Result<Option<Sym>, PersistError> {
            id.checked_sub(1).map(sym).transpose()
        };

        let file_count = reader.len()?;
        let mut files = Vec::with_capacity(file_count);
        for _ in 0..file_count {
            let rel_path = reader.str()?.to_owned();
            let language = Language::from_u8(reader.byte()?)
                .ok_or(PersistError::Corrupt("bad language"))?;
            let origin =
                Origin::from_u8(reader.byte()?).ok_or(PersistError::Corrupt("bad origin"))?;
            let module = sym(reader.varint()?)?;
            let source_set = sym(reader.varint()?)?;
            let package = sym(reader.varint()?)?;
            let mtime_ns = reader.varint()?;
            let size = reader.varint()?;
            let import_count = reader.len()?;
            let mut imports = Vec::with_capacity(import_count);
            for _ in 0..import_count {
                let path = sym(reader.varint()?)?;
                let alias = optional_sym(reader.varint()?)?;
                let bits = reader.byte()?;
                imports.push(Import {
                    path,
                    alias,
                    wildcard: bits & 1 != 0,
                    is_static: bits & 2 != 0,
                });
            }
            let symbol_count = reader.len()?;
            let mut symbols = Vec::with_capacity(symbol_count);
            for _ in 0..symbol_count {
                let name = sym(reader.varint()?)?;
                let kind =
                    SymbolKind::from_u8(reader.byte()?).ok_or(PersistError::Corrupt("bad kind"))?;
                let visibility = Visibility::from_u8(reader.byte()?)
                    .ok_or(PersistError::Corrupt("bad visibility"))?;
                let flags = reader.byte()?;
                let container = sym(reader.varint()?)?;
                let receiver = optional_sym(reader.varint()?)?;
                let line = reader.u32()?;
                let column = reader.u32()?;
                let before = reader.u32()?;
                let after = reader.u32()?;
                symbols.push(Symbol {
                    name,
                    kind,
                    visibility,
                    flags,
                    container,
                    receiver,
                    name_pos: Position { line, column },
                    start_line: line.saturating_sub(before),
                    end_line: line.saturating_add(after),
                });
            }
            files.push(FileData {
                rel_path,
                language,
                origin,
                module,
                source_set,
                package,
                imports,
                symbols,
                mtime_ns,
                size,
            });
        }
        if reader.take(4)? != END_MAGIC || reader.pos != bytes.len() {
            return Err(PersistError::Corrupt("bad trailer"));
        }

        let mut index = CodeIndex::new(PathBuf::from(root));
        index.interner = interner;
        for file in files {
            index.insert_file_data(file);
        }
        Ok(index)
    }
}

/// Live strings in first-use order, with their ids in the snapshot.
#[derive(Default)]
struct StringTable {
    strings: Vec<String>,
    ids: HashMap<Sym, u64>,
}

impl StringTable {
    fn add(&mut self, sym: Sym, interner: &Interner) {
        if self.ids.contains_key(&sym) {
            return;
        }
        self.ids.insert(sym, self.strings.len() as u64);
        self.strings.push(interner.resolve(sym).to_owned());
    }

    fn id(&self, sym: Sym) -> u64 {
        self.ids[&sym]
    }
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn write_str(out: &mut Vec<u8>, value: &str) {
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], PersistError> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(PersistError::Corrupt("truncated"))?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, PersistError> {
        Ok(self.take(1)?[0])
    }

    fn varint(&mut self) -> Result<u64, PersistError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(PersistError::Corrupt("varint too long"))
    }

    fn u32(&mut self) -> Result<u32, PersistError> {
        u32::try_from(self.varint()?).map_err(|_| PersistError::Corrupt("value out of range"))
    }

    /// A collection length, bounded by the remaining bytes so corrupt input can't make us
    /// allocate huge vectors.
    fn len(&mut self) -> Result<usize, PersistError> {
        let len = self.varint()?;
        if len > (self.bytes.len() - self.pos) as u64 {
            return Err(PersistError::Corrupt("length out of range"));
        }
        Ok(len as usize)
    }

    fn str(&mut self) -> Result<&'a str, PersistError> {
        let len = self.len()?;
        std::str::from_utf8(self.take(len)?).map_err(|_| PersistError::Corrupt("bad utf-8"))
    }
}

#[cfg(test)]
#[path = "persist_tests.rs"]
mod tests;
