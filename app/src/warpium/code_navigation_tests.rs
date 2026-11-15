use super::*;

#[test]
fn kotlin_sources_scripts_and_java_are_navigable() {
    for path in ["a/Foo.kt", "build.gradle.kts", "a/Bar.java"] {
        assert!(is_navigable_path(Path::new(path)), "{path}");
    }
}

#[test]
fn other_files_are_not_navigable() {
    for path in ["a/foo.rs", "a/strings.xml", "Makefile", "a/kt"] {
        assert!(!is_navigable_path(Path::new(path)), "{path}");
    }
}
