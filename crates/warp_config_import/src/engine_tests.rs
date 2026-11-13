use super::*;
use crate::source::WarpChannel;

fn target_in(home: &Path) -> WarpiumTarget {
    let warpium = home.join(".warp-oss");
    WarpiumTarget {
        data_dir: warpium.clone(),
        home_config_dir: warpium.clone(),
        settings_file: warpium.join("settings.toml"),
        keybindings_file: warpium.join("keybindings.yaml"),
    }
}

fn options(run_id: &str) -> ImportOptions {
    ImportOptions {
        dry_run: false,
        run_id: run_id.to_owned(),
    }
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn known() -> Vec<String> {
    vec!["appearance.text.font_size".to_owned()]
}

#[test]
fn nothing_to_import_without_warp() {
    let home = tempfile::tempdir().unwrap();
    let target = target_in(home.path());
    assert!(prepare_import(home.path(), &target, &known(), &options("r")).is_none());
}

#[test]
fn end_to_end_import_and_commit() {
    let home = tempfile::tempdir().unwrap();
    let target = target_in(home.path());
    write(
        &home.path().join(".warp/settings.toml"),
        "[appearance.text]\nfont_size = 15\n",
    );
    write(&home.path().join(".warp/themes/a.yaml"), "a");
    write(&target.settings_file, "[terminal]\ncopy_on_select = true\n");

    let prepared = prepare_import(home.path(), &target, &known(), &options("run1")).unwrap();
    assert_eq!(prepared.source.channel, WarpChannel::Stable);
    assert!(prepared.initial_import);
    assert_eq!(prepared.report.imported, vec!["themes/a.yaml"]);
    assert_eq!(prepared.manifest.source_channel.as_deref(), Some("stable"));

    let merge = prepared.settings.unwrap().unwrap();
    assert_eq!(merge.report.imported, vec!["appearance.text.font_size"]);
    assert!(commit_settings_merge(&merge, &target, &options("run1")).unwrap());

    let written = std::fs::read_to_string(&target.settings_file).unwrap();
    assert!(written.contains("copy_on_select = true"));
    assert!(written.contains("font_size = 15"));
    assert_eq!(
        std::fs::read_to_string(target.backup_dir("run1").join("settings.toml")).unwrap(),
        "[terminal]\ncopy_on_select = true\n"
    );
}

#[test]
fn commit_refuses_a_stale_merge() {
    let home = tempfile::tempdir().unwrap();
    let target = target_in(home.path());
    write(
        &home.path().join(".warp/settings.toml"),
        "[appearance.text]\nfont_size = 15\n",
    );
    let prepared = prepare_import(home.path(), &target, &known(), &options("run1")).unwrap();
    let merge = prepared.settings.unwrap().unwrap();

    // Warpium wrote its settings file after the merge was computed.
    write(&target.settings_file, "[terminal]\ncopy_on_select = true\n");
    assert!(matches!(
        commit_settings_merge(&merge, &target, &options("run1")),
        Err(CommitError::TargetChanged)
    ));
    assert_eq!(
        std::fs::read_to_string(&target.settings_file).unwrap(),
        "[terminal]\ncopy_on_select = true\n"
    );
}

#[test]
fn dry_run_commit_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let target = target_in(home.path());
    write(
        &home.path().join(".warp/settings.toml"),
        "[appearance.text]\nfont_size = 15\n",
    );
    let dry_run = ImportOptions {
        dry_run: true,
        run_id: "dry".to_owned(),
    };
    let prepared = prepare_import(home.path(), &target, &known(), &dry_run).unwrap();
    assert!(prepared.report.dry_run);
    let merge = prepared.settings.unwrap().unwrap();
    assert!(merge.merged.is_some());
    assert!(!commit_settings_merge(&merge, &target, &dry_run).unwrap());
    assert!(!target.settings_file.exists());
}

#[test]
fn missing_warp_settings_file_skips_the_settings_phase() {
    let home = tempfile::tempdir().unwrap();
    let target = target_in(home.path());
    std::fs::create_dir_all(home.path().join(".warp")).unwrap();
    let prepared = prepare_import(home.path(), &target, &known(), &options("r")).unwrap();
    assert!(prepared.settings.is_none());
}
