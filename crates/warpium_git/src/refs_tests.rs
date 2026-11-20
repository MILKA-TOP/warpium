use super::*;

fn label(name: &str, kind: RefKind, is_head: bool) -> RefLabel {
    RefLabel {
        name: name.to_owned(),
        kind,
        is_head,
    }
}

#[test]
fn parses_full_decorations_in_display_order() {
    let labels = parse_decorations(
        "tag: refs/tags/v1.0, refs/remotes/origin/main, HEAD -> refs/heads/main, \
         refs/remotes/origin/HEAD, refs/heads/feature/x",
    );
    assert_eq!(
        labels,
        vec![
            label("main", RefKind::LocalBranch, true),
            label("feature/x", RefKind::LocalBranch, false),
            label("origin/main", RefKind::RemoteBranch, false),
            label("v1.0", RefKind::Tag, false),
        ]
    );
}

#[test]
fn detached_head_and_short_names() {
    let labels = parse_decorations("HEAD, tag: v2, main, refs/stash");
    assert_eq!(
        labels,
        vec![
            label("HEAD", RefKind::Head, true),
            label("main", RefKind::LocalBranch, false),
            label("v2", RefKind::Tag, false),
            label("stash", RefKind::Other, false),
        ]
    );
}

#[test]
fn empty_decorations() {
    assert!(parse_decorations("").is_empty());
    assert!(parse_decorations("  ").is_empty());
}

#[test]
fn branch_list_groups_local_before_remote() {
    let labels = parse_branch_list(
        " refs/remotes/origin/main\n*refs/heads/main\n refs/remotes/origin/HEAD\n refs/heads/dev\n",
    );
    assert_eq!(
        labels,
        vec![
            label("main", RefKind::LocalBranch, true),
            label("dev", RefKind::LocalBranch, false),
            label("origin/main", RefKind::RemoteBranch, false),
        ]
    );
}
