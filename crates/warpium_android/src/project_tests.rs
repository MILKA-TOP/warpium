use std::fs;

use super::*;

#[test]
fn parses_kotlin_settings_includes() {
    let settings = r#"
pluginManagement { repositories { google() } }
rootProject.name = "Demo"
include(":app")
include(
    ":core:data", // the data layer
    ":feature:home",
)
// include(":disabled")
"#;

    assert_eq!(
        parse_settings_modules(settings),
        vec![":app", ":core:data", ":feature:home"]
    );
}

#[test]
fn parses_groovy_settings_includes() {
    let settings =
        "include ':app', ':lib'\ninclude ':wear',\n        ':tv'\nrootProject.name = 'x'\n";

    assert_eq!(
        parse_settings_modules(settings),
        vec![":app", ":lib", ":wear", ":tv"]
    );
}

#[test]
fn recognizes_application_plugin_spellings() {
    assert!(is_app_build_script(
        r#"plugins { id("com.android.application") }"#
    ));
    assert!(is_app_build_script(
        "plugins { alias(libs.plugins.android.application) }"
    ));
    assert!(is_app_build_script(
        "plugins { alias(libs.plugins.androidApplication) }"
    ));
    assert!(is_app_build_script(
        r#"plugins { id("nowinandroid.android.application") }"#
    ));
    assert!(!is_app_build_script(
        r#"plugins { id("com.android.library") }"#
    ));
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn finds_app_modules_with_ids_and_debug_suffix() {
    let root = tempfile::tempdir().unwrap();
    write(
        &root.path().join("settings.gradle.kts"),
        "include(\":app\", \":core\", \":wear:app\")\n",
    );
    write(
        &root.path().join("app/build.gradle.kts"),
        r#"plugins { alias(libs.plugins.android.application) }
android {
    namespace = "com.example.ns"
    defaultConfig { applicationId = "com.example.app" }
    buildTypes {
        debug { applicationIdSuffix = ".debug" }
        release { isMinifyEnabled = true }
    }
}"#,
    );
    write(
        &root.path().join("core/build.gradle.kts"),
        r#"plugins { id("com.android.library") }"#,
    );
    write(
        &root.path().join("wear/app/build.gradle"),
        "apply plugin: 'com.android.application'\nandroid { namespace 'com.example.wear' }\n",
    );

    let modules = find_app_modules(root.path());

    assert_eq!(
        modules,
        vec![
            AndroidAppModule {
                gradle_path: ":app".to_owned(),
                dir: root.path().join("app"),
                application_id: Some("com.example.app".to_owned()),
                debug_suffix: Some(".debug".to_owned()),
            },
            AndroidAppModule {
                gradle_path: ":wear:app".to_owned(),
                dir: root.path().join("wear/app"),
                application_id: Some("com.example.wear".to_owned()),
                debug_suffix: None,
            },
        ]
    );
}

#[test]
fn finds_single_module_root_project() {
    let root = tempfile::tempdir().unwrap();
    write(
        &root.path().join("build.gradle"),
        "plugins { id 'com.android.application' }\nandroid { defaultConfig { applicationId \"com.solo\" } }\n",
    );

    let modules = find_app_modules(root.path());

    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].gradle_path, "");
    assert_eq!(modules[0].install_task("debug"), ":installDebug");
}

#[test]
fn install_task_capitalizes_the_variant() {
    let module = AndroidAppModule {
        gradle_path: ":wear:app".to_owned(),
        dir: PathBuf::from("/p/wear/app"),
        application_id: None,
        debug_suffix: None,
    };

    assert_eq!(
        module.install_task("freeDebug"),
        ":wear:app:installFreeDebug"
    );
    assert_eq!(module.display_name(), "app");
}

#[test]
fn package_for_variant_prefers_build_metadata() {
    let root = tempfile::tempdir().unwrap();
    let module = AndroidAppModule {
        gradle_path: ":app".to_owned(),
        dir: root.path().join("app"),
        application_id: Some("com.example".to_owned()),
        debug_suffix: Some(".debug".to_owned()),
    };
    let guessed = module.package_for_variant("debug");
    write(
        &root
            .path()
            .join("app/build/outputs/apk/debug/output-metadata.json"),
        r#"{ "version": 3, "artifactType": { "type": "APK" }, "applicationId": "com.example.dev", "variantName": "debug" }"#,
    );

    assert_eq!(guessed.as_deref(), Some("com.example.debug"));
    assert_eq!(
        module.package_for_variant("debug").as_deref(),
        Some("com.example.dev")
    );
    assert_eq!(
        module.package_for_variant("release").as_deref(),
        Some("com.example")
    );
}

#[test]
fn detects_android_projects_by_manifest() {
    let library = tempfile::tempdir().unwrap();
    write(
        &library.path().join("settings.gradle.kts"),
        "include(\":shared\")\n",
    );
    write(
        &library
            .path()
            .join("shared/src/androidMain/AndroidManifest.xml"),
        "<manifest/>",
    );
    let plain = tempfile::tempdir().unwrap();
    write(
        &plain.path().join("settings.gradle.kts"),
        "include(\":cli\")\n",
    );
    write(
        &plain.path().join("cli/build.gradle.kts"),
        "plugins { kotlin(\"jvm\") }\n",
    );

    assert!(is_android_project(library.path()));
    assert!(!is_android_project(plain.path()));
}

#[test]
fn launch_app_command_quotes_the_package() {
    let command = launch_app_command("com.example.app");

    assert!(
        command.contains(
            "resolve-activity --brief -c android.intent.category.LAUNCHER com.example.app"
        )
    );
    assert!(command.contains("monkey -p com.example.app"));
    assert!(launch_app_command("evil;reboot").contains("'evil;reboot'"));
}
