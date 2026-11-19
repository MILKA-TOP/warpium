use super::*;

#[test]
fn running_wins_over_the_last_exit_code() {
    assert_eq!(
        SubTabStatus::of_terminal(true, Some(1)),
        SubTabStatus::Running
    );
}

#[test]
fn zero_exit_code_is_success() {
    assert_eq!(
        SubTabStatus::of_terminal(false, Some(0)),
        SubTabStatus::Succeeded
    );
}

#[test]
fn non_zero_exit_code_is_failure_with_the_code() {
    assert_eq!(
        SubTabStatus::of_terminal(false, Some(130)),
        SubTabStatus::Failed(130)
    );
}

#[test]
fn nothing_run_yet_is_idle() {
    assert_eq!(SubTabStatus::of_terminal(false, None), SubTabStatus::Idle);
}

#[test]
fn labels_show_a_check_or_a_cross_with_the_code() {
    assert_eq!(SubTabStatus::Succeeded.label().as_deref(), Some("✓"));
    assert_eq!(SubTabStatus::Failed(2).label().as_deref(), Some("✗ 2"));
    assert_eq!(SubTabStatus::Running.label(), None);
    assert_eq!(SubTabStatus::Idle.label(), None);
}
