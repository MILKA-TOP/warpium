use super::*;

#[test]
fn places_kmp_source_sets() {
    assert_eq!(
        place("shared/src/commonMain/kotlin/com/example/Platform.kt"),
        Placement {
            module: "shared".into(),
            source_set: "commonMain".into(),
            origin: Origin::Main,
        }
    );
    assert_eq!(
        place("core/data/src/iosMain/kotlin/X.kt"),
        Placement {
            module: "core/data".into(),
            source_set: "iosMain".into(),
            origin: Origin::Main,
        }
    );
}

#[test]
fn places_test_source_sets_as_tests() {
    for path in [
        "app/src/test/java/X.kt",
        "app/src/androidTest/java/X.kt",
        "shared/src/commonTest/kotlin/X.kt",
        "app/src/testFixtures/kotlin/X.kt",
        "shared/src/androidUnitTest/kotlin/X.kt",
    ] {
        assert_eq!(place(path).origin, Origin::Test, "{path}");
    }
}

#[test]
fn places_root_project_sources_in_the_empty_module() {
    assert_eq!(
        place("src/main/kotlin/App.kt"),
        Placement {
            module: String::new(),
            source_set: "main".into(),
            origin: Origin::Main,
        }
    );
}

#[test]
fn places_generated_sources() {
    assert_eq!(
        place("shared/build/generated/ksp/metadata/commonMain/kotlin/com/X.kt"),
        Placement {
            module: "shared".into(),
            source_set: "commonMain".into(),
            origin: Origin::Generated,
        }
    );
    assert_eq!(
        place("app/build/generated/source/buildConfig/debug/com/BuildConfig.java"),
        Placement {
            module: "app".into(),
            source_set: "debug".into(),
            origin: Origin::Generated,
        }
    );
}

#[test]
fn indexes_sources_but_not_build_or_hidden_dirs() {
    assert!(is_indexable("app/src/main/java/com/App.kt"));
    assert!(is_indexable("Legacy.java"));
    assert!(!is_indexable("app/build.gradle.kts"));
    assert!(!is_indexable("app/src/main/res/values/strings.xml"));
    assert!(!is_indexable("app/build/intermediates/X.kt"));
    assert!(!is_indexable(".idea/X.kt"));
    assert!(!is_indexable("app/.gradle/X.kt"));
}

#[test]
fn indexes_generated_sources_except_resource_classes() {
    assert!(is_indexable(
        "app/build/generated/ksp/debug/kotlin/com/Factory.kt"
    ));
    assert!(!is_indexable(
        "app/build/generated/source/r/debug/com/app/R.java"
    ));
    assert!(!is_indexable(
        ".hidden/build/generated/ksp/debug/kotlin/X.kt"
    ));
}

#[test]
fn places_test_fixture_modules_as_tests() {
    for path in [
        "core/testing/src/main/kotlin/X.kt",
        "core/data-test/src/main/kotlin/X.kt",
        "core/screenshot-testing/src/main/kotlin/X.kt",
        "sync/sync-test/src/main/kotlin/X.kt",
        "core/test-fixtures/src/main/kotlin/X.kt",
        "core/testFixtures/src/main/kotlin/X.kt",
        "core/test_utils/src/main/kotlin/X.kt",
        "integration-tests/src/main/kotlin/X.kt",
    ] {
        assert_eq!(place(path).origin, Origin::Test, "{path}");
    }
}

#[test]
fn modules_that_merely_mention_testing_stay_project_code() {
    for path in [
        "core/data/src/main/kotlin/X.kt",
        "core/testing-manifest/src/main/kotlin/X.kt",
        "feature/contest/src/main/kotlin/X.kt",
        "ui-test-hilt-manifest/src/main/kotlin/X.kt",
    ] {
        assert_eq!(place(path).origin, Origin::Main, "{path}");
    }
}
