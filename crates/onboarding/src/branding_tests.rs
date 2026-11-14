use warp_core::channel::Channel;
use warp_core::ui::theme::ColorScheme;

use super::{
    customize_title, get_started_label, input_callout_title, warpium_mark_path, welcome_subtitle,
    welcome_title,
};

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

#[test]
fn oss_channel_buttons_and_callouts_name_warpium() {
    assert_eq!(get_started_label(Channel::Oss), "Get started");
    assert_eq!(input_callout_title(Channel::Oss), "Meet the Warpium input");
}

#[test]
fn stable_channel_buttons_and_callouts_keep_warp_branding() {
    assert_eq!(get_started_label(Channel::Stable), "Get Warping");
    assert_eq!(input_callout_title(Channel::Stable), "Meet the Warp input");
}

#[test]
fn warpium_mark_uses_the_lifted_variant_on_dark_themes() {
    assert_eq!(
        warpium_mark_path(ColorScheme::LightOnDark),
        "bundled/svg/warpium-mark-on-dark.svg"
    );
}

#[test]
fn warpium_mark_uses_the_standard_variant_on_light_themes() {
    assert_eq!(
        warpium_mark_path(ColorScheme::DarkOnLight),
        "bundled/svg/warpium-mark.svg"
    );
}

/// `Icon` tints by the red channel of the rasterized SVG, so a black monochrome mark (used by the
/// get-started page) renders as nothing.
#[test]
fn neutral_warpium_mark_is_a_white_icon_mask() {
    let svg = include_str!("../../../app/assets/bundled/svg/warpium-mark-neutral.svg");

    assert!(
        !svg.contains("#000000"),
        "neutral mark must not be black: {svg}"
    );
    assert!(
        svg.contains(r##"fill="#FFFFFF""##),
        "neutral mark must be white: {svg}"
    );
}
