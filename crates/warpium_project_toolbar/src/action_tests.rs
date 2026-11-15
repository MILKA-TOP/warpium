use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn builtin_ids_round_trip() {
    for action in BuiltinAction::ALL {
        assert_eq!(BuiltinAction::from_id(action.id()), Some(*action));
    }
}

#[test]
fn builtin_ids_are_catalog_style_names() {
    assert_eq!(
        BuiltinAction::SurfaceProjectExplorerOpen.id(),
        "surface.project_explorer.open"
    );
    assert_eq!(BuiltinAction::from_id("toggle_project_explorer"), None);
}

#[test]
fn resolves_cwd_relative_to_the_project_root() {
    let root = Path::new("/work/app");

    assert_eq!(resolve_cwd(root, None), PathBuf::from("/work/app"));
    assert_eq!(resolve_cwd(root, Some(".")), PathBuf::from("/work/app"));
    assert_eq!(resolve_cwd(root, Some("android/app")), PathBuf::from("/work/app/android/app"));
    assert_eq!(resolve_cwd(root, Some("a/../b")), PathBuf::from("/work/app/b"));
}

#[test]
fn command_without_cwd_or_env_is_unchanged() {
    for shell in [ShellFamily::Posix, ShellFamily::Fish, ShellFamily::PowerShell] {
        assert_eq!(shell_line("./gradlew test", None, &BTreeMap::new(), shell), "./gradlew test");
    }
}

#[test]
fn posix_runs_in_a_subshell_with_quoted_cwd_and_env() {
    let line = shell_line(
        "./gradlew assembleDebug",
        Some(Path::new("/work/it's here")),
        &env(&[("GRADLE_OPTS", "-Xmx4g -Dx='y'")]),
        ShellFamily::Posix,
    );

    assert_eq!(
        line,
        r#"( cd -- '/work/it'\''s here' && export GRADLE_OPTS='-Xmx4g -Dx='\''y'\''' && ./gradlew assembleDebug )"#
    );
}

#[test]
fn posix_env_only_still_uses_a_subshell() {
    assert_eq!(
        shell_line("make", None, &env(&[("A", "1")]), ShellFamily::Posix),
        "( export A='1' && make )"
    );
}

#[test]
fn fish_restores_the_directory_and_scopes_env_to_the_block() {
    let line = shell_line(
        "make",
        Some(Path::new("/work/it's")),
        &env(&[("A", r"a\b")]),
        ShellFamily::Fish,
    );

    assert_eq!(line, r"if pushd '/work/it\'s'; set -lx A 'a\\b'; make; popd; end");
}

#[test]
fn fish_env_only_uses_a_block() {
    assert_eq!(
        shell_line("make", None, &env(&[("A", "1")]), ShellFamily::Fish),
        "begin; set -lx A '1'; make; end"
    );
}

#[test]
fn powershell_pushes_and_pops_the_location() {
    let line = shell_line(
        "./gradlew.bat test",
        Some(Path::new("C:/work/it's")),
        &env(&[("A", "1")]),
        ShellFamily::PowerShell,
    );

    assert_eq!(
        line,
        "& { Push-Location -LiteralPath 'C:/work/it''s'; try { $env:A = '1'; ./gradlew.bat test } finally { Pop-Location } }"
    );
}

#[test]
fn env_names_must_be_portable() {
    assert!(is_valid_env_name("GRADLE_OPTS"));
    assert!(is_valid_env_name("_x1"));
    assert!(!is_valid_env_name("1X"));
    assert!(!is_valid_env_name("MY-VAR"));
    assert!(!is_valid_env_name(""));
}
