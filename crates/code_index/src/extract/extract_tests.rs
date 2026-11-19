use super::*;
use crate::model::flags;

fn kotlin(source: &str) -> RawFile {
    extract(Language::Kotlin, source.as_bytes()).expect("Kotlin parses")
}

fn java(source: &str) -> RawFile {
    extract(Language::Java, source.as_bytes()).expect("Java parses")
}

/// `(kind, container, name)` of every symbol, in source order.
fn outline(file: &RawFile) -> Vec<(SymbolKind, &str, &str)> {
    file.symbols
        .iter()
        .map(|symbol| (symbol.kind, symbol.container.as_str(), symbol.name.as_str()))
        .collect()
}

fn find<'a>(file: &'a RawFile, name: &str) -> &'a RawSymbol {
    file.symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol named {name}"))
}

#[test]
fn kotlin_records_package_and_imports() {
    let file = kotlin(
        "package com.example.app\n\
         import kotlinx.coroutines.flow.Flow\n\
         import com.example.Bar as Baz\n\
         import com.example.util.*\n",
    );

    assert_eq!(file.package, "com.example.app");
    assert_eq!(
        file.imports,
        vec![
            RawImport {
                path: "kotlinx.coroutines.flow.Flow".into(),
                alias: None,
                wildcard: false,
                is_static: false,
            },
            RawImport {
                path: "com.example.Bar".into(),
                alias: Some("Baz".into()),
                wildcard: false,
                is_static: false,
            },
            RawImport {
                path: "com.example.util".into(),
                alias: None,
                wildcard: true,
                is_static: false,
            },
        ]
    );
}

#[test]
fn kotlin_extracts_type_kinds() {
    let file = kotlin(
        "class A\n\
         interface B\n\
         object C\n\
         enum class D { X, Y }\n\
         annotation class E\n\
         typealias F = Map<String, Int>\n\
         fun interface G { fun run() }\n",
    );

    assert_eq!(
        outline(&file),
        vec![
            (SymbolKind::Class, "", "A"),
            (SymbolKind::Interface, "", "B"),
            (SymbolKind::Object, "", "C"),
            (SymbolKind::Enum, "", "D"),
            (SymbolKind::EnumEntry, "D", "X"),
            (SymbolKind::EnumEntry, "D", "Y"),
            (SymbolKind::Annotation, "", "E"),
            (SymbolKind::TypeAlias, "", "F"),
            (SymbolKind::Interface, "", "G"),
            (SymbolKind::Function, "G", "run"),
        ]
    );
}

#[test]
fn kotlin_nests_members_under_their_containers() {
    let file = kotlin(
        "class User(val id: Int, name: String) {\n\
             companion object { fun create() = User(1, \"\") }\n\
             inner class Session { val token = \"\" }\n\
             constructor() : this(0, \"\")\n\
         }\n\
         class Repo { companion object Factory }\n",
    );

    assert_eq!(
        outline(&file),
        vec![
            (SymbolKind::Class, "", "User"),
            (SymbolKind::Property, "User", "id"),
            (SymbolKind::CompanionObject, "User", "Companion"),
            (SymbolKind::Function, "User.Companion", "create"),
            (SymbolKind::Class, "User", "Session"),
            (SymbolKind::Property, "User.Session", "token"),
            (SymbolKind::Constructor, "User", "User"),
            (SymbolKind::Class, "", "Repo"),
            (SymbolKind::CompanionObject, "Repo", "Factory"),
        ]
    );
}

#[test]
fn kotlin_skips_local_declarations() {
    let file = kotlin("fun outer() {\n    fun local() {}\n    val x = 1\n    class Local\n}\n");

    assert_eq!(outline(&file), vec![(SymbolKind::Function, "", "outer")]);
}

#[test]
fn kotlin_records_extension_receivers() {
    let file = kotlin(
        "fun String.shout(): String = uppercase()\n\
         fun <T> List<T>.second(): T = this[1]\n\
         val Int.squared get() = this * this\n",
    );

    assert_eq!(find(&file, "shout").receiver.as_deref(), Some("String"));
    assert_eq!(find(&file, "second").receiver.as_deref(), Some("List<T>"));
    assert_eq!(find(&file, "squared").receiver.as_deref(), Some("Int"));
}

#[test]
fn kotlin_reads_visibility_and_platform_modifiers() {
    let file = kotlin(
        "expect class Platform\n\
         actual fun platformName(): String = \"\"\n\
         internal data class Model(val x: Int)\n\
         private const val LIMIT = 3\n\
         sealed interface State\n",
    );

    assert_eq!(find(&file, "Platform").flags, flags::EXPECT);
    assert_eq!(find(&file, "platformName").flags, flags::ACTUAL);
    assert_eq!(find(&file, "Model").visibility, Visibility::Internal);
    assert_eq!(find(&file, "Model").flags, flags::DATA);
    assert_eq!(find(&file, "LIMIT").visibility, Visibility::Private);
    assert_eq!(find(&file, "LIMIT").flags, flags::CONST);
    assert_eq!(find(&file, "State").flags, flags::SEALED);
    assert_eq!(find(&file, "x").visibility, Visibility::Public);
}

