//! Warpium must never offer or install official Warp builds: Warp's own updater has to stay off
//! for the OSS channel that Warpium builds. These read the sources that keep it off, so a change
//! (for instance from an upstream merge) fails here rather than shipping.

const OSS_BINARY: &str = include_str!("../../../app/src/bin/oss.rs");
const WARP_AUTOUPDATE: &str = include_str!("../../../app/src/autoupdate/mod.rs");

#[test]
fn the_oss_channel_has_no_warp_autoupdate_config() {
    assert!(
        OSS_BINARY.contains("Channel::Oss"),
        "app/src/bin/oss.rs should build the OSS channel"
    );
    assert!(
        OSS_BINARY.contains("autoupdate_config: None,"),
        "app/src/bin/oss.rs must keep `autoupdate_config: None` so Warpium never uses Warp's \
         update servers"
    );
}

#[test]
fn warps_updater_refuses_to_fetch_versions_for_the_oss_channel() {
    let fetch_version = WARP_AUTOUPDATE
        .split("async fn fetch_version(")
        .nth(1)
        .expect("app/src/autoupdate/mod.rs should define fetch_version");
    let body = &fetch_version[..fetch_version
        .find("\n}\n")
        .expect("fetch_version should end")];

    assert!(
        body.contains("Channel::Oss") && body.contains("anyhow::bail!"),
        "fetch_version must bail for Channel::Oss so Warp builds are never fetched"
    );
}

#[test]
fn warps_release_assets_are_unreachable_for_the_oss_channel() {
    let assets_url = WARP_AUTOUPDATE
        .split("fn release_assets_directory_url(")
        .nth(1)
        .expect("app/src/autoupdate/mod.rs should define release_assets_directory_url");
    let body = &assets_url[..assets_url.find("\n}\n").expect("function should end")];

    assert!(
        body.contains("Channel::Oss") && body.contains("unreachable!"),
        "release_assets_directory_url must not produce a Warp download URL for Channel::Oss"
    );
}
