use super::*;

const CURRENT: &str = "v0.2026.06.03.09.49.stable_00.2";
const NOW: u64 = 1_000_000;

fn version(tag: &str) -> WarpiumVersion {
    WarpiumVersion::parse(tag).unwrap()
}

fn release(tag: &str) -> Release {
    Release {
        tag: tag.to_owned(),
        dmg_size: 1,
    }
}

#[test]
fn offers_a_newer_release() {
    let latest = release("v0.2026.06.03.09.49.stable_00.10");

    let offered = update_to_offer(Some(&version(CURRENT)), Some(&latest), None);

    assert_eq!(offered, Some(&latest));
}

#[test]
fn does_not_offer_the_running_version() {
    let latest = release(CURRENT);

    assert_eq!(
        update_to_offer(Some(&version(CURRENT)), Some(&latest), None),
        None
    );
}

#[test]
fn does_not_offer_an_older_release() {
    let latest = release("v0.2026.06.03.09.49.stable_00.1");

    assert_eq!(
        update_to_offer(Some(&version(CURRENT)), Some(&latest), None),
        None
    );
}

#[test]
fn does_not_offer_a_skipped_release() {
    let latest = release("v0.2026.06.03.09.49.stable_00.3");

    let offered = update_to_offer(
        Some(&version(CURRENT)),
        Some(&latest),
        Some("v0.2026.06.03.09.49.stable_00.3"),
    );

    assert_eq!(offered, None);
}

#[test]
fn offers_a_release_newer_than_the_skipped_one() {
    let latest = release("v0.2026.06.03.09.49.stable_00.4");

    let offered = update_to_offer(
        Some(&version(CURRENT)),
        Some(&latest),
        Some("v0.2026.06.03.09.49.stable_00.3"),
    );

    assert_eq!(offered, Some(&latest));
}

#[test]
fn development_builds_are_never_offered_updates() {
    let latest = release("v0.2026.06.03.09.49.stable_00.4");

    assert_eq!(update_to_offer(None, Some(&latest), None), None);
}

#[test]
fn nothing_is_offered_without_a_latest_release() {
    assert_eq!(update_to_offer(Some(&version(CURRENT)), None, None), None);
}

#[test]
fn a_saved_release_with_a_malformed_tag_is_not_offered() {
    let latest = release("v9");

    assert_eq!(
        update_to_offer(Some(&version(CURRENT)), Some(&latest), None),
        None
    );
}

#[test]
fn banner_heading_names_both_versions() {
    let heading = banner_heading(
        &version(CURRENT),
        &release("v0.2026.06.03.09.49.stable_00.3"),
    );

    assert_eq!(
        heading,
        "Warpium v0.2026.06.03.09.49.stable_00.3 is available (you have v0.2026.06.03.09.49.stable_00.2)."
    );
}

#[test]
fn manual_report_offers_a_newer_release() {
    let outcome = CheckOutcome::Checked(Some(release("v0.2026.06.03.09.49.stable_00.3")));

    assert_eq!(
        manual_report(&version(CURRENT), &outcome, NOW),
        "Warpium v0.2026.06.03.09.49.stable_00.3 is available (you have v0.2026.06.03.09.49.stable_00.2)."
    );
}

#[test]
fn manual_report_says_up_to_date_for_the_same_version() {
    let outcome = CheckOutcome::Checked(Some(release(CURRENT)));

    assert_eq!(
        manual_report(&version(CURRENT), &outcome, NOW),
        "Warpium is up to date (v0.2026.06.03.09.49.stable_00.2)."
    );
}

#[test]
fn manual_report_says_up_to_date_without_a_release() {
    assert_eq!(
        manual_report(&version(CURRENT), &CheckOutcome::Checked(None), NOW),
        "Warpium is up to date (v0.2026.06.03.09.49.stable_00.2)."
    );
}

#[test]
fn manual_report_explains_a_rate_limit() {
    let outcome = CheckOutcome::Deferred {
        until: NOW + 2 * 60 * 60,
    };

    assert_eq!(
        manual_report(&version(CURRENT), &outcome, NOW),
        "GitHub's rate limit was reached; try again in 2 hours."
    );
}

#[test]
fn manual_report_explains_a_failure() {
    let outcome = CheckOutcome::Failed(CheckError::Http { status: 502 });

    assert_eq!(
        manual_report(&version(CURRENT), &outcome, NOW),
        "Couldn't check for Warpium updates: GitHub answered HTTP 502."
    );
}

#[test]
fn progress_is_a_whole_percentage() {
    assert_eq!(progress_percent(0, 200), Some(0));
    assert_eq!(progress_percent(99, 200), Some(49));
    assert_eq!(progress_percent(200, 200), Some(100));
}

#[test]
fn progress_is_capped_and_unknown_without_a_total() {
    assert_eq!(progress_percent(300, 200), Some(100));
    assert_eq!(progress_percent(10, 0), None);
}
