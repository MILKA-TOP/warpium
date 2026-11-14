use std::path::PathBuf;

use super::*;
use crate::source::WarpChannel;

/// Settings Warpium registers, as dotted TOML paths. `appearance.themes.theme` is a table-valued
/// setting; the others are scalars.
fn known() -> Vec<String> {
    [
        "appearance.text.font_name",
        "appearance.text.font_size",
        "appearance.themes.theme",
        "general.login_item",
        "warpium.sync_from_warp",
    ]
    .iter()
    .map(|path| path.to_string())
    .collect()
}

fn merge(
    source: &str,
    target: Option<&str>,
    base: &BTreeMap<String, String>,
    initial: bool,
) -> SettingsMerge {
    merge_settings_toml(source, target, &known(), &[], base, initial).unwrap()
}

fn parse(contents: &str) -> DocumentMut {
    contents.parse().unwrap()
}

const WARP: &str = r#"
[appearance.text]
font_name = "JetBrains Mono"
font_size = 14

[appearance.themes.theme]
custom = { name = "Dracula", path = "dracula.yaml" }

[general]
login_item = true

[agents.new_setting_from_newer_warp]
enabled = true
"#;

#[test]
fn initial_import_into_an_empty_warpium() {
    let result = merge(WARP, None, &BTreeMap::new(), true);
    let merged = parse(result.merged.as_deref().unwrap());

    assert_eq!(
        merged["appearance"]["text"]["font_name"].as_str(),
        Some("JetBrains Mono")
    );
    assert_eq!(
        merged["appearance"]["text"]["font_size"].as_integer(),
        Some(14)
    );
    assert_eq!(
        merged["appearance"]["themes"]["theme"]["custom"]["name"].as_str(),
        Some("Dracula")
    );
    // Unknown settings still flow over, leaf by leaf.
    assert_eq!(
        merged["agents"]["new_setting_from_newer_warp"]["enabled"].as_bool(),
        Some(true)
    );
    // Launch-at-login is never imported.
    assert!(merged.get("general").is_none());
    assert_eq!(
        result.report.imported,
        vec![
            "agents.new_setting_from_newer_warp.enabled",
            "appearance.text.font_name",
            "appearance.text.font_size",
            "appearance.themes.theme",
        ]
    );
    assert_eq!(result.target_snapshot, None);
}

#[test]
fn keeps_warpium_only_settings_and_formatting() {
    let target = "# My Warpium settings\n[warpium]\nsync_from_warp = false\n\n[terminal]\ncopy_on_select = true # mine\n";
    let result = merge(WARP, Some(target), &BTreeMap::new(), true);
    let merged_text = result.merged.unwrap();
    let merged = parse(&merged_text);

    assert!(merged_text.starts_with("# My Warpium settings"));
    assert!(merged_text.contains("copy_on_select = true # mine"));
    assert_eq!(merged["warpium"]["sync_from_warp"].as_bool(), Some(false));
    assert_eq!(
        merged["appearance"]["text"]["font_size"].as_integer(),
        Some(14)
    );
}

#[test]
fn ongoing_sync_per_setting() {
    let first = merge(WARP, None, &BTreeMap::new(), true);
    let base = first.base.clone();
    let mut warpium = first.merged.unwrap();

    // Warpium changes the font name; Warp changes the font size and the theme, and resets the
    // unknown setting to its default (removing it from the file).
    warpium = warpium.replace("JetBrains Mono", "Fira Code");
    let warp = WARP
        .replace("font_size = 14", "font_size = 16")
        .replace("Dracula", "Nord")
        .replace("[agents.new_setting_from_newer_warp]\nenabled = true\n", "");

    let result = merge(&warp, Some(&warpium), &base, false);
    let merged = parse(result.merged.as_deref().unwrap());
    assert_eq!(
        merged["appearance"]["text"]["font_name"].as_str(),
        Some("Fira Code")
    );
    assert_eq!(
        merged["appearance"]["text"]["font_size"].as_integer(),
        Some(16)
    );
    assert_eq!(
        merged["appearance"]["themes"]["theme"]["custom"]["name"].as_str(),
        Some("Nord")
    );
    assert!(merged.get("agents").is_none());
    assert_eq!(
        result.report.updated,
        vec!["appearance.text.font_size", "appearance.themes.theme"]
    );
    assert_eq!(
        result.report.removed,
        vec!["agents.new_setting_from_newer_warp.enabled"]
    );
    assert!(result.report.conflicts.is_empty());

    // Now both sides change the font name: Warpium's value wins and it's reported.
    let warpium = result.merged.unwrap();
    let warp = warp.replace("JetBrains Mono", "Menlo");
    let result = merge(&warp, Some(&warpium), &result.base, false);
    assert_eq!(result.report.conflicts, vec!["appearance.text.font_name"]);
    assert!(result.merged.is_none());
}

