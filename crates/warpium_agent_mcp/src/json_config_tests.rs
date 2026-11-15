use serde_json::json;

use super::*;

fn entry() -> Value {
    json!({
        "command": "/Applications/Warpium.app/Contents/MacOS/warpium-logcat",
        "args": ["mcp", "--root", "/work/app"],
    })
}

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn set(existing: Option<&str>) -> (Vec<Change>, Option<String>) {
    update_servers(existing, &[("warpium-logcat", Some(entry()))]).unwrap()
}

#[test]
fn creates_a_config_with_the_entry() {
    let (changes, text) = set(None);

    assert_eq!(changes, vec![Change::Added]);
    assert_eq!(
        parsed(&text.unwrap()),
        json!({ "mcpServers": { "warpium-logcat": entry() } })
    );
}

#[test]
fn keeps_other_servers_and_the_existing_wrapper() {
    let existing = r#"{"servers": {"github": {"command": "gh-mcp"}}, "other": 1}"#;

    let (_, text) = set(Some(existing));

    let config = parsed(&text.unwrap());
    assert_eq!(config["servers"]["github"], json!({ "command": "gh-mcp" }));
    assert_eq!(config["servers"]["warpium-logcat"], entry());
    assert_eq!(config["other"], json!(1));
    assert!(config.get("mcpServers").is_none());
}

#[test]
fn adds_to_a_bare_map_of_servers() {
    let existing = r#"{"github": {"command": "gh-mcp"}}"#;

    let (_, text) = set(Some(existing));

    assert_eq!(parsed(&text.unwrap())["warpium-logcat"], entry());
}

#[test]
fn installing_twice_changes_nothing_the_second_time() {
    let (_, first) = set(None);

    let (changes, second) = set(first.as_deref());

    assert_eq!(changes, vec![Change::Unchanged]);
    assert_eq!(second, None);
}

#[test]
fn replaces_a_stale_entry() {
    let existing = r#"{"mcpServers": {"warpium-logcat": {"command": "/old/path"}}}"#;

    let (changes, text) = set(Some(existing));

    assert_eq!(changes, vec![Change::Updated]);
    assert_eq!(
        parsed(&text.unwrap())["mcpServers"]["warpium-logcat"],
        entry()
    );
}

#[test]
fn removes_only_the_named_servers() {
    let existing =
        r#"{"mcpServers": {"github": {"command": "gh-mcp"}, "warpium-logcat": {"command": "x"}}}"#;

    let (changes, text) = update_servers(
        Some(existing),
        &[("warpium-logcat", None), ("warpium-code-index", None)],
    )
    .unwrap();

    assert_eq!(changes, vec![Change::Removed, Change::Absent]);
    assert_eq!(
        parsed(&text.unwrap()),
        json!({ "mcpServers": { "github": { "command": "gh-mcp" } } })
    );
}

#[test]
fn removing_from_a_config_without_the_entry_changes_nothing() {
    assert_eq!(
        update_servers(None, &[("warpium-logcat", None)]),
        Ok((vec![Change::Absent], None))
    );
    assert_eq!(
        update_servers(Some(r#"{"mcpServers": {}}"#), &[("warpium-logcat", None)]),
        Ok((vec![Change::Absent], None))
    );
}

#[test]
fn leaves_an_unreadable_config_alone() {
    assert!(update_servers(Some("{ not json"), &[("warpium-logcat", Some(entry()))]).is_err());
    assert!(update_servers(Some("[]"), &[("warpium-logcat", Some(entry()))]).is_err());
}

#[test]
fn reads_back_an_argument_of_a_server() {
    let (_, text) = set(None);

    assert_eq!(
        server_arg(&text.unwrap(), "warpium-logcat", "--root"),
        Some(PathBuf::from("/work/app"))
    );
}

#[test]
fn claude_local_scope_goes_under_the_project_and_keeps_everything_else() {
    let existing = r#"{
        "numStartups": 3,
        "projects": {
            "/work/app": {"allowedTools": [], "mcpServers": {"github": {"command": "gh"}}},
            "/work/other": {"mcpServers": {}}
        }
    }"#;

    let (changes, text) = update_claude_servers(
        Some(existing),
        Some(Path::new("/work/app")),
        &[("warpium-logcat", Some(entry()))],
    )
    .unwrap();

    let config = parsed(&text.unwrap());
    assert_eq!(changes, vec![Change::Added]);
    assert_eq!(config["numStartups"], json!(3));
    assert_eq!(config["projects"]["/work/app"]["allowedTools"], json!([]));
    assert_eq!(
        config["projects"]["/work/app"]["mcpServers"]["github"],
        json!({ "command": "gh" })
    );
    assert_eq!(
        config["projects"]["/work/app"]["mcpServers"]["warpium-logcat"],
        entry()
    );
    assert_eq!(
        config["projects"]["/work/other"],
        json!({ "mcpServers": {} })
    );
    assert!(config.get("mcpServers").is_none());
}

#[test]
fn claude_user_scope_without_a_project() {
    let (_, text) =
        update_claude_servers(None, None, &[("warpium-logcat", Some(entry()))]).unwrap();
    let text = text.unwrap();

    assert_eq!(claude_server(&text, None, "warpium-logcat"), Some(entry()));
    assert_eq!(
        claude_server(&text, Some(Path::new("/work/app")), "warpium-logcat"),
        None
    );
}

#[test]
fn claude_removal_from_a_project_without_servers_writes_nothing() {
    let existing = r#"{"projects": {"/work/app": {"allowedTools": []}}}"#;

    let result = update_claude_servers(
        Some(existing),
        Some(Path::new("/work/app")),
        &[("warpium-logcat", None)],
    );

    assert_eq!(result, Ok((vec![Change::Absent], None)));
}