#[test]
fn kotlin_points_at_the_name_and_spans_the_declaration() {
    let file = kotlin("@Deprecated(\"x\")\nclass Old {\n    fun a() {}\n}\n");

    let class = find(&file, "Old");
    assert_eq!(class.name_pos, Position { line: 1, column: 6 });
    assert_eq!((class.start_line, class.end_line), (0, 3));
}

#[test]
fn columns_count_characters_not_bytes() {
    let file = kotlin("@Named(\"пользователь\") val userRepo = 1\n");

    assert_eq!(
        find(&file, "userRepo").name_pos,
        Position {
            line: 0,
            column: 27
        }
    );
}

#[test]
fn kotlin_destructuring_declares_each_name() {
    let file = kotlin("val (first, second) = 1 to 2\n");

    assert_eq!(
        outline(&file),
        vec![
            (SymbolKind::Property, "", "first"),
            (SymbolKind::Property, "", "second"),
        ]
    );
}

#[test]
fn java_records_package_and_imports() {
    let file = java(
        "package com.example.j;\nimport java.util.List;\nimport static java.util.Collections.*;\n",
    );

    assert_eq!(file.package, "com.example.j");
    assert_eq!(
        file.imports,
        vec![
            RawImport {
                path: "java.util.List".into(),
                alias: None,
                wildcard: false,
                is_static: false,
            },
            RawImport {
                path: "java.util.Collections".into(),
                alias: None,
                wildcard: true,
                is_static: true,
            },
        ]
    );
}

#[test]
fn java_extracts_types_and_members() {
    let file = java(
        "public class Outer {\n\
           public static final int MAX = 3;\n\
           private String a, b;\n\
           Outer() {}\n\
           public void run() { new Runnable() { public void run() {} }; }\n\
           interface Cb { void call(); }\n\
           enum Mode { A; void m() {} }\n\
           @interface Ann {}\n\
           record Point(int x) {}\n\
         }\n",
    );

    assert_eq!(
        outline(&file),
        vec![
            (SymbolKind::Class, "", "Outer"),
            (SymbolKind::Property, "Outer", "MAX"),
            (SymbolKind::Property, "Outer", "a"),
            (SymbolKind::Property, "Outer", "b"),
            (SymbolKind::Constructor, "Outer", "Outer"),
            (SymbolKind::Function, "Outer", "run"),
            (SymbolKind::Interface, "Outer", "Cb"),
            (SymbolKind::Function, "Outer.Cb", "call"),
            (SymbolKind::Enum, "Outer", "Mode"),
            (SymbolKind::EnumEntry, "Outer.Mode", "A"),
            (SymbolKind::Function, "Outer.Mode", "m"),
            (SymbolKind::Annotation, "Outer", "Ann"),
            (SymbolKind::Record, "Outer", "Point"),
            (SymbolKind::Property, "Outer.Point", "x"),
        ]
    );
}

#[test]
fn java_defaults_to_package_private_except_in_interfaces() {
    let file = java(
        "class Impl { void hidden() {} public static void open() {} }\n\
         interface Api { void call(); }\n",
    );

    assert_eq!(find(&file, "Impl").visibility, Visibility::PackagePrivate);
    assert_eq!(find(&file, "hidden").visibility, Visibility::PackagePrivate);
    assert_eq!(find(&file, "open").visibility, Visibility::Public);
    assert_eq!(find(&file, "open").flags, flags::STATIC);
    assert_eq!(find(&file, "call").visibility, Visibility::Public);
}

#[test]
fn kotlin_records_declared_supertypes_without_type_arguments() {
    let file = kotlin(
        "class Repo(d: Dao) : Base<Item>(), com.example.Listener, Outer.Inner, Closeable by d\n\
         object Single : Base<Item>()\n",
    );

    assert_eq!(
        file.supertypes,
        vec![
            (0, "Base".to_owned()),
            (0, "com.example.Listener".to_owned()),
            (0, "Outer.Inner".to_owned()),
            (0, "Closeable".to_owned()),
            (1, "Base".to_owned()),
        ]
    );
}

#[test]
fn java_records_superclass_and_interfaces() {
    let file = java(
        "class Repo extends Base<Item> implements Listener, java.io.Closeable {}\n\
         interface Api extends Remote {}\n",
    );

    assert_eq!(
        file.supertypes,
        vec![
            (0, "Base".to_owned()),
            (0, "Listener".to_owned()),
            (0, "java.io.Closeable".to_owned()),
            (1, "Remote".to_owned()),
        ]
    );
}

#[test]
fn kotlin_override_members_are_flagged() {
    let file = kotlin(
        "class Square(override val name: String) : Shape {\n\
             override fun area() = 1.0\n\
             fun scale() {}\n\
         }\n",
    );

    assert!(find(&file, "name").flags & flags::OVERRIDE != 0);
    assert!(find(&file, "area").flags & flags::OVERRIDE != 0);
    assert_eq!(find(&file, "scale").flags & flags::OVERRIDE, 0);
}

#[test]
fn java_methods_annotated_override_are_flagged() {
    let file = java(
        "class Square implements Shape {\n\
             @Override public double area() { return 1; }\n\
             public void scale() {}\n\
         }\n",
    );

    assert!(find(&file, "area").flags & flags::OVERRIDE != 0);
    assert_eq!(find(&file, "scale").flags & flags::OVERRIDE, 0);
}
