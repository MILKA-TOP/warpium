use super::*;

fn values() -> VariableValues {
    VariableValues {
        project_root: PathBuf::from("/work/my app"),
        file: Some(PathBuf::from("/work/my app/src/it's.kt")),
        selection: Some("hello world".to_owned()),
        cwd: Some(PathBuf::from("/work/my app/sub")),
        prompts: BTreeMap::from([("Tag name".to_owned(), "v1; rm -rf /".to_owned())]),
    }
}

#[test]
fn expands_each_variable_as_one_quoted_posix_word() {
    let expanded = expand_variables(
        "lint ${file} --in ${fileDir} --root ${projectRoot} --cwd ${cwd} -m ${selection}",
        &values(),
        ShellFamily::Posix,
    )
    .unwrap();

    assert_eq!(
        expanded,
        r"lint '/work/my app/src/it'\''s.kt' --in '/work/my app/src' --root '/work/my app' --cwd '/work/my app/sub' -m 'hello world'"
    );
}

#[test]
fn quotes_for_fish_and_powershell() {
    assert_eq!(
        expand_variables("cat ${file}", &values(), ShellFamily::Fish).unwrap(),
        r"cat '/work/my app/src/it\'s.kt'"
    );
    assert_eq!(
        expand_variables("cat ${file}", &values(), ShellFamily::PowerShell).unwrap(),
        "cat '/work/my app/src/it''s.kt'"
    );
}

#[test]
fn prompt_answers_are_quoted_so_they_cannot_inject_commands() {
    let expanded = expand_variables(
        "git tag ${prompt:Tag name} && git push origin ${prompt:Tag name}",
        &values(),
        ShellFamily::Posix,
    )
    .unwrap();

    assert_eq!(
        expanded,
        "git tag 'v1; rm -rf /' && git push origin 'v1; rm -rf /'"
    );
}

#[test]
fn prompt_labels_are_listed_once_in_order() {
    assert_eq!(
        prompt_labels("a ${prompt:Second} ${file} ${prompt:First} ${prompt:Second}"),
        vec!["Second".to_owned(), "First".to_owned()]
    );
    assert!(prompt_labels("make build").is_empty());
}

#[test]
fn cwd_falls_back_to_the_project_root() {
    let values = VariableValues {
        cwd: None,
        ..values()
    };

    assert_eq!(
        expand_variables("ls ${cwd}", &values, ShellFamily::Posix).unwrap(),
        "ls '/work/my app'"
    );
}

#[test]
fn reports_missing_values_and_unknown_variables() {
    let empty = VariableValues {
        project_root: PathBuf::from("/p"),
        ..VariableValues::default()
    };
    assert_eq!(
        expand_variables("cat ${file}", &empty, ShellFamily::Posix),
        Err(VariableError::NoFile)
    );
    assert_eq!(
        expand_variables("echo ${selection}", &empty, ShellFamily::Posix),
        Err(VariableError::NoSelection)
    );
    assert_eq!(
        expand_variables("echo ${prompt:Name}", &empty, ShellFamily::Posix),
        Err(VariableError::MissingPrompt("Name".to_owned()))
    );
    assert_eq!(
        expand_variables("echo ${home}", &empty, ShellFamily::Posix),
        Err(VariableError::Unknown("home".to_owned()))
    );
    assert_eq!(
        validate_variables("echo ${file"),
        Err(VariableError::Unterminated)
    );
    assert_eq!(validate_variables("echo ${file} ${prompt:X}"), Ok(()));
}

#[test]
fn escaped_and_plain_dollars_stay_literal() {
    assert_eq!(
        expand_variables(
            "echo $${file} $HOME ${projectRoot}",
            &values(),
            ShellFamily::Posix
        )
        .unwrap(),
        "echo ${file} $HOME '/work/my app'"
    );
    assert!(!has_variables("echo $HOME"));
}

#[test]
fn shell_family_follows_the_program_name() {
    assert_eq!(
        ShellFamily::from_program("/opt/homebrew/bin/fish"),
        ShellFamily::Fish
    );
    assert_eq!(
        ShellFamily::from_program("/usr/local/bin/pwsh"),
        ShellFamily::PowerShell
    );
    assert_eq!(ShellFamily::from_program("/bin/zsh"), ShellFamily::Posix);
    assert_eq!(ShellFamily::from_program(""), ShellFamily::Posix);
}
