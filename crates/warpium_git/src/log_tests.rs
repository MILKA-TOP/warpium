use super::*;
use crate::refs::RefKind;

fn hex(byte: char) -> String {
    byte.to_string().repeat(40)
}

fn record(oid: char, parents: &[char], refs: &str, subject: &str) -> String {
    let parents: Vec<String> = parents.iter().map(|parent| hex(*parent)).collect();
    format!(
        "{}\x1f{}\x1fAda Lovelace\x1fada@example.com\x1f1700000000\x1f{refs}\x1f{subject}\x1e\n",
        hex(oid),
        parents.join(" ")
    )
}

#[test]
fn parses_records() {
    let output = record('a', &['b', 'c'], "HEAD -> refs/heads/main", "Merge branch 'x'")
        + &record('b', &[], "", "Initial");
    let mut parser = LogParser::default();
    let mut commits = Vec::new();
    parser.feed(output.as_bytes(), &mut commits);
    parser.finish(&mut commits);

    assert_eq!(commits.len(), 2);
    let merge = &commits[0];
    assert_eq!(merge.oid.to_hex(), hex('a'));
    assert_eq!(merge.parents.len(), 2);
    assert_eq!(merge.author_name, "Ada Lovelace");
    assert_eq!(merge.author_email, "ada@example.com");
    assert_eq!(merge.author_time, 1_700_000_000);
    assert_eq!(merge.subject, "Merge branch 'x'");
    assert_eq!(merge.refs.len(), 1);
    assert_eq!(merge.refs[0].kind, RefKind::LocalBranch);
    assert!(merge.refs[0].is_head);
    assert!(commits[1].parents.is_empty());
    assert!(commits[1].refs.is_empty());
}

#[test]
fn records_split_across_chunks() {
    let output = record('a', &['b'], "", "First") + &record('b', &[], "", "Second");
    let bytes = output.as_bytes();
    for chunk_size in [1, 3, 7, 40, 41, 100] {
        let mut parser = LogParser::default();
        let mut commits = Vec::new();
        for chunk in bytes.chunks(chunk_size) {
            parser.feed(chunk, &mut commits);
        }
        parser.finish(&mut commits);
        let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["First", "Second"], "chunk size {chunk_size}");
    }
}

#[test]
fn subject_keeps_separators_after_the_last_field() {
    let output = format!(
        "{}\x1f\x1fA\x1fa@b\x1f1\x1f\x1fsubject with \x1f inside\x1e",
        hex('a')
    );
    let mut parser = LogParser::default();
    let mut commits = Vec::new();
    parser.feed(output.as_bytes(), &mut commits);
    assert_eq!(commits[0].subject, "subject with \x1f inside");
}

#[test]
fn malformed_records_are_skipped() {
    let output = format!("nothex\x1f\x1fA\x1fa\x1f1\x1f\x1fs\x1e{}", record('a', &[], "", "ok"));
    let mut parser = LogParser::default();
    let mut commits = Vec::new();
    parser.feed(output.as_bytes(), &mut commits);
    parser.finish(&mut commits);
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].subject, "ok");
}

#[test]
fn query_args() {
    let query = LogQuery::default();
    let args = query.args();
    assert_eq!(args[0], "log");
    assert!(args.contains(&"--date-order".to_owned()));
    assert_eq!(&args[args.len() - 2..], ["HEAD", "--"]);
    assert!(query.shows_graph());

    let query = LogQuery {
        revisions: Revisions::Rev("feature".to_owned()),
        author: Some("ada".to_owned()),
        text: Some("fix".to_owned()),
        paths: vec!["src/main.kt".to_owned()],
    };
    let args = query.args();
    assert!(args.contains(&"--author=ada".to_owned()));
    assert!(args.contains(&"--grep=fix".to_owned()));
    assert!(args.contains(&"--fixed-strings".to_owned()));
    assert!(args.contains(&"--parents".to_owned()));
    assert_eq!(&args[args.len() - 3..], ["feature", "--", "src/main.kt"]);
    assert!(!query.shows_graph());

    let all = LogQuery {
        revisions: Revisions::All,
        ..Default::default()
    };
    assert!(all.args().contains(&"--all".to_owned()));
}
