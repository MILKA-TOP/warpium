use super::*;

#[test]
fn safe_arguments_stay_unquoted() {
    assert_eq!(quote_arg(":app:test"), ":app:test");
    assert_eq!(quote_arg("-Dkey=value"), "-Dkey=value");
    assert_eq!(quote_arg("/work/app/run.sh"), "/work/app/run.sh");
}

#[test]
fn other_arguments_are_single_quoted() {
    assert_eq!(
        quote_arg("com.example.Test.should work"),
        "'com.example.Test.should work'"
    );
    assert_eq!(quote_arg("it's"), r"'it'\''s'");
    assert_eq!(quote_arg(""), "''");
    assert_eq!(quote_arg("pkg.*"), "'pkg.*'");
}

#[test]
fn split_args_groups_double_quoted_parts() {
    assert_eq!(
        split_args(r#"-Xmx2g  -Dname="two words" -Dq="a \"b\"""#),
        vec!["-Xmx2g", "-Dname=two words", r#"-Dq=a "b""#]
    );
    assert_eq!(split_args("  "), Vec::<String>::new());
    assert_eq!(split_args(r#""""#), vec![""]);
}

#[test]
fn join_command_skips_empty_parts() {
    assert_eq!(
        join_command(["./gradlew", "", " build ", "  "]),
        "./gradlew build"
    );
}

#[test]
fn toolbar_variables_are_escaped() {
    assert_eq!(
        escape_toolbar_variables("echo ${HOME} ${file} $PATH"),
        "echo $${HOME} $${file} $PATH"
    );
}

#[test]
fn env_names_must_be_shell_identifiers() {
    assert!(is_valid_env_name("JAVA_HOME"));
    assert!(is_valid_env_name("_x1"));
    assert!(!is_valid_env_name("1X"));
    assert!(!is_valid_env_name("A-B"));
    assert!(!is_valid_env_name(""));
    assert!(!is_valid_env_name("A;rm"));
}
