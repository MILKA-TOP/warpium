//! Button icons: a name from the app's bundled icon set, or an SVG file in the project.

use std::path::{Component, Path, PathBuf};

/// Icon names a button can use, mapped to the bundled monochrome SVG each one draws. Several
/// names are aliases for the same image.
pub const BUNDLED_ICONS: &[(&str, &str)] = &[
    ("apple", "bundled/svg/apple.svg"),
    ("bell", "bundled/svg/bell.svg"),
    ("bolt", "bundled/svg/lightning-02.svg"),
    ("book", "bundled/svg/book-open.svg"),
    ("bookmark", "bundled/svg/bookmark.svg"),
    ("bug", "bundled/svg/bug.svg"),
    ("build", "bundled/svg/tool-02.svg"),
    ("calendar", "bundled/svg/calendar-date.svg"),
    ("chart", "bundled/svg/bar-chart-04.svg"),
    ("chat", "bundled/svg/message-chat-square.svg"),
    ("check", "bundled/svg/check.svg"),
    ("clean", "bundled/svg/trash-02.svg"),
    ("clock", "bundled/svg/clock.svg"),
    ("cloud", "bundled/svg/cloud-01.svg"),
    ("code", "bundled/svg/code-02.svg"),
    ("commit", "bundled/svg/git-commit.svg"),
    ("copy", "bundled/svg/copy.svg"),
    ("debug", "bundled/svg/bug.svg"),
    ("deploy", "bundled/svg/rocket.svg"),
    ("device", "bundled/svg/phone.svg"),
    ("docker", "bundled/svg/docker.svg"),
    ("docs", "bundled/svg/book-open.svg"),
    ("download", "bundled/svg/download-02.svg"),
    ("edit", "bundled/svg/edit-01.svg"),
    ("eye", "bundled/svg/eye.svg"),
    ("file", "bundled/svg/file.svg"),
    ("filter", "bundled/svg/filter-funnel.svg"),
    ("folder", "bundled/svg/folder.svg"),
    ("gear", "bundled/svg/gear.svg"),
    ("git", "bundled/svg/git-branch-02.svg"),
    ("github", "bundled/svg/github.svg"),
    ("globe", "bundled/svg/globe-01.svg"),
    ("history", "bundled/svg/history.svg"),
    ("image", "bundled/svg/image-01.svg"),
    ("install", "bundled/svg/download-02.svg"),
    ("key", "bundled/svg/key.svg"),
    ("laptop", "bundled/svg/laptop.svg"),
    ("layout", "bundled/svg/layout-alt-01.svg"),
    ("link", "bundled/svg/link-external-02.svg"),
    ("logs", "bundled/svg/terminal-input.svg"),
    ("package", "bundled/svg/package-check.svg"),
    ("paperclip", "bundled/svg/paperclip.svg"),
    ("pencil", "bundled/svg/pencil-02.svg"),
    ("phone", "bundled/svg/phone.svg"),
    ("play", "bundled/svg/play-white.svg"),
    ("refresh", "bundled/svg/refresh.svg"),
    ("rocket", "bundled/svg/rocket.svg"),
    ("run", "bundled/svg/play-white.svg"),
    ("search", "bundled/svg/search.svg"),
    ("settings", "bundled/svg/gear.svg"),
    ("sliders", "bundled/svg/sliders-04.svg"),
    ("sparkle", "bundled/svg/sparkle.svg"),
    ("star", "bundled/svg/stars-01.svg"),
    ("stop", "bundled/svg/stop.svg"),
    ("sync", "bundled/svg/refresh.svg"),
    ("terminal", "bundled/svg/terminal.svg"),
    ("test", "bundled/svg/check.svg"),
    ("tool", "bundled/svg/tool-01.svg"),
    ("trash", "bundled/svg/trash-02.svg"),
    ("unlock", "bundled/svg/lock-unlocked-01.svg"),
    ("upload", "bundled/svg/upload-01.svg"),
    ("user", "bundled/svg/user.svg"),
    ("users", "bundled/svg/users-02.svg"),
    ("web", "bundled/svg/globe-01.svg"),
    ("workflow", "bundled/svg/workflow.svg"),
];

/// The icon drawn on a toolbar button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ButtonIcon {
    /// One of [`BUNDLED_ICONS`]; the asset path is what the app's icon element loads.
    Bundled {
        name: String,
        asset_path: &'static str,
    },
    /// An SVG file, relative to the project's `.warpium/` directory.
    Custom(PathBuf),
}

/// The bundled asset for an icon name, if the name is known.
pub fn bundled_icon_path(name: &str) -> Option<&'static str> {
    BUNDLED_ICONS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, path)| *path)
}

/// Parses an `icon` value: a bundled icon name, or a relative `.svg` path inside `.warpium/`.
pub fn parse_icon(value: &str) -> Result<ButtonIcon, String> {
    if let Some(asset_path) = bundled_icon_path(value) {
        return Ok(ButtonIcon::Bundled {
            name: value.to_owned(),
            asset_path,
        });
    }
    let path = Path::new(value);
    let is_svg = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"));
    if !is_svg {
        return Err(format!(
            "unknown icon `{value}`: use a bundled icon name (such as `build`, `play`, `bug`, \
             `terminal` or `globe`) or an `.svg` path relative to `.warpium/`"
        ));
    }
    if !is_contained_relative_path(path) {
        return Err(format!(
            "icon path `{value}` must be relative to `.warpium/` and stay inside it"
        ));
    }
    Ok(ButtonIcon::Custom(path.to_path_buf()))
}

/// Whether `path` is relative and never climbs above its base directory.
pub(crate) fn is_contained_relative_path(path: &Path) -> bool {
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

#[cfg(test)]
#[path = "icons_tests.rs"]
mod tests;
