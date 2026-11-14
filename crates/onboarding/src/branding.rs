//! Channel-dependent branding for the onboarding slides. The OSS channel is branded as the
//! Warpium fork; every other channel keeps Warp's branding.

use warp_core::channel::Channel;
use warp_core::ui::Icon;
use warp_core::ui::theme::Fill;
use warpui_core::elements::Icon as WarpUiIcon;

const WARPIUM_MARK_PATH: &str = "bundled/svg/warpium-mark-neutral.svg";

/// The app's logo mark, tinted with `fill`.
pub(crate) fn logo_mark(channel: Channel, fill: Fill) -> WarpUiIcon {
    match channel {
        Channel::Oss => WarpUiIcon::new(WARPIUM_MARK_PATH, fill.into_solid()),
        Channel::Stable
        | Channel::Preview
        | Channel::Dev
        | Channel::Local
        | Channel::Integration => Icon::WarpLogoLight.to_warpui_icon(fill),
    }
}

pub(crate) fn welcome_title(channel: Channel) -> String {
    format!("Welcome to {}", channel.app_display_name())
}

pub(crate) fn welcome_subtitle(channel: Channel) -> &'static str {
    match channel {
        Channel::Oss => "A lightweight IDE for AI-driven mobile development, built on Warp.",
        Channel::Stable
        | Channel::Preview
        | Channel::Dev
        | Channel::Local
        | Channel::Integration => "A modern terminal with state of the art agents built in.",
    }
}

pub(crate) fn customize_title(channel: Channel) -> String {
    format!("Customize your {}", channel.app_display_name())
}

#[cfg(test)]
#[path = "branding_tests.rs"]
mod tests;
