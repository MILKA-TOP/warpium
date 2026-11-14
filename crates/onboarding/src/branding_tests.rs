use warp_core::channel::Channel;

use super::{customize_title, welcome_subtitle, welcome_title};

#[test]
fn oss_channel_is_branded_as_warpium() {
    assert_eq!(welcome_title(Channel::Oss), "Welcome to Warpium");
    assert_eq!(
        welcome_subtitle(Channel::Oss),
        "A lightweight IDE for AI-driven mobile development, built on Warp."
    );
    assert_eq!(customize_title(Channel::Oss), "Customize your Warpium");
}

#[test]
fn stable_channel_keeps_warp_branding() {
    assert_eq!(welcome_title(Channel::Stable), "Welcome to Warp");
    assert_eq!(
        welcome_subtitle(Channel::Stable),
        "A modern terminal with state of the art agents built in."
    );
    assert_eq!(customize_title(Channel::Stable), "Customize your Warp");
}
