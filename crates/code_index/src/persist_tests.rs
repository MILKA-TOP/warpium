use std::collections::BTreeMap;

use tempfile::TempDir;

use super::*;
use crate::index::{SearchScope, SymbolInfo};

fn fixture_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// Every file's symbols, keyed by path, for comparing two indexes.
fn contents(index: &CodeIndex) -> BTreeMap<String, Vec<SymbolInfo>> {
    index
        .stamps()
        .into_keys()
        .map(|path| {
            let symbols = index.file_symbols(&path);
            (path, symbols)
        })
        .collect()
}

#[test]
fn round_trip_preserves_files_symbols_and_headers() {
    let root = fixture_root("kmp");
    let index = CodeIndex::build(&root, 2);
    let dir = TempDir::new().expect("temp dir");
    let snapshot = dir.path().join("nested").join("index.bin");

    index.save(&snapshot).expect("save");
    let loaded = CodeIndex::load(&snapshot, &root).expect("load");

    assert_eq!(contents(&loaded), contents(&index));
    assert_eq!(loaded.stamps(), index.stamps());
    let platform = "shared/src/commonMain/kotlin/com/example/shared/Platform.kt";
    assert_eq!(loaded.file_header(platform), index.file_header(platform));
    assert_eq!(
        loaded.search("UVM", SearchScope::Types, 5),
        index.search("UVM", SearchScope::Types, 5)
    );
}

#[test]
fn loaded_index_needs_no_reparse() {
    let root = fixture_root("plain");
    let index = CodeIndex::build(&root, 2);
    let mut loaded = CodeIndex::decode(&index.encode(), &root).expect("decode");

    let stats = loaded.refresh(2);

    assert_eq!(stats, crate::UpdateStats::default());
}

#[test]
fn snapshot_drops_strings_of_removed_files() {
    let root = fixture_root("plain");
    let mut index = CodeIndex::build(&root, 2);
    let before = index.encode().len();
    index.remove_path("src/main/kotlin");

    let after = CodeIndex::decode(&index.encode(), &root).expect("decode");

    assert!(index.encode().len() < before);
    assert_eq!(after.interner.get("UserViewModel"), None);
}

#[test]
fn rejects_another_format_version() {
    let root = fixture_root("plain");
    let mut bytes = CodeIndex::build(&root, 1).encode();
    bytes[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());

    let result = CodeIndex::decode(&bytes, &root);

    assert!(matches!(
        result,
        Err(PersistError::VersionMismatch { found }) if found == FORMAT_VERSION + 1
    ));
}

#[test]
fn rejects_a_snapshot_of_another_root() {
    let bytes = CodeIndex::build(fixture_root("plain"), 1).encode();

    let result = CodeIndex::decode(&bytes, &fixture_root("kmp"));

    assert!(matches!(result, Err(PersistError::RootMismatch)));
}

#[test]
fn rejects_a_truncated_snapshot() {
    let root = fixture_root("plain");
    let bytes = CodeIndex::build(&root, 1).encode();

    let result = CodeIndex::decode(&bytes[..bytes.len() - 10], &root);

    assert!(matches!(result, Err(PersistError::Corrupt(_))));
}

#[test]
fn rejects_garbage() {
    let result = CodeIndex::decode(b"not an index", Path::new("/"));

    assert!(matches!(result, Err(PersistError::Corrupt(_))));
}
