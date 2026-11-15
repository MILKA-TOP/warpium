use std::collections::BTreeMap;
use std::path::PathBuf;

use super::*;
use crate::template::{PERSONAL_TEMPLATE, TEAM_TEMPLATE};

fn team(text: &str) -> ParsedConfig {
    parse_config(text, ConfigSource::Team).expect("team config should parse")
}

fn personal(text: &str) -> ParsedConfig {
    parse_config(text, ConfigSource::Personal).expect("personal config should parse")
}

fn team_error(text: &str) -> String {
    parse_config(text, ConfigSource::Team)
        .expect_err("team config should be rejected")
        .message
}

fn ids(buttons: &[ToolbarButton]) -> Vec<&str> {
    buttons.iter().map(|button| button.id.as_str()).collect()
}

/// The template with every example line after the first `# [[button]]` uncommented.
fn uncommented_examples(template: &str) -> String {
    let mut in_examples = false;
    template
        .lines()
        .map(|line| {
            in_examples |= line == "# [[button]]";
            if in_examples {
                line.strip_prefix("# ").unwrap_or(line).to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn parses_a_command_button_with_all_options() {
    let config = team(
        r#"
[[button]]
id = "assemble"
label = "Assemble"
icon = "build"
tooltip = "Build the debug APK"
command = "./gradlew assembleDebug"
target = "split_right"
cwd = "app"
env = { GRADLE_OPTS = "-Xmx4g" }
run_config = true
"#,
    );

    let buttons = merge(Some(&config), None).unwrap();

    assert_eq!(
        buttons,
        vec![ToolbarButton {
            id: "assemble".to_owned(),
            label: Some("Assemble".to_owned()),
            icon: Some(ButtonIcon::Bundled {
                name: "build".to_owned(),
                asset_path: "bundled/svg/tool-02.svg",
            }),
            tooltip: Some("Build the debug APK".to_owned()),
            run_config: true,
            action: ButtonAction::Command(CommandAction {
                command: "./gradlew assembleDebug".to_owned(),
                target: CommandTarget::SplitRight,
                cwd: Some("app".to_owned()),
                env: BTreeMap::from([("GRADLE_OPTS".to_owned(), "-Xmx4g".to_owned())]),
            }),
            action_source: ConfigSource::Team,
        }]
    );
}

#[test]
fn command_defaults_to_the_current_session_and_the_project_root() {
    let buttons = merge(
        Some(&team("[[button]]\nid = \"t\"\ncommand = \"make\"\n")),
        None,
    )
    .unwrap();

    assert_eq!(
        buttons[0].action,
        ButtonAction::Command(CommandAction {
            command: "make".to_owned(),
            target: CommandTarget::Current,
            cwd: None,
            env: BTreeMap::new(),
        })
    );
}

#[test]
fn parses_every_non_command_action_kind() {
    let config = team(
        r#"
[[button]]
id = "w"
workflow = "Deploy"
[[button]]
id = "l"
launch_config = "Android"
[[button]]
id = "t"
tab_config = "android-dev"
[[button]]
id = "u"
url = "https://example.com/ci"
[[button]]
id = "a"
action = "surface.project_explorer.open"
"#,
    );

    let actions = merge(Some(&config), None)
        .unwrap()
        .into_iter()
        .map(|button| button.action)
        .collect::<Vec<_>>();

    assert_eq!(
        actions,
        vec![
            ButtonAction::Workflow("Deploy".to_owned()),
            ButtonAction::LaunchConfig("Android".to_owned()),
            ButtonAction::TabConfig("android-dev".to_owned()),
            ButtonAction::Url("https://example.com/ci".to_owned()),
            ButtonAction::Builtin(BuiltinAction::SurfaceProjectExplorerOpen),
        ]
    );
}

#[test]
fn custom_svg_icon_is_kept_relative_to_the_warpium_directory() {
    let buttons = merge(
        Some(&team(
            "[[button]]\nid = \"x\"\nicon = \"icons/android.svg\"\ncommand = \"x\"\n",
        )),
        None,
    )
    .unwrap();

    assert_eq!(
        buttons[0].icon,
        Some(ButtonIcon::Custom(PathBuf::from("icons/android.svg")))
    );
}

#[test]
fn empty_file_has_no_buttons() {
    assert_eq!(merge(Some(&team("")), None).unwrap(), vec![]);
}

#[test]
fn templates_have_no_active_buttons() {
    assert!(team(TEAM_TEMPLATE).is_empty());
    assert!(personal(PERSONAL_TEMPLATE).is_empty());
}

#[test]
fn team_template_examples_parse_when_uncommented() {
    let config = team(&uncommented_examples(TEAM_TEMPLATE));

    let buttons = merge(Some(&config), None).unwrap();

    assert_eq!(
        ids(&buttons),
        vec!["assemble", "test", "deploy", "dev-layout", "ci", "explorer"]
    );
}

#[test]
fn personal_template_examples_merge_into_the_team_examples_when_uncommented() {
    let team_config = team(&uncommented_examples(TEAM_TEMPLATE));
    let personal_config = personal(&uncommented_examples(PERSONAL_TEMPLATE));

    let buttons = merge(Some(&team_config), Some(&personal_config)).unwrap();

    assert_eq!(
        ids(&buttons),
        vec![
            "assemble",
            "test",
            "deploy",
            "dev-layout",
            "explorer",
            "my-emulator"
        ]
    );
}

#[test]
fn reports_toml_syntax_errors_with_their_location() {
    let message = team_error("[[button]\nid = \"x\"\n");

    assert!(message.contains("line 1"), "message: {message}");
}

#[test]
fn rejects_unknown_keys() {
    let message = team_error("[[button]]\nid = \"x\"\ncomand = \"make\"\n");

    assert!(message.contains("comand"), "message: {message}");
}

#[test]
fn rejects_a_button_without_an_id() {
    assert_eq!(
        team_error("[[button]]\ncommand = \"make\"\n"),
        "button 1: every button needs a non-empty `id`"
    );
}

#[test]
fn rejects_duplicate_ids() {
    assert_eq!(
        team_error(
            "[[button]]\nid = \"x\"\nurl = \"https://a\"\n[[button]]\nid = \"x\"\nurl = \"https://b\"\n"
        ),
        "button id `x` is used more than once"
    );
}

#[test]
fn rejects_a_team_button_without_an_action() {
    assert_eq!(
        team_error("[[button]]\nid = \"x\"\nlabel = \"X\"\n"),
        "button `x` has no action"
    );
}

#[test]
fn rejects_more_than_one_action() {
    let message = team_error("[[button]]\nid = \"x\"\ncommand = \"make\"\nurl = \"https://a\"\n");

    assert!(
        message.starts_with("button 1: `x`: set only one of"),
        "message: {message}"
    );
}

#[test]
fn rejects_an_unknown_target() {
    let message = team_error("[[button]]\nid = \"x\"\ncommand = \"make\"\ntarget = \"window\"\n");

    assert!(
        message.contains("unknown target `window`"),
        "message: {message}"
    );
}

#[test]
fn rejects_a_cwd_outside_the_project() {
    for cwd in ["../other", "/tmp", "app/../../other"] {
        let message = team_error(&format!(
            "[[button]]\nid = \"x\"\ncommand = \"make\"\ncwd = \"{cwd}\"\n"
        ));

        assert!(
            message.contains("`cwd` must be relative"),
            "cwd {cwd}: {message}"
        );
    }
}

#[test]
fn accepts_a_cwd_that_stays_inside_the_project() {
    let config = team("[[button]]\nid = \"x\"\ncommand = \"make\"\ncwd = \"app/../lib\"\n");

    assert!(merge(Some(&config), None).is_ok());
}

#[test]
fn rejects_invalid_env_names() {
    let message =
        team_error("[[button]]\nid = \"x\"\ncommand = \"make\"\nenv = { \"MY-VAR\" = \"1\" }\n");

    assert!(
        message.contains("`MY-VAR` is not a valid"),
        "message: {message}"
    );
}

#[test]
fn rejects_command_options_on_other_actions() {
    let message = team_error("[[button]]\nid = \"x\"\nurl = \"https://a\"\ntarget = \"new_tab\"\n");

    assert!(
        message.contains("only apply to `command` buttons"),
        "message: {message}"
    );
}

#[test]
fn rejects_non_http_urls() {
    let message = team_error("[[button]]\nid = \"x\"\nurl = \"file:///etc/passwd\"\n");

    assert!(
        message.contains("must start with https://"),
        "message: {message}"
    );
}

#[test]
fn rejects_unknown_builtin_actions() {
    let message = team_error("[[button]]\nid = \"x\"\naction = \"pane.close\"\n");

    assert!(
        message.contains("unknown action `pane.close`"),
        "message: {message}"
    );
}

#[test]
fn rejects_unknown_icons_and_escaping_svg_paths() {
    let unknown = team_error("[[button]]\nid = \"x\"\nicon = \"hammer\"\ncommand = \"x\"\n");
    let escaping = team_error("[[button]]\nid = \"x\"\nicon = \"../logo.svg\"\ncommand = \"x\"\n");

    assert!(
        unknown.contains("unknown icon `hammer`"),
        "message: {unknown}"
    );
    assert!(
        escaping.contains("must be relative to `.warpium/`"),
        "message: {escaping}"
    );
}

#[test]
fn error_display_names_the_file() {
    let error = parse_config(
        "[[button]]\nid = \"x\"\nurl = \"ftp://x\"\n",
        ConfigSource::Personal,
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Invalid personal toolbar overrides: button 1: `x`: `url` must start with https:// or \
         http://, got `ftp://x`"
    );
}

#[test]
fn override_of_a_removed_team_button_is_ignored() {
    let team_config = team("[[button]]\nid = \"test\"\ncommand = \"make test\"\n");
    let personal_config = personal(
        "[[button]]\nid = \"assemble\"\ntarget = \"split_right\"\n[[button]]\nid = \"ci\"\nhidden = true\n",
    );

    let buttons = merge(Some(&team_config), Some(&personal_config)).unwrap();

    assert_eq!(ids(&buttons), vec!["test"]);
}

#[test]
fn override_changes_only_the_fields_it_sets() {
    let team_config =
        team("[[button]]\nid = \"b\"\nlabel = \"Build\"\nicon = \"build\"\ncommand = \"make\"\n");
    let personal_config = personal("[[button]]\nid = \"b\"\nlabel = \"My build\"\n");

    let buttons = merge(Some(&team_config), Some(&personal_config)).unwrap();

    assert_eq!(buttons[0].label.as_deref(), Some("My build"));
    assert_eq!(buttons[0].icon, team_config_icon("build"));
    assert_eq!(buttons[0].action_source, ConfigSource::Team);
}

fn team_config_icon(name: &str) -> Option<ButtonIcon> {
    crate::icons::parse_icon(name).ok()
}

#[test]
fn override_of_command_options_keeps_the_team_command_and_its_trust_requirement() {
    let team_config =
        team("[[button]]\nid = \"b\"\ncommand = \"make\"\ntarget = \"new_tab\"\ncwd = \"app\"\n");
    let personal_config = personal("[[button]]\nid = \"b\"\ntarget = \"split_down\"\n");

    let buttons = merge(Some(&team_config), Some(&personal_config)).unwrap();

    assert_eq!(
        buttons[0].action,
        ButtonAction::Command(CommandAction {
            command: "make".to_owned(),
            target: CommandTarget::SplitDown,
            cwd: Some("app".to_owned()),
            env: BTreeMap::new(),
        })
    );
    assert!(buttons[0].requires_trust());
}

#[test]
fn override_with_an_action_replaces_the_whole_action_and_is_trusted() {
    let team_config =
        team("[[button]]\nid = \"b\"\ncommand = \"make\"\ntarget = \"new_tab\"\ncwd = \"app\"\n");
    let personal_config = personal("[[button]]\nid = \"b\"\ncommand = \"make -j8\"\n");

    let buttons = merge(Some(&team_config), Some(&personal_config)).unwrap();

    assert_eq!(
        buttons[0].action,
        ButtonAction::Command(CommandAction {
            command: "make -j8".to_owned(),
            target: CommandTarget::Current,
            cwd: None,
            env: BTreeMap::new(),
        })
    );
    assert_eq!(buttons[0].action_source, ConfigSource::Personal);
    assert!(!buttons[0].requires_trust());
}

#[test]
fn override_can_switch_a_command_to_another_action_kind() {
    let team_config = team("[[button]]\nid = \"b\"\ncommand = \"make\"\ntarget = \"new_tab\"\n");
    let personal_config = personal("[[button]]\nid = \"b\"\nurl = \"https://example.com\"\n");

    let buttons = merge(Some(&team_config), Some(&personal_config)).unwrap();

    assert_eq!(
        buttons[0].action,
        ButtonAction::Url("https://example.com".to_owned())
    );
}

#[test]
fn hidden_override_removes_the_team_button() {
    let team_config = team(
        "[[button]]\nid = \"a\"\nurl = \"https://a\"\n[[button]]\nid = \"b\"\nurl = \"https://b\"\n",
    );
    let personal_config = personal("[[button]]\nid = \"a\"\nhidden = true\n");

    let buttons = merge(Some(&team_config), Some(&personal_config)).unwrap();

    assert_eq!(ids(&buttons), vec!["b"]);
}

#[test]
fn new_personal_buttons_follow_the_team_buttons() {
    let team_config = team("[[button]]\nid = \"a\"\nurl = \"https://a\"\n");
    let personal_config = personal(
        "[[button]]\nid = \"z\"\ncommand = \"echo z\"\n[[button]]\nid = \"y\"\nurl = \"https://y\"\n",
    );

    let buttons = merge(Some(&team_config), Some(&personal_config)).unwrap();

    assert_eq!(ids(&buttons), vec!["a", "z", "y"]);
    assert!(!buttons[1].requires_trust());
}

#[test]
fn personal_buttons_work_without_a_team_file() {
    let personal_config = personal("[[button]]\nid = \"z\"\ncommand = \"echo z\"\n");

    let buttons = merge(None, Some(&personal_config)).unwrap();

    assert_eq!(ids(&buttons), vec!["z"]);
}

#[test]
fn hidden_personal_button_with_an_unknown_id_is_ignored() {
    let personal_config = personal("[[button]]\nid = \"nope\"\nhidden = true\n");

    assert_eq!(merge(None, Some(&personal_config)).unwrap(), vec![]);
}

#[test]
fn override_setting_command_options_on_a_non_command_button_is_an_error() {
    let team_config = team("[[button]]\nid = \"b\"\nurl = \"https://b\"\n");
    let personal_config = personal("[[button]]\nid = \"b\"\ntarget = \"new_tab\"\n");

    let error = merge(Some(&team_config), Some(&personal_config)).unwrap_err();

    assert!(
        error.message.contains("only apply to `command` buttons"),
        "message: {}",
        error.message
    );
}

#[test]
fn tooltip_falls_back_to_the_label_then_the_action() {
    let config = team(
        "[[button]]\nid = \"a\"\nlabel = \"Build\"\ncommand = \"make\"\n[[button]]\nid = \"b\"\ncommand = \"make test\"\n[[button]]\nid = \"c\"\nworkflow = \"Deploy\"\n",
    );

    let tooltips = merge(Some(&config), None)
        .unwrap()
        .iter()
        .map(ToolbarButton::tooltip_text)
        .collect::<Vec<_>>();

    assert_eq!(
        tooltips,
        vec!["Build", "make test", "Run workflow \"Deploy\""]
    );
}

#[test]
fn only_team_commands_require_trust() {
    let config = team(
        "[[button]]\nid = \"c\"\ncommand = \"make\"\n[[button]]\nid = \"u\"\nurl = \"https://u\"\n[[button]]\nid = \"w\"\nworkflow = \"W\"\n",
    );

    let requires_trust = merge(Some(&config), None)
        .unwrap()
        .iter()
        .map(ToolbarButton::requires_trust)
        .collect::<Vec<_>>();

    assert_eq!(requires_trust, vec![true, false, false]);
}
