use std::fs;
use std::path::Path;

use super::*;

fn make_sdk(root: &Path, with_emulator: bool) {
    fs::create_dir_all(root.join("platform-tools")).unwrap();
    fs::write(adb_in(root), "").unwrap();
    if with_emulator {
        fs::create_dir_all(root.join("emulator")).unwrap();
        fs::write(emulator_in(root), "").unwrap();
    }
}

#[test]
fn property_reads_plain_value() {
    let text = "# comment\nndk.dir=/ndk\nsdk.dir=/Users/me/Library/Android/sdk\n";

    assert_eq!(
        property(text, "sdk.dir").as_deref(),
        Some("/Users/me/Library/Android/sdk")
    );
}

#[test]
fn property_unescapes_windows_paths() {
    let text = r"sdk.dir=C\:\\Users\\me\\AppData\\Local\\Android\\Sdk";

    assert_eq!(
        property(text, "sdk.dir").as_deref(),
        Some(r"C:\Users\me\AppData\Local\Android\Sdk")
    );
}

#[test]
fn property_accepts_colon_and_spaces_as_separator() {
    assert_eq!(
        property("sdk.dir : /sdk", "sdk.dir").as_deref(),
        Some("/sdk")
    );
    assert_eq!(property("sdk.dir /sdk", "sdk.dir").as_deref(), Some("/sdk"));
}

#[test]
fn property_joins_continuation_lines() {
    let text = "sdk.dir=/very/long\\\n    /path\n";

    assert_eq!(
        property(text, "sdk.dir").as_deref(),
        Some("/very/long/path")
    );
}

#[test]
fn property_ignores_comments_and_other_keys() {
    let text = "#sdk.dir=/commented\n!sdk.dir=/also\nsdk.dirx=/other\n";

    assert_eq!(property(text, "sdk.dir"), None);
}

#[test]
fn find_sdk_prefers_local_properties() {
    let project = tempfile::tempdir().unwrap();
    let local_sdk = tempfile::tempdir().unwrap();
    let home_sdk = tempfile::tempdir().unwrap();
    make_sdk(local_sdk.path(), true);
    make_sdk(home_sdk.path(), false);
    fs::write(
        project.path().join("local.properties"),
        format!("sdk.dir={}\n", local_sdk.path().display()),
    )
    .unwrap();
    let env = SdkEnv {
        android_home: Some(home_sdk.path().to_path_buf()),
        ..SdkEnv::default()
    };

    let sdk = find_sdk(Some(project.path()), &env).unwrap();

    assert_eq!(sdk.source, SdkSource::LocalProperties);
    assert_eq!(sdk.adb, adb_in(local_sdk.path()));
    assert_eq!(sdk.emulator, Some(emulator_in(local_sdk.path())));
}

#[test]
fn find_sdk_skips_directories_without_adb() {
    let empty = tempfile::tempdir().unwrap();
    let sdk_root = tempfile::tempdir().unwrap();
    make_sdk(sdk_root.path(), false);
    let env = SdkEnv {
        android_home: Some(empty.path().to_path_buf()),
        android_sdk_root: Some(sdk_root.path().to_path_buf()),
        ..SdkEnv::default()
    };

    let sdk = find_sdk(None, &env).unwrap();

    assert_eq!(sdk.source, SdkSource::AndroidSdkRoot);
    assert_eq!(sdk.emulator, None);
}

#[test]
fn find_sdk_falls_back_to_default_location() {
    let home = tempfile::tempdir().unwrap();
    let env = SdkEnv {
        home: Some(home.path().to_path_buf()),
        local_app_data: Some(home.path().to_path_buf()),
        ..SdkEnv::default()
    };
    let default = default_sdk_dir(&env).unwrap();
    make_sdk(&default, true);

    let sdk = find_sdk(None, &env).unwrap();

    assert_eq!(sdk.source, SdkSource::DefaultLocation);
    assert_eq!(sdk.root, Some(default));
}

#[test]
fn find_sdk_falls_back_to_adb_on_path() {
    let bin = tempfile::tempdir().unwrap();
    let adb = bin
        .path()
        .join(if cfg!(windows) { "adb.exe" } else { "adb" });
    fs::write(&adb, "").unwrap();
    let env = SdkEnv {
        path: Some(bin.path().as_os_str().to_owned()),
        ..SdkEnv::default()
    };

    let sdk = find_sdk(None, &env).unwrap();

    assert_eq!(sdk.source, SdkSource::Path);
    assert_eq!(sdk.adb, adb);
    assert_eq!(sdk.root, None);
}

#[test]
fn find_sdk_returns_none_without_any_sdk() {
    assert_eq!(find_sdk(None, &SdkEnv::default()), None);
}