#[test]
fn table_settings_merge_as_one_value() {
    // Warp's theme is a custom theme, Warpium's a built-in one with a different shape. Merging
    // field by field would produce a theme with both shapes.
    let base_merge = merge(WARP, None, &BTreeMap::new(), true);
    let warpium = base_merge.merged.unwrap().replace(
        "custom = { name = \"Dracula\", path = \"dracula.yaml\" }",
        "light = \"Solarized\"",
    );
    let result = merge(WARP, Some(&warpium), &base_merge.base, false);
    // Only Warpium changed the theme: keep it whole.
    assert!(result.merged.is_none());
    assert_eq!(result.report.unchanged, 4);
}

#[test]
fn formatting_differences_are_not_changes() {
    let warp = "[appearance.themes]\ntheme = { custom = { path = \"dracula.yaml\", name = \"Dracula\" } }\n";
    let warpium =
        "[appearance.themes.theme.custom]\nname = \"Dracula\" # same\npath = \"dracula.yaml\"\n";
    let result = merge(warp, Some(warpium), &BTreeMap::new(), false);
    assert!(result.merged.is_none());
    assert_eq!(result.report.unchanged, 1);
    assert!(result.report.conflicts.is_empty());
}

#[test]
fn rewrites_paths_into_warpiums_directories() {
    let home = PathBuf::from("/Users/someone");
    let source = WarpInstallation::for_channel(WarpChannel::Stable, &home);
    let target = WarpiumTarget {
        data_dir: home.join(".warp-oss"),
        home_config_dir: home.join(".warp-oss"),
        settings_file: home.join(".warp-oss/settings.toml"),
        keybindings_file: home.join(".warp-oss/keybindings.yaml"),
    };
    let rewrites = path_rewrites(&source, &target, &home);
    assert_eq!(
        rewrites,
        vec![
            (
                "/Users/someone/.warp".to_owned(),
                "/Users/someone/.warp-oss".to_owned()
            ),
            ("~/.warp".to_owned(), "~/.warp-oss".to_owned()),
        ]
    );

    let warp = r#"
[appearance.themes.theme]
custom = { name = "Mine", path = "/Users/someone/.warp/themes/mine.yaml" }

[session]
working_directory = "~/.warp-projects/x"
dirs = ["~/.warp/workflows", "/Users/someone/.warpish"]
"#;
    let result =
        merge_settings_toml(warp, None, &known(), &rewrites, &BTreeMap::new(), true).unwrap();
    let merged = parse(result.merged.as_deref().unwrap());
    assert_eq!(
        merged["appearance"]["themes"]["theme"]["custom"]["path"].as_str(),
        Some("/Users/someone/.warp-oss/themes/mine.yaml")
    );
    // Only whole path components match.
    assert_eq!(
        merged["session"]["working_directory"].as_str(),
        Some("~/.warp-projects/x")
    );
    let dirs = merged["session"]["dirs"].as_array().unwrap();
    assert_eq!(
        dirs.get(0).and_then(Value::as_str),
        Some("~/.warp-oss/workflows")
    );
    assert_eq!(
        dirs.get(1).and_then(Value::as_str),
        Some("/Users/someone/.warpish")
    );

    // Re-running against the rewritten result is in sync, not a conflict.
    let again = merge_settings_toml(
        warp,
        result.merged.as_deref(),
        &known(),
        &rewrites,
        &result.base,
        false,
    )
    .unwrap();
    assert!(again.merged.is_none());
    assert!(again.report.conflicts.is_empty());
}

