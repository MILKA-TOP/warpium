use std::path::{Path, PathBuf};

use super::*;

#[test]
fn every_bundled_icon_exists_in_the_app_assets() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/assets");

    let missing = BUNDLED_ICONS
        .iter()
        .filter(|(_, path)| !assets.join(path).is_file())
        .map(|(name, path)| format!("{name} -> {path}"))
        .collect::<Vec<_>>();

    assert_eq!(missing, Vec::<String>::new());
}

#[test]
fn bundled_icon_names_are_unique() {
    let mut names = BUNDLED_ICONS.iter().map(|(name, _)| *name).collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();

    assert_eq!(names.len(), BUNDLED_ICONS.len());
}

#[test]
fn parses_a_bundled_name() {
    assert_eq!(
        parse_icon("bug"),
        Ok(ButtonIcon::Bundled {
            name: "bug".to_owned(),
            asset_path: "bundled/svg/bug.svg",
        })
    );
}

#[test]
fn parses_a_relative_svg_path_case_insensitively() {
    assert_eq!(
        parse_icon("icons/Logo.SVG"),
        Ok(ButtonIcon::Custom(PathBuf::from("icons/Logo.SVG")))
    );
}

#[test]
fn rejects_non_svg_files() {
    assert!(parse_icon("logo.png").is_err());
}

#[test]
fn rejects_absolute_and_escaping_svg_paths() {
    assert!(parse_icon("/tmp/logo.svg").is_err());
    assert!(parse_icon("../logo.svg").is_err());
    assert!(parse_icon("icons/../../logo.svg").is_err());
}
