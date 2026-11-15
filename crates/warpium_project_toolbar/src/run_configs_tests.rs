use super::*;
use crate::config::{ConfigSource, merge, parse_config};

fn buttons(text: &str) -> Vec<ToolbarButton> {
    let config = parse_config(text, ConfigSource::Team).unwrap();
    merge(Some(&config), None).unwrap()
}

fn url_buttons(count: usize) -> Vec<ToolbarButton> {
    let text = (0..count)
        .map(|index| format!("[[button]]\nid = \"b{index}\"\nurl = \"https://x/{index}\"\n"))
        .collect::<String>();
    buttons(&text)
}

fn strip_ids(strip: &ToolbarStrip<'_>) -> (Vec<String>, Vec<String>) {
    (
        strip
            .inline
            .iter()
            .map(|button| button.id.clone())
            .collect(),
        strip
            .overflow
            .iter()
            .map(|button| button.id.clone())
            .collect(),
    )
}

#[test]
fn run_config_buttons_come_first_then_project_launch_configs() {
    let buttons = buttons(
        "[[button]]\nid = \"assemble\"\nlabel = \"Assemble\"\ncommand = \"make\"\nrun_config = true\n[[button]]\nid = \"ci\"\nurl = \"https://ci\"\n",
    );

    let items = run_config_items(&buttons, &["Android dev".to_owned()]);

    assert_eq!(
        items,
        vec![
            RunConfigItem {
                key: "button:assemble".to_owned(),
                label: "Assemble".to_owned(),
                kind: RunConfigKind::Button("assemble".to_owned()),
            },
            RunConfigItem {
                key: "launch:Android dev".to_owned(),
                label: "Android dev".to_owned(),
                kind: RunConfigKind::LaunchConfig("Android dev".to_owned()),
            },
        ]
    );
}

#[test]
fn launch_config_already_opened_by_a_run_config_button_is_listed_once() {
    let buttons =
        buttons("[[button]]\nid = \"dev\"\nlaunch_config = \"Android dev\"\nrun_config = true\n");

    let items = run_config_items(
        &buttons,
        &["Android dev".to_owned(), "Android dev".to_owned()],
    );

    assert_eq!(
        items
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        vec!["button:dev"]
    );
}

#[test]
fn remembered_selection_is_used_while_it_exists() {
    let items = run_config_items(
        &buttons(
            "[[button]]\nid = \"a\"\ncommand = \"a\"\nrun_config = true\n[[button]]\nid = \"b\"\ncommand = \"b\"\nrun_config = true\n",
        ),
        &[],
    );

    assert_eq!(
        selected_run_config(&items, Some("button:b")).map(|item| item.key.as_str()),
        Some("button:b")
    );
    assert_eq!(
        selected_run_config(&items, Some("button:gone")).map(|item| item.key.as_str()),
        Some("button:a")
    );
    assert_eq!(
        selected_run_config(&items, None).map(|item| item.key.as_str()),
        Some("button:a")
    );
    assert_eq!(selected_run_config(&[], Some("button:a")), None);
}

#[test]
fn run_config_buttons_are_not_on_the_toolbar() {
    let buttons = buttons(
        "[[button]]\nid = \"a\"\ncommand = \"a\"\nrun_config = true\n[[button]]\nid = \"b\"\nurl = \"https://b\"\n",
    );

    let strip = toolbar_strip(&buttons);

    assert_eq!(strip_ids(&strip), (vec!["b".to_owned()], vec![]));
}

#[test]
fn up_to_one_more_than_the_limit_stays_inline() {
    let buttons = url_buttons(MAX_INLINE_BUTTONS + 1);

    let strip = toolbar_strip(&buttons);

    assert_eq!(strip.inline.len(), MAX_INLINE_BUTTONS + 1);
    assert!(strip.overflow.is_empty());
}

#[test]
fn buttons_past_the_limit_move_to_the_overflow_menu_in_order() {
    let buttons = url_buttons(MAX_INLINE_BUTTONS + 3);

    let (inline, overflow) = strip_ids(&toolbar_strip(&buttons));

    assert_eq!(inline, vec!["b0", "b1", "b2", "b3", "b4"]);
    assert_eq!(overflow, vec!["b5", "b6", "b7"]);
}
