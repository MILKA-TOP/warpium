use super::*;

#[test]
fn splits_sections_keyed_by_plus_path() {
    let output = "\
:100644 100644 4cb29ea 0000000 M\0plain.txt\0\
:100644 100644 422c2b7 0000000 M\0src/with space.txt\0\0\
diff --git a/plain.txt b/plain.txt
index 4cb29ea..6addb9b 100644
--- a/plain.txt
+++ b/plain.txt
@@ -1,3 +1,4 @@
 one
-two
+TWO
 three
+four
diff --git a/src/with space.txt b/src/with space.txt
index 422c2b7..55dce13 100644
--- a/src/with space.txt\t
+++ b/src/with space.txt\t
@@ -1,2 +1,2 @@
 a
-b
+B
";
    let sections = split_combined_diff(output);
    assert_eq!(sections.len(), 2);
    let plain = sections.get("plain.txt").expect("plain.txt section");
    assert!(plain.starts_with("diff --git a/plain.txt b/plain.txt\n"));
    assert!(plain.contains("@@ -1,3 +1,4 @@"));
    let spaced = sections
        .get("src/with space.txt")
        .expect("path with spaces is keyed without the trailing tab");
    assert!(spaced.contains("+++ b/src/with space.txt\t\n"));
}

#[test]
fn splits_rename_and_delete_and_binary_sections() {
    let output = "\
diff --git a/old name.txt b/new name.txt
similarity index 92%
rename from old name.txt
rename to new name.txt
index 1111111..2222222 100644
--- a/old name.txt
+++ b/new name.txt
@@ -1 +1 @@
-x
+y
diff --git a/pure rename.txt b/renamed only.txt
similarity index 100%
rename from pure rename.txt
rename to renamed only.txt
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 3333333..000000
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/blob.bin b/blob.bin
index 4444444..5555555 100644
Binary files a/blob.bin and b/blob.bin differ
";
    let sections = split_combined_diff(output);
    assert_eq!(sections.len(), 4);
    assert!(sections.contains_key("new name.txt"));
    assert!(sections.contains_key("renamed only.txt"));
    assert!(sections.contains_key("gone.txt"));
    assert!(sections.contains_key("blob.bin"));
}

#[test]
fn empty_output_yields_no_sections() {
    assert!(split_combined_diff("").is_empty());
}

#[test]
fn additions_hunks_build_single_all_additions_hunk() {
    let hunks = additions_hunks("one\ntwo\n");
    assert_eq!(hunks.len(), 1);
    let hunk = &hunks[0];
    assert_eq!(hunk.old_start_line, 0);
    assert_eq!(hunk.old_line_count, 0);
    assert_eq!(hunk.new_start_line, 1);
    assert_eq!(hunk.new_line_count, 2);
    assert_eq!(hunk.lines.len(), 2);
    assert_eq!(hunk.lines[0].line_type, DiffLineType::Add);
    assert_eq!(hunk.lines[0].new_line_number, Some(1));
    assert_eq!(hunk.lines[1].new_line_number, Some(2));
    assert_eq!(hunk.lines[1].text, "two");
    assert!(hunk.lines.iter().all(|line| line.old_line_number.is_none()));
}

#[test]
fn additions_hunks_empty_content_has_no_hunks() {
    assert!(additions_hunks("").is_empty());
}
