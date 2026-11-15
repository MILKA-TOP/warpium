use std::fs;

use super::*;

#[test]
fn parses_kotlin_frame() {
    let line = "\tat com.example.app.ui.MainViewModel.load(MainViewModel.kt:42)";

    let frame = parse_frame(line).unwrap();

    assert_eq!(
        frame,
        StackFrame {
            class_name: "com.example.app.ui.MainViewModel".to_owned(),
            method: "load".to_owned(),
            file: "MainViewModel.kt".to_owned(),
            line: Some(42),
            span: 4..line.len(),
        }
    );
}

#[test]
fn parses_inner_classes_and_lambdas() {
    let frame = parse_frame("at com.example.Screen$Content$1$2.invoke(Screen.kt:88)").unwrap();

    assert_eq!(frame.class_name, "com.example.Screen$Content$1$2");
    assert_eq!(frame.method, "invoke");
    assert_eq!(frame.outer_simple_name(), "Screen");
    assert_eq!(frame.package(), "com.example");
}

#[test]
fn parses_module_and_class_loader_prefixes() {
    let platform = parse_frame("    at java.base/java.lang.Thread.run(Thread.java:833)").unwrap();
    let app = parse_frame("at app//com.example.Main.onCreate(Main.java:10)").unwrap();

    assert_eq!(platform.class_name, "java.lang.Thread");
    assert_eq!(app.class_name, "com.example.Main");
    assert_eq!(app.line, Some(10));
}

#[test]
fn parses_frames_without_line_numbers() {
    let native = parse_frame("at android.os.BinderProxy.transactNative(Native Method)").unwrap();
    let unknown = parse_frame("at com.example.Foo.bar(Unknown Source:4)").unwrap();
    let no_line = parse_frame("at com.example.Foo.bar(Foo.kt)").unwrap();

    assert_eq!((native.file.as_str(), native.line), ("", None));
    assert_eq!((unknown.file.as_str(), unknown.line), ("", Some(4)));
    assert_eq!((no_line.file.as_str(), no_line.line), ("Foo.kt", None));
}

#[test]
fn parses_mangled_kotlin_method_names() {
    let frame = parse_frame("at com.example.Money.plus-abc123(Money.kt:5)").unwrap();

    assert_eq!(frame.method, "plus-abc123");
}

#[test]
fn rejects_non_frame_lines() {
    assert_eq!(parse_frame("java.lang.IllegalStateException: boom"), None);
    assert_eq!(parse_frame("Caused by: java.io.IOException"), None);
    assert_eq!(parse_frame("\t... 12 more"), None);
    assert_eq!(parse_frame("look at this(thing)"), None);
}

fn frame(class_name: &str, file: &str) -> StackFrame {
    StackFrame {
        class_name: class_name.to_owned(),
        method: "m".to_owned(),
        file: file.to_owned(),
        line: Some(1),
        span: 0..0,
    }
}

#[test]
fn lookup_names_go_from_nested_to_top_level() {
    assert_eq!(
        frame("com.example.Screen$Content$1", "Screen.kt").lookup_names(),
        vec!["com.example.Screen.Content", "com.example.Screen"]
    );
    assert_eq!(frame("Main", "Main.java").lookup_names(), vec!["Main"]);
}

#[test]
fn lookup_names_skip_kotlin_file_facades() {
    assert!(
        frame("com.example.UtilsKt", "Utils.kt")
            .lookup_names()
            .is_empty()
    );
    assert_eq!(
        frame("com.example.SdkKt", "SdkKt.java").lookup_names(),
        vec!["com.example.SdkKt"]
    );
}

#[test]
fn choose_source_file_prefers_matching_file_name() {
    let files = vec![
        PathBuf::from("/p/a/Other.kt"),
        PathBuf::from("/p/b/Screen.kt"),
    ];

    assert_eq!(
        choose_source_file(&frame("com.example.Screen", "Screen.kt"), &files),
        Some(PathBuf::from("/p/b/Screen.kt"))
    );
}

#[test]
fn choose_source_file_finds_sibling_file() {
    let dir = tempfile::tempdir().unwrap();
    let class_file = dir.path().join("Models.kt");
    let inline_file = dir.path().join("Extensions.kt");
    fs::write(&class_file, "").unwrap();
    fs::write(&inline_file, "").unwrap();

    assert_eq!(
        choose_source_file(&frame("com.example.User", "Extensions.kt"), &[class_file]),
        Some(inline_file)
    );
}

#[test]
fn choose_source_file_without_frame_file_takes_the_class_file() {
    let files = vec![PathBuf::from("/p/Foo.kt")];

    assert_eq!(
        choose_source_file(&frame("com.example.Foo", ""), &files),
        Some(PathBuf::from("/p/Foo.kt"))
    );
}

#[test]
fn find_source_file_prefers_the_package_directory() {
    let root = tempfile::tempdir().unwrap();
    let wrong = root.path().join("feature/src/main/java/org/other");
    let right = root.path().join("app/src/main/java/com/example/util");
    let generated = root.path().join("app/build/generated/com/example/util");
    for dir in [&wrong, &right, &generated] {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("Utils.kt"), "").unwrap();
    }

    let found = find_source_file(root.path(), &frame("com.example.util.UtilsKt", "Utils.kt"));

    assert_eq!(found, Some(right.join("Utils.kt")));
}

#[test]
fn find_source_file_falls_back_to_any_file_with_the_name() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("src/flat");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("Main.java"), "").unwrap();

    let found = find_source_file(root.path(), &frame("com.example.Main", "Main.java"));

    assert_eq!(found, Some(dir.join("Main.java")));
}
