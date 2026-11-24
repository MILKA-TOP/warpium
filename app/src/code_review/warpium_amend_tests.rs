use super::*;

#[test]
fn replaces_the_commit_prefix_only() {
    assert_eq!(amend_intent_label("Commit", true), "Amend");
    assert_eq!(amend_intent_label("Commit and push", true), "Amend and push");
    assert_eq!(
        amend_intent_label("Commit and create PR", true),
        "Amend and create PR"
    );
    assert_eq!(amend_intent_label("Commit and publish", true), "Amend and publish");
}

#[test]
fn keeps_the_base_label_when_not_amending() {
    assert_eq!(amend_intent_label("Commit and push", false), "Commit and push");
}

#[test]
fn warns_once_only_when_head_is_pushed() {
    assert!(needs_pushed_amend_warning(true, true, false));
    assert!(!needs_pushed_amend_warning(true, true, true));
    assert!(!needs_pushed_amend_warning(true, false, false));
    assert!(!needs_pushed_amend_warning(false, true, false));
}