#[test]
fn preview_rewrites_both_config_directories() {
    let home = PathBuf::from("/Users/someone");
    let source = WarpInstallation::for_channel(WarpChannel::Preview, &home);
    let target = WarpiumTarget {
        data_dir: home.join(".warp-oss"),
        home_config_dir: home.join(".warp-oss"),
        settings_file: home.join(".warp-oss/settings.toml"),
        keybindings_file: home.join(".warp-oss/keybindings.yaml"),
    };
    let rewrites = path_rewrites(&source, &target, &home);
    assert_eq!(
        rewrite_str("~/.warp-preview/themes/a.yaml", &rewrites).as_deref(),
        Some("~/.warp-oss/themes/a.yaml")
    );
    assert_eq!(
        rewrite_str("/Users/someone/.warp/skills", &rewrites).as_deref(),
        Some("/Users/someone/.warp-oss/skills")
    );
}

#[test]
fn unparseable_files_are_reported_not_touched() {
    assert!(matches!(
        merge_settings_toml("[broken", None, &known(), &[], &BTreeMap::new(), true),
        Err(SettingsMergeError::SourceUnparseable(_))
    ));
    assert!(matches!(
        merge_settings_toml(WARP, Some("x = = 1"), &known(), &[], &BTreeMap::new(), true),
        Err(SettingsMergeError::TargetUnparseable(_))
    ));
}

#[test]
fn removing_the_last_setting_drops_empty_tables() {
    let warp = "[appearance.text]\nfont_size = 14\n";
    let first = merge(warp, None, &BTreeMap::new(), true);
    let result = merge("", first.merged.as_deref(), &first.base, false);
    let merged = result.merged.unwrap();
    assert!(parse(&merged).get("appearance").is_none(), "{merged}");
}

#[test]
fn a_setting_becoming_known_does_not_remove_its_old_leaves() {
    // An older Warpium didn't know `appearance.themes.theme`, so it was merged leaf by leaf.
    let older_known: Vec<String> = Vec::new();
    let first = merge_settings_toml(WARP, None, &older_known, &[], &BTreeMap::new(), true).unwrap();
    assert!(
        first
            .base
            .contains_key("appearance.themes.theme.custom.name")
    );

    // After an upgrade it's one unit: the leaves aren't treated as removed in Warp.
    let result = merge(WARP, first.merged.as_deref(), &first.base, false);
    assert!(
        result.report.removed.is_empty(),
        "{:?}",
        result.report.removed
    );
    let merged = result
        .merged
        .map(|text| parse(&text))
        .unwrap_or_else(|| parse(first.merged.as_deref().unwrap()));
    assert_eq!(
        merged["appearance"]["themes"]["theme"]["custom"]["name"].as_str(),
        Some("Dracula")
    );
    assert!(
        !result
            .base
            .contains_key("appearance.themes.theme.custom.name")
    );
    assert!(result.base.contains_key("appearance.themes.theme"));
}

#[test]
fn keys_containing_dots_are_skipped() {
    let warp = "[x.map]\n\"a.b\" = 1\nc = 2\n";
    let result = merge(warp, None, &BTreeMap::new(), true);
    let merged = parse(result.merged.as_deref().unwrap());
    assert_eq!(merged["x"]["map"]["c"].as_integer(), Some(2));
    assert!(merged["x"]["map"].get("a").is_none());
    assert_eq!(result.report.skipped.len(), 1);
    assert_eq!(result.report.skipped[0].0, "x.map.a.b");
}
