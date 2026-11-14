use super::*;

fn v(tag: &str) -> WarpiumVersion {
    WarpiumVersion::parse(tag).unwrap_or_else(|| panic!("{tag} should parse"))
}

#[test]
fn parses_a_release_tag() {
    let version = v("v0.2026.06.03.09.49.stable_00.1");

    assert_eq!(version.tag(), "v0.2026.06.03.09.49.stable_00.1");
    assert_eq!(version.to_string(), "v0.2026.06.03.09.49.stable_00.1");
}

#[test]
fn derives_the_plist_version() {
    assert_eq!(
        v("v0.2026.06.03.09.49.stable_00.1").plist_version(),
        "0.2026.06.03.09.49.00.1"
    );
    assert_eq!(
        v("v0.2026.11.20.08.05.preview_03.12").plist_version(),
        "0.2026.11.20.08.05.03.12"
    );
}

#[test]
fn short_name_is_the_upstream_date_time_and_fork_counter() {
    assert_eq!(
        v("v0.2026.06.03.09.49.stable_00.1").short_name(),
        "2026.06.03.0949.1"
    );
    assert_eq!(
        v("v0.2026.11.20.08.05.preview_03.12").short_name(),
        "2026.11.20.0805.12"
    );
}

#[test]
fn rejects_malformed_tags() {
    for tag in [
        "",
        "v0.2026.06.03.09.49.stable_00",
        "0.2026.06.03.09.49.stable_00.1",
        "v0.2026.06.03.09.49.stable_00.0",
        "v0.2026.06.03.09.49.stable_00.01",
        "v0.2026.06.03.09.49.stable_00.1.2",
        "v0.2026.6.03.09.49.stable_00.1",
        "v0.26.06.03.09.49.stable_00.1",
        "v0.2026.06.03.09.49.stable00.1",
        "v0.2026.06.03.09.49.Stable_00.1",
        "v0.2026.06.03.09.49.stable_0.1",
        "v0.2026.06.03.09.49.stable_00.x",
        "v0.2026.06.03.09.49.stable_00.-1",
        "vx.2026.06.03.09.49.stable_00.1",
        "latest",
    ] {
        assert!(
            WarpiumVersion::parse(tag).is_none(),
            "{tag:?} should be rejected"
        );
    }
}

#[test]
fn compares_the_fork_counter_numerically() {
    assert!(v("v0.2026.06.03.09.49.stable_00.10") > v("v0.2026.06.03.09.49.stable_00.2"));
    assert!(v("v0.2026.06.03.09.49.stable_00.2") > v("v0.2026.06.03.09.49.stable_00.1"));
}

#[test]
fn a_newer_upstream_release_wins_over_a_higher_fork_counter() {
    assert!(v("v0.2026.06.10.08.00.stable_00.1") > v("v0.2026.06.03.09.49.stable_00.12"));
}

#[test]
fn an_older_upstream_release_is_older_whatever_its_fork_counter() {
    assert!(v("v0.2026.05.28.09.49.stable_00.30") < v("v0.2026.06.03.09.49.stable_00.1"));
}

#[test]
fn compares_upstream_date_components_numerically() {
    assert!(v("v0.2027.01.01.00.00.stable_00.1") > v("v0.2026.12.31.23.59.stable_00.9"));
    assert!(v("v0.2026.06.03.10.00.stable_00.1") > v("v0.2026.06.03.09.59.stable_00.1"));
    assert!(v("v1.2026.01.01.00.00.stable_00.1") > v("v0.2026.12.31.23.59.stable_00.1"));
}

#[test]
fn compares_the_upstream_channel_build() {
    assert!(v("v0.2026.06.03.09.49.stable_01.1") > v("v0.2026.06.03.09.49.stable_00.4"));
}

#[test]
fn equal_tags_are_equal() {
    assert_eq!(
        v("v0.2026.06.03.09.49.stable_00.1"),
        v(" v0.2026.06.03.09.49.stable_00.1\n")
    );
}

#[test]
fn the_runtime_override_wins_over_the_embedded_tag() {
    let current = resolve_current_version(
        Some("v0.2026.05.01.00.00.stable_00.1"),
        Some("v0.2026.06.03.09.49.stable_00.1"),
    );

    assert_eq!(
        current.map(|version| version.tag().to_owned()),
        Some("v0.2026.05.01.00.00.stable_00.1".to_owned())
    );
}

#[test]
fn a_malformed_override_falls_back_to_the_embedded_tag() {
    let current = resolve_current_version(Some("old"), Some("v0.2026.06.03.09.49.stable_00.1"));

    assert_eq!(
        current.map(|version| version.tag().to_owned()),
        Some("v0.2026.06.03.09.49.stable_00.1".to_owned())
    );
}

#[test]
fn builds_without_a_release_tag_have_no_version() {
    assert_eq!(resolve_current_version(None, None), None);
    assert_eq!(resolve_current_version(None, Some("")), None);
}
