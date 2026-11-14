use std::path::Path;

use super::*;
use crate::ManifestLoad;
use crate::source::WarpChannel;

struct Fixture {
    home: tempfile::TempDir,
    source: WarpInstallation,
    target: WarpiumTarget,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let source = WarpInstallation::for_channel(WarpChannel::Stable, home.path());
        let warpium = home.path().join(".warp-oss");
        let target = WarpiumTarget {
            data_dir: warpium.clone(),
            home_config_dir: warpium.clone(),
            settings_file: warpium.join("settings.toml"),
            keybindings_file: warpium.join("keybindings.yaml"),
        };
        Self {
            home,
            source,
            target,
        }
    }

    fn write_source(&self, relative: &str, contents: &str) {
        write(&self.home.path().join(".warp").join(relative), contents);
    }

    fn write_target(&self, relative: &str, contents: &str) {
        write(&self.target.data_dir.join(relative), contents);
    }

    fn read_target(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.target.data_dir.join(relative)).ok()
    }

    /// Runs one file sync the way the app does, persisting the manifest between runs.
    fn sync(&self, dry_run: bool, run_id: &str) -> ImportReport {
        let manifest_path = self.target.manifest_path();
        let load = Manifest::load(&manifest_path);
        let initial = load.is_initial_import();
        let mut manifest = load.into_manifest();
        let mut report = ImportReport::default();
        let options = ImportOptions {
            dry_run,
            run_id: run_id.to_owned(),
        };
        sync_file_categories(
            &file_categories(&self.source, &self.target),
            &self.target,
            &mut manifest,
            initial,
            &options,
            &mut report,
        );
        if !dry_run {
            manifest.save(&manifest_path).unwrap();
        }
        report
    }
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn initial_import_copies_every_category() {
    let fixture = Fixture::new();
    fixture.write_source("themes/dracula.yaml", "accent: '#ff79c6'");
    fixture.write_source("workflows/nested/deploy.yaml", "name: deploy");
    fixture.write_source("launch_configurations/work.yaml", "name: work");
    fixture.write_source("tab_configs/repo.toml", "name = 'repo'");
    fixture.write_source("skills/review/SKILL.md", "# Review");
    fixture.write_source("keybindings.yaml", "workspace:new_tab: cmd-t");
    fixture.write_source(".mcp.json", r#"{"mcpServers": {"fs": {"command": "npx"}}}"#);
    // Not configuration: never imported.
    fixture.write_source("themes/.DS_Store", "junk");
    fixture.write_source("skills/review/.git/HEAD", "ref");
    fixture.write_source("worktrees/repo/file", "generated");

    let report = fixture.sync(false, "run1");

    assert_eq!(
        report.imported,
        vec![
            "themes/dracula.yaml",
            "workflows/nested/deploy.yaml",
            "launch_configurations/work.yaml",
            "tab_configs/repo.toml",
            "skills/review/SKILL.md",
            "keybindings.yaml",
            ".mcp.json",
        ]
    );
    assert!(report.conflicts.is_empty() && report.errors.is_empty());
    assert_eq!(
        fixture
            .read_target("workflows/nested/deploy.yaml")
            .as_deref(),
        Some("name: deploy")
    );
    assert_eq!(
        fixture.read_target("keybindings.yaml").as_deref(),
        Some("workspace:new_tab: cmd-t")
    );
    assert!(fixture.read_target("themes/.DS_Store").is_none());
    assert!(fixture.read_target("skills/review/.git/HEAD").is_none());
    assert!(fixture.read_target("worktrees/repo/file").is_none());

    // Nothing changed since: the next run is a no-op.
    let report = fixture.sync(false, "run2");
    assert_eq!(report.changed_count(), 0);
    assert_eq!(report.unchanged, 7);
}

#[test]
fn ongoing_sync_follows_the_three_way_rule() {
    let fixture = Fixture::new();
    fixture.write_source("themes/a.yaml", "a1");
    fixture.write_source("themes/b.yaml", "b1");
    fixture.write_source("themes/c.yaml", "c1");
    fixture.write_source("themes/d.yaml", "d1");
    fixture.sync(false, "run1");

    // a: changed in Warp only -> updated.
    fixture.write_source("themes/a.yaml", "a2");
    // b: changed in Warpium only -> kept.
    fixture.write_target("themes/b.yaml", "b-local");
    // c: changed on both sides -> conflict, Warpium's version kept.
    fixture.write_source("themes/c.yaml", "c2");
    fixture.write_target("themes/c.yaml", "c-local");
    // d: deleted in Warp -> kept in Warpium.
    std::fs::remove_file(fixture.home.path().join(".warp/themes/d.yaml")).unwrap();
    // e: new in Warp -> imported. f: Warpium-only -> untouched.
    fixture.write_source("themes/e.yaml", "e1");
    fixture.write_target("themes/f.yaml", "f-local");

    let report = fixture.sync(false, "run2");
    assert_eq!(report.updated, vec!["themes/a.yaml"]);
    assert_eq!(report.imported, vec!["themes/e.yaml"]);
    assert_eq!(report.conflicts, vec!["themes/c.yaml"]);
    assert_eq!(
        report.skipped,
        vec![(
            "themes/d.yaml".to_owned(),
            "removed in Warp; kept in Warpium".to_owned()
        )]
    );
    assert_eq!(fixture.read_target("themes/a.yaml").as_deref(), Some("a2"));
    assert_eq!(
        fixture.read_target("themes/b.yaml").as_deref(),
        Some("b-local")
    );
    assert_eq!(
        fixture.read_target("themes/c.yaml").as_deref(),
        Some("c-local")
    );
    assert_eq!(fixture.read_target("themes/d.yaml").as_deref(), Some("d1"));
    assert_eq!(
        fixture.read_target("themes/f.yaml").as_deref(),
        Some("f-local")
    );

    // An update replaces an unmodified file, which is backed up first.
    assert_eq!(
        std::fs::read_to_string(fixture.target.backup_dir("run2").join("themes/a.yaml")).unwrap(),
        "a1"
    );

    // d is now Warpium's: a later run doesn't report it again.
    let report = fixture.sync(false, "run3");
    assert!(report.skipped.is_empty());
    assert_eq!(report.conflicts, vec!["themes/c.yaml"]);
}

#[test]
fn first_import_overwrites_warpium_files_with_a_backup() {
    let fixture = Fixture::new();
    fixture.write_source("keybindings.yaml", "from-warp");
    fixture.write_target("keybindings.yaml", "from-warpium");

    let report = fixture.sync(false, "first");
    assert_eq!(report.updated, vec!["keybindings.yaml"]);
    assert_eq!(
        fixture.read_target("keybindings.yaml").as_deref(),
        Some("from-warp")
    );
    assert_eq!(
        std::fs::read_to_string(fixture.target.backup_dir("first").join("keybindings.yaml"))
            .unwrap(),
        "from-warpium"
    );
}

#[test]
fn dry_run_writes_nothing() {
    let fixture = Fixture::new();
    fixture.write_source("themes/a.yaml", "a1");
    fixture.write_source("keybindings.yaml", "from-warp");
    fixture.write_target("keybindings.yaml", "from-warpium");

    let report = fixture.sync(true, "dry");
    assert_eq!(report.imported, vec!["themes/a.yaml"]);
    assert_eq!(report.updated, vec!["keybindings.yaml"]);
    assert!(fixture.read_target("themes/a.yaml").is_none());
    assert_eq!(
        fixture.read_target("keybindings.yaml").as_deref(),
        Some("from-warpium")
    );
    assert!(!fixture.target.backup_dir("dry").exists());
    assert!(matches!(
        Manifest::load(&fixture.target.manifest_path()),
        ManifestLoad::Missing
    ));
}

#[test]
fn mcp_config_with_inline_credentials_is_skipped() {
    let fixture = Fixture::new();
    fixture.write_source(
        ".mcp.json",
        r#"{"mcpServers": {"gh": {"command": "gh-mcp", "env": {"GITHUB_TOKEN": "ghp_x"}}}}"#,
    );
    let report = fixture.sync(false, "run1");
    assert!(report.imported.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].0, ".mcp.json");
    assert!(fixture.read_target(".mcp.json").is_none());
}

