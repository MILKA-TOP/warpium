use super::*;

#[test]
fn parses_details() {
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let output = format!(
        "{a}\x1f{b}\x1fAda\x1fada@example.com\x1f1700000000\x1fGrace\x1fgrace@example.com\x1f1700000500\x1fHEAD -> refs/heads/main\x1fSubject line\n\nBody with \x1f a separator.\n\n\n"
    );
    let details = parse_details(&output).unwrap();
    assert_eq!(details.oid.to_hex(), a);
    assert_eq!(details.parents.len(), 1);
    assert_eq!(details.author_name, "Ada");
    assert_eq!(details.committer_name, "Grace");
    assert_eq!(details.committer_time, 1_700_000_500);
    assert_eq!(details.refs.len(), 1);
    assert_eq!(
        details.message,
        "Subject line\n\nBody with \x1f a separator."
    );
}

#[test]
fn rejects_truncated_output() {
    assert!(parse_details("").is_none());
    assert!(parse_details(&format!("{}\x1f\x1fAda", "a".repeat(40))).is_none());
}

#[test]
fn args_name_the_commit() {
    let oid = Oid::from_hex(&"c".repeat(40)).unwrap();
    let args = details_args(&oid);
    assert_eq!(args[0], "show");
    assert_eq!(args.last(), Some(&oid.to_hex()));
}
