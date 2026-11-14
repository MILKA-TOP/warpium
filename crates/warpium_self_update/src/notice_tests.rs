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
fn offer_description_names_both_versions_by_short_name() {
    let description = offer_description(
        &version(CURRENT),
        &release("v0.2026.06.03.09.49.stable_00.3"),
    );

    assert_eq!(
        description,
        "Version 2026.06.03.0949.3 (you have 2026.06.03.0949.2). Update now downloads it in the \
         background; you choose when to restart."
    );
}

#[test]
fn offer_description_reads_newer_after_an_upstream_sync_resets_the_fork_counter() {
    let description = offer_description(
        &version(CURRENT),
        &release("v0.2026.06.03.17.10.stable_01.1"),
    );

    assert_eq!(
        description,
        "Version 2026.06.03.1710.1 (you have 2026.06.03.0949.2). Update now downloads it in the \
         background; you choose when to restart."
    );
}

#[test]
fn offer_description_uses_full_tags_when_short_names_collide() {
    let description = offer_description(
        &version(CURRENT),
        &release("v0.2026.06.03.09.49.stable_01.2"),
    );

    assert_eq!(
        description,
        "Version v0.2026.06.03.09.49.stable_01.2 (you have v0.2026.06.03.09.49.stable_00.2). Update \
         now downloads it in the background; you choose when to restart."
    );
}

#[test]
fn downloading_heading_shows_progress_when_known() {
    assert_eq!(downloading_heading(Some(42)), "Downloading update… 42%");
    assert_eq!(downloading_heading(None), "Downloading update…");
}

#[test]
fn ready_description_names_the_release() {
    assert_eq!(
        ready_description(
            Some(&version(CURRENT)),
            &release("v0.2026.06.03.09.49.stable_00.3")
        ),
        "Restart now to switch to Warpium 2026.06.03.0949.3, or it's installed when you quit."
    );
}

#[test]
fn release_name_uses_the_full_tag_when_it_collides_with_the_running_version() {
    assert_eq!(
        release_name(
            Some(&version(CURRENT)),
            &release("v0.2026.06.03.09.49.stable_01.2")
        ),
        "v0.2026.06.03.09.49.stable_01.2"
    );
}

#[test]
fn release_name_falls_back_to_a_malformed_tag() {
    assert_eq!(release_name(None, &release("v9")), "v9");
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