#[test]
fn inline_secret_heuristic() {
    assert!(!json_has_inline_secrets(
        br#"{"mcpServers": {"fs": {"command": "npx", "args": ["-y", "server"], "env": {}}}}"#
    ));
    assert!(json_has_inline_secrets(
        br#"{"mcpServers": {"x": {"url": "https://x", "headers": {"Authorization": "Bearer y"}}}}"#
    ));
    assert!(json_has_inline_secrets(
        br#"{"servers": [{"apiKey": "k"}]}"#
    ));
    assert!(!json_has_inline_secrets(
        br#"{"servers": [{"api_key": ""}]}"#
    ));
    assert!(json_has_inline_secrets(b"{ not json"));
}

#[cfg(unix)]
#[test]
fn follows_symlinked_directories_and_keeps_executable_bits() {
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = Fixture::new();
    // Preview's config directory links its entries into Stable's.
    let preview_dir = fixture.home.path().join(".warp-preview");
    std::fs::create_dir_all(&preview_dir).unwrap();
    fixture.write_source("themes/linked.yaml", "linked");
    std::os::unix::fs::symlink(
        fixture.home.path().join(".warp/themes"),
        preview_dir.join("themes"),
    )
    .unwrap();
    let script = fixture.home.path().join(".warp/skills/tool/run.sh");
    write(&script, "#!/bin/sh");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let source = WarpInstallation::for_channel(WarpChannel::Preview, fixture.home.path());
    let mut manifest = Manifest::default();
    let mut report = ImportReport::default();
    sync_file_categories(
        &file_categories(&source, &fixture.target),
        &fixture.target,
        &mut manifest,
        true,
        &ImportOptions {
            dry_run: false,
            run_id: "preview".to_owned(),
        },
        &mut report,
    );

    assert_eq!(
        fixture.read_target("themes/linked.yaml").as_deref(),
        Some("linked")
    );
    let mode = std::fs::metadata(fixture.target.data_dir.join("skills/tool/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
}
