use string_offset::ByteOffset;
use warp_ripgrep::search::Match as RipgrepMatch;

use super::*;

fn matched(file: &str, line: &str, submatch_start: usize) -> RipgrepMatch {
    RipgrepMatch {
        file_path: PathBuf::from(file),
        line_number: 7,
        line_text: line.to_owned(),
        submatches: vec![Submatch {
            byte_start: ByteOffset::from(submatch_start),
            byte_end: ByteOffset::from(submatch_start + 3),
        }],
    }
}

#[test]
fn the_match_is_listed_relative_to_its_root() {
    let roots = vec![PathBuf::from("/repo")];
    let item = TextMatchSearchItem::new(&roots, matched("/repo/src/Foo.kt", "val x = 1", 4), 1.)
        .expect("the file is under the root");

    assert_eq!(item.root, PathBuf::from("/repo"));
    assert_eq!(item.rel_path, "src/Foo.kt");
    assert_eq!(item.line_number, 7);
}

#[test]
fn the_deepest_root_holding_the_file_wins() {
    let roots = vec![PathBuf::from("/repo"), PathBuf::from("/repo/app")];
    let item = TextMatchSearchItem::new(&roots, matched("/repo/app/Foo.kt", "val x = 1", 4), 1.)
        .expect("the file is under both roots");

    assert_eq!(item.root, PathBuf::from("/repo/app"));
    assert_eq!(item.rel_path, "Foo.kt");
}

#[test]
fn a_file_outside_every_root_is_skipped() {
    let roots = vec![PathBuf::from("/repo")];
    assert!(TextMatchSearchItem::new(&roots, matched("/elsewhere/Foo.kt", "val x = 1", 4), 1.).is_none());
}

#[test]
fn the_column_is_the_first_submatches_char_column() {
    let roots = vec![PathBuf::from("/repo")];
    let item = TextMatchSearchItem::new(&roots, matched("/repo/Foo.kt", "    val x = 1", 8), 1.)
        .expect("the file is under the root");

    assert_eq!(item.first_match_column(), Some(9));
}

#[test]
fn the_column_counts_chars_not_bytes() {
    let roots = vec![PathBuf::from("/repo")];
    // "ä" is two bytes, so the submatch at byte 3 starts at character 2.
    let item = TextMatchSearchItem::new(&roots, matched("/repo/Foo.kt", "äbx", 2), 1.)
        .expect("the file is under the root");

    assert_eq!(item.first_match_column(), Some(2));
}
