//! Channel-dependent branding for the onboarding slides. The OSS channel is branded as the
//! Warpium fork; every other channel keeps Warp's branding.

use warp_core::channel::Channel;
use warp_core::ui::Icon;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::ColorScheme;
use warp_core::ui::theme::color::internal_colors;
use warpui_core::Element;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::{CacheOption, Image};

/// The app's logo mark. Warpium shows its full-colour mark, which reads on any theme; Warp shows
/// its mark as a faint foreground tint.
pub(crate) fn logo_mark(channel: Channel, appearance: &Appearance) -> Box<dyn Element> {
    match channel {
        Channel::Oss => {
            let path = warpium_mark_path(appearance.theme().inferred_color_scheme());
            Image::new(AssetSource::Bundled { path }, CacheOption::BySize).finish()
        }
        Channel::Stable
        | Channel::Preview
        | Channel::Dev
        | Channel::Local
        | Channel::Integration => Icon::WarpLogoLight
            .to_warpui_icon(internal_colors::fg_overlay_4(appearance.theme()))
            .finish(),
    }
}

fn warpium_mark_path(color_scheme: ColorScheme) -> &'static str {
    match color_scheme {
        ColorScheme::LightOnDark => "bundled/svg/warpium-mark-on-dark.svg",
        ColorScheme::DarkOnLight => "bundled/svg/warpium-mark.svg",
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

/// The label of the button that finishes setup and opens the app.
pub fn get_started_label(channel: Channel) -> &'static str {
    match channel {
        Channel::Oss => "Get started",
        Channel::Stable
        | Channel::Preview
        | Channel::Dev
        | Channel::Local
        | Channel::Integration => "Get Warping",
    }
}

pub(crate) fn input_callout_title(channel: Channel) -> &'static str {
    match channel {
        Channel::Oss => "Meet the Warpium input",
        Channel::Stable
        | Channel::Preview
        | Channel::Dev
        | Channel::Local
        | Channel::Integration => "Meet the Warp input",
    }
}

#[cfg(test)]
#[path = "branding_tests.rs"]
mod tests;
