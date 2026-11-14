use serde_json::json;

use super::*;

/// `GET /repos/MILKA-TOP/warpium/releases/latest`, recorded from the API.
const LATEST_FIXTURE: &str = include_str!("../fixtures/releases_latest.json");
const TAG: &str = "v0.2026.06.03.09.49.stable_00.1";

fn release_json(tag: &str, assets: &[&str]) -> Vec<u8> {
    let assets: Vec<_> = assets
        .iter()
        .map(|name| json!({ "name": name, "size": 10, "state": "uploaded" }))
        .collect();
    json!({ "tag_name": tag, "draft": false, "prerelease": false, "assets": assets })
        .to_string()
        .into_bytes()
}

#[test]
fn parses_the_recorded_latest_release() {
    let release = parse_latest_release(LATEST_FIXTURE.as_bytes()).unwrap();

    assert_eq!(
        release,
        Release {
            tag: TAG.to_owned(),
            dmg: ReleaseAsset {
                name: "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg".to_owned(),
                url: "https://github.com/MILKA-TOP/warpium/releases/download/v0.2026.06.03.09.49.stable_00.1/Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg".to_owned(),
                size: 162_767_155,
            },
            checksum: ReleaseAsset {
                name: "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg.sha256".to_owned(),
                url: "https://github.com/MILKA-TOP/warpium/releases/download/v0.2026.06.03.09.49.stable_00.1/Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg.sha256".to_owned(),
                size: 116,
            },
        }
    );
}

#[test]
fn links_to_the_release_page() {
    let release = parse_latest_release(LATEST_FIXTURE.as_bytes()).unwrap();

    assert_eq!(
        release.page_url(),
        "https://github.com/MILKA-TOP/warpium/releases/tag/v0.2026.06.03.09.49.stable_00.1"
    );
}

#[test]
fn ignores_a_draft() {
    let body = json!({ "tag_name": TAG, "draft": true, "prerelease": false, "assets": [] });

    let err = parse_latest_release(body.to_string().as_bytes()).unwrap_err();

    assert_eq!(err, ReleaseError::Draft);
    assert!(err.is_ignored_release());
}

#[test]
fn ignores_a_prerelease() {
    let body = json!({ "tag_name": TAG, "draft": false, "prerelease": true, "assets": [] });

    let err = parse_latest_release(body.to_string().as_bytes()).unwrap_err();

    assert_eq!(err, ReleaseError::Prerelease);
    assert!(err.is_ignored_release());
}

#[test]
fn ignores_a_tag_that_is_not_a_warpium_version() {
    let body = release_json("v0.2026.06.03.09.49.stable_00", &[]);

    let err = parse_latest_release(&body).unwrap_err();

    assert_eq!(
        err,
        ReleaseError::MalformedTag("v0.2026.06.03.09.49.stable_00".to_owned())
    );
    assert!(err.is_ignored_release());
}

#[test]
fn requires_the_arm64_dmg() {
    let body = release_json(
        TAG,
        &["Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg.sha256"],
    );

    let err = parse_latest_release(&body).unwrap_err();

    assert_eq!(
        err,
        ReleaseError::MissingAsset {
            tag: TAG.to_owned(),
            name: "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg".to_owned(),
        }
    );
    assert!(!err.is_ignored_release());
}

#[test]
fn requires_the_checksum() {
    let body = release_json(TAG, &["Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg"]);

    let err = parse_latest_release(&body).unwrap_err();

    assert_eq!(
        err,
        ReleaseError::MissingAsset {
            tag: TAG.to_owned(),
            name: "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg.sha256".to_owned(),
        }
    );
}

#[test]
fn picks_the_assets_of_the_release_tag_among_others() {
    let body = release_json(
        TAG,
        &[
            "Warpium-v0.2026.06.03.09.49.stable_00.0-arm64.dmg",
            "Warpium-v0.2026.06.03.09.49.stable_00.1-x86_64.dmg",
            "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg.sha256",
            "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg",
        ],
    );

    let release = parse_latest_release(&body).unwrap();

    assert_eq!(
        release.dmg.name,
        "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg"
    );
}

#[test]
fn skips_an_asset_that_is_still_uploading() {
    let body = json!({
        "tag_name": TAG,
        "assets": [
            { "name": "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg", "state": "starter" },
            { "name": "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg.sha256", "state": "uploaded" },
        ],
    });

    let err = parse_latest_release(body.to_string().as_bytes()).unwrap_err();

    assert!(matches!(err, ReleaseError::MissingAsset { .. }), "{err:?}");
}

#[test]
fn download_urls_ignore_the_urls_in_the_response() {
    let body = json!({
        "tag_name": TAG,
        "assets": [
            {
                "name": "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg",
                "browser_download_url": "https://example.com/evil.dmg",
            },
            { "name": "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg.sha256" },
        ],
    });

    let release = parse_latest_release(body.to_string().as_bytes()).unwrap();

    assert_eq!(
        release.dmg.url,
        "https://github.com/MILKA-TOP/warpium/releases/download/v0.2026.06.03.09.49.stable_00.1/Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg"
    );
}

#[test]
fn rejects_a_body_that_is_not_a_release() {
    let err = parse_latest_release(b"{\"message\":\"Not Found\"}").unwrap_err();

    assert!(matches!(err, ReleaseError::Parse(_)), "{err:?}");
}
