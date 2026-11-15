use super::*;
use crate::config::{ConfigSource, merge, parse_config};

fn buttons(text: &str) -> Vec<ToolbarButton> {
    let config = parse_config(text, ConfigSource::Team).unwrap();
    merge(Some(&config), None).unwrap()
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

    let ids = toolbar_buttons(&buttons)
        .into_iter()
        .map(|button| button.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["b"]);
}

#[test]
fn every_button_stays_inline_when_all_fit() {
    let fit = fit_inline_buttons(&[26., 26., 60.], 26., 100., 212.);

    assert_eq!(
        fit,
        InlineFit {
            visible: 3,
            overflow: false
        }
    );
}

#[test]
fn buttons_that_do_not_fit_move_to_the_overflow_menu() {
    let fit = fit_inline_buttons(&[26., 26., 26., 26.], 26., 100., 200.);

    assert_eq!(
        fit,
        InlineFit {
            visible: 2,
            overflow: true
        }
    );
}

#[test]
fn last_button_moves_to_the_overflow_menu_when_it_is_wider_than_the_menu_button() {
    let fit = fit_inline_buttons(&[26., 26., 80.], 26., 0., 120.);

    assert_eq!(
        fit,
        InlineFit {
            visible: 2,
            overflow: true
        }
    );
}

#[test]
fn a_wide_button_hides_the_narrow_ones_after_it_to_keep_the_order() {
    let fit = fit_inline_buttons(&[26., 200., 26.], 26., 0., 100.);

    assert_eq!(
        fit,
        InlineFit {
            visible: 1,
            overflow: true
        }
    );
}

#[test]
fn no_room_at_all_leaves_only_the_overflow_menu() {
    let fit = fit_inline_buttons(&[26., 26.], 26., 100., 110.);

    assert_eq!(
        fit,
        InlineFit {
            visible: 0,
            overflow: true
        }
    );
}

#[test]
fn rounding_noise_does_not_collapse_a_button() {
    let fit = fit_inline_buttons(&[26.2, 26.2], 26., 0., 52.);

    assert_eq!(
        fit,
        InlineFit {
            visible: 2,
            overflow: false
        }
    );
}

#[test]
fn unbounded_space_shows_every_button() {
    let fit = fit_inline_buttons(&[26.; 12], 26., 100., f32::INFINITY);

    assert_eq!(
        fit,
        InlineFit {
            visible: 12,
            overflow: false
        }
    );
}

#[test]
fn no_buttons_needs_no_overflow_menu() {
    let fit = fit_inline_buttons(&[], 26., 100., 50.);

    assert_eq!(
        fit,
        InlineFit {
            visible: 0,
            overflow: false
        }
    );
}
