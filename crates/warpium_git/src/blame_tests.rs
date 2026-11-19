use super::*;

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ZERO: &str = "0000000000000000000000000000000000000000";

fn porcelain() -> String {
    format!(
        "{A} 1 1 2\n\
         author Ada Lovelace\n\
         author-mail <ada@example.com>\n\
         author-time 1700000000\n\
         author-tz +0000\n\
         committer Ada Lovelace\n\
         committer-mail <ada@example.com>\n\
         committer-time 1700000000\n\
         committer-tz +0000\n\
         summary Initial commit\n\
         boundary\n\
         filename src/Main.kt\n\
         \tfun main() {{\n\
         {A} 2 2\n\
         \t    println(1)\n\
         {B} 2 3 1\n\
         author Grace Hopper\n\
         author-mail <grace@example.com>\n\
         author-time 1700001000\n\
         author-tz +0000\n\
         summary Print more\n\
         previous {A} src/Main.kt\n\
         filename src/Main.kt\n\
         \t    println(2)\n\
         {ZERO} 4 4 1\n\
         author External file (--contents)\n\
         author-mail <external.file>\n\
         author-time 1700002000\n\
         author-tz +0000\n\
         summary Version of src/Main.kt from src/Main.kt\n\
         filename src/Main.kt\n\
         \t    // edited\n\
         {A} 3 5 1\n\
         filename src/Old.kt\n\
         \t}}\n"
    )
}

#[test]
fn parses_porcelain() {
    let blame = Blame::parse_porcelain(&porcelain()).unwrap();
    assert_eq!(blame.line_count(), 5);
    assert_eq!(blame.commits().len(), 3);

    let first = blame.line(0).unwrap();
    assert_eq!(first.author, "Ada Lovelace");
    assert_eq!(first.author_mail, "ada@example.com");
    assert_eq!(first.author_time, 1_700_000_000);
    assert_eq!(first.summary, "Initial commit");
    assert!(!first.is_uncommitted());

    assert_eq!(blame.line(1).unwrap().oid, first.oid);
    assert_eq!(blame.line(2).unwrap().author, "Grace Hopper");
    assert!(blame.line(3).unwrap().is_uncommitted());
    // A repeated commit keeps the details from its first appearance.
    assert_eq!(blame.line(4).unwrap().summary, "Initial commit");
    assert_eq!(blame.commit_index(4), blame.commit_index(0));
    assert_ne!(blame.commit_index(2), blame.commit_index(0));
    assert!(blame.line(5).is_none());
    assert_eq!(blame.max_author_chars(), "External file (--contents)".len());
}

#[test]
fn empty_output_is_an_empty_blame() {
    let blame = Blame::parse_porcelain("").unwrap();
    assert_eq!(blame.line_count(), 0);
}

#[test]
fn rejects_garbage() {
    assert!(Blame::parse_porcelain("not a header\n").is_err());
    assert!(Blame::parse_porcelain("\tcontent without header\n").is_err());
}

#[test]
fn args() {
    assert_eq!(
        blame_args("src/a.kt", true),
        [
            "blame",
            "--porcelain",
            "--encoding=UTF-8",
            "--contents",
            "-",
            "--",
            "src/a.kt"
        ]
    );
    assert!(!blame_args("a", false).contains(&"--contents".to_owned()));
}
