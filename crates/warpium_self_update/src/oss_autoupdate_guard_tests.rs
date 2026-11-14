//! Warpium must never offer or install official Warp builds: Warp's own updater has to stay off
//! for the OSS channel that Warpium builds, without even contacting Warp's update servers. These
//! read the sources that keep it off, so a change (for instance from an upstream merge) fails here
//! rather than shipping.

const OSS_BINARY: &str = include_str!("../../../app/src/bin/oss.rs");
const WARP_AUTOUPDATE: &str = include_str!("../../../app/src/autoupdate/mod.rs");
const WARP_CHANNEL_VERSIONS: &str = include_str!("../../../app/src/autoupdate/channel_versions.rs");
const WARP_CHANGELOG: &str = include_str!("../../../app/src/autoupdate/changelog.rs");

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

/// The body of the first `fn {name}(` in `source`, up to the next line that closes an item at
/// `indent`.
fn function_body(source: &str, name: &str, indent: &str) -> String {
    let source = source.replace("\r\n", "\n");
    let start = source
        .split(&format!("fn {name}("))
        .nth(1)
        .unwrap_or_else(|| panic!("{name} should be defined"));
    let end = format!("\n{indent}}}\n");
    start[..start
        .find(&end)
        .unwrap_or_else(|| panic!("{name} should end"))]
        .to_owned()
}

#[test]
fn warps_polling_loop_is_not_started_for_the_oss_channel() {
    let body = function_body(WARP_AUTOUPDATE, "start_polling", "    ");
    let oss_return = body
        .find("Channel::Oss")
        .and_then(|guard| body[guard..].find("return;").map(|offset| guard + offset))
        .expect("start_polling must return early for Channel::Oss");
    let loop_start = body
        .find("self.poll_for_update(ctx)")
        .expect("start_polling should start the polling loop");

    assert!(
        oss_return < loop_start,
        "start_polling must return for Channel::Oss before it starts the polling loop"
    );
}

#[test]
fn warps_channel_versions_are_not_fetched_for_the_oss_channel() {
    let body = function_body(WARP_CHANNEL_VERSIONS, "fetch_channel_versions", "");
    let oss_bail = body
        .find("Channel::Oss")
        .expect("fetch_channel_versions must bail for Channel::Oss");
    let request = body
        .find(".fetch_channel_versions(")
        .expect("fetch_channel_versions should call the Warp server");

    assert!(
        oss_bail < request && body[oss_bail..request].contains("anyhow::bail!"),
        "fetch_channel_versions must bail for Channel::Oss before contacting Warp's servers"
    );
}

#[test]
fn the_changelog_is_not_fetched_for_the_oss_channel() {
    let body = function_body(WARP_CHANGELOG, "get_current_changelog", "");
    let oss_return = body
        .find("Channel::Oss")
        .expect("get_current_changelog must return early for Channel::Oss");
    let fetch = body
        .find("fetch_channel_versions(")
        .expect("get_current_changelog should fetch channel versions");

    assert!(
        oss_return < fetch && body[oss_return..fetch].contains("return Ok(None);"),
        "get_current_changelog must return Ok(None) for Channel::Oss before fetching channel \
         versions"
    );
}
