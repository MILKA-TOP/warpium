use super::*;

fn oid(byte: char) -> Oid {
    Oid::from_hex(&byte.to_string().repeat(40)).unwrap()
}

#[test]
fn parses_name_status() {
    let output = b"M\0src/a.kt\0A\0b.txt\0R087\0old/name.kt\0new/name.kt\0D\0gone\0C100\0x\0y\0T\0link\0";
    let changes = parse_name_status(output);
    assert_eq!(
        changes,
        vec![
            FileChange {
                path: "src/a.kt".to_owned(),
                kind: FileChangeKind::Modified
            },
            FileChange {
                path: "b.txt".to_owned(),
                kind: FileChangeKind::Added
            },
            FileChange {
                path: "new/name.kt".to_owned(),
                kind: FileChangeKind::Renamed {
                    from: "old/name.kt".to_owned()
                }
            },
            FileChange {
                path: "gone".to_owned(),
                kind: FileChangeKind::Deleted
            },
            FileChange {
                path: "y".to_owned(),
                kind: FileChangeKind::Copied {
                    from: "x".to_owned()
                }
            },
            FileChange {
                path: "link".to_owned(),
                kind: FileChangeKind::TypeChanged
            },
        ]
    );
    assert_eq!(changes[2].kind.letter(), 'R');
    assert_eq!(changes[2].kind.old_path(), Some("old/name.kt"));
    assert_eq!(changes[0].kind.old_path(), None);
}

#[test]
fn truncated_name_status_stops() {
    assert!(parse_name_status(b"M").is_empty());
    assert!(parse_name_status(b"").is_empty());
    assert_eq!(parse_name_status(b"R100\0only-old\0").len(), 0);
}

#[test]
fn args_against_first_parent_or_root() {
    let args = changed_files_args(&oid('a'), Some(&oid('b')));
    assert_eq!(args[args.len() - 2..], [oid('b').to_hex(), oid('a').to_hex()]);
    let args = changed_files_args(&oid('a'), None);
    assert!(args.contains(&"--root".to_owned()));

    let rename = FileChange {
        path: "new.kt".to_owned(),
        kind: FileChangeKind::Renamed {
            from: "old.kt".to_owned(),
        },
    };
    let args = file_diff_args(&oid('a'), Some(&oid('b')), &rename);
    assert_eq!(args[args.len() - 3..], ["--", "old.kt", "new.kt"]);
}

#[test]
fn parses_unified_diff_with_line_numbers() {
    let patch = "diff --git a/f b/f\n\
                 index 1..2 100644\n\
                 --- a/f\n\
                 +++ b/f\n\
                 @@ -10,4 +10,5 @@ fun main() {\n \
                 keep\n\
                 -old\n\
                 +new\n\
                 +added\n \
                 tail\n\
                 \\ No newline at end of file\n";
    let lines = parse_unified_diff(patch);
    let kinds: Vec<DiffLineKind> = lines.iter().map(|line| line.kind).collect();
    assert_eq!(
        kinds,
        [
            DiffLineKind::Hunk,
            DiffLineKind::Context,
            DiffLineKind::Removed,
            DiffLineKind::Added,
            DiffLineKind::Added,
            DiffLineKind::Context,
            DiffLineKind::Info,
        ]
    );
    assert_eq!((lines[1].old_line, lines[1].new_line), (Some(10), Some(10)));
    assert_eq!((lines[2].old_line, lines[2].new_line), (Some(11), None));
    assert_eq!((lines[3].old_line, lines[3].new_line), (None, Some(11)));
    assert_eq!((lines[4].old_line, lines[4].new_line), (None, Some(12)));
    assert_eq!((lines[5].old_line, lines[5].new_line), (Some(12), Some(13)));
    assert_eq!(lines[2].text, "old");
}

#[test]
fn removed_lines_that_look_like_headers_stay_in_the_hunk() {
    let patch = "@@ -1,2 +1,1 @@\n--- not a header\n+++ also content\n";
    let lines = parse_unified_diff(patch);
    assert_eq!(lines[1].kind, DiffLineKind::Removed);
    assert_eq!(lines[1].text, "-- not a header");
    assert_eq!(lines[2].kind, DiffLineKind::Added);
}

#[test]
fn binary_and_rename_headers_are_info() {
    let patch = "diff --git a/x b/y\nsimilarity index 90%\nrename from x\nrename to y\nBinary files a/x and b/y differ\n";
    let lines = parse_unified_diff(patch);
    let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(
        texts,
        ["rename from x", "rename to y", "Binary files a/x and b/y differ"]
    );
    assert!(lines.iter().all(|line| line.kind == DiffLineKind::Info));
}
