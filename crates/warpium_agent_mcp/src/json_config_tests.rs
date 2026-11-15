use super::*;

fn entry() -> Value {
    server_entry(
        Path::new("/Applications/Warpium.app/Contents/MacOS/warpium-logcat"),
        Some(Path::new("/work/app")),
    )
}

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn creates_a_config_with_the_entry() {
    let text = update_mcp_config(None, Some(&entry())).unwrap().unwrap();

    assert_eq!(
        parsed(&text),
        json!({
            "mcpServers": {
                "warpium-logcat": {
                    "command": "/Applications/Warpium.app/Contents/MacOS/warpium-logcat",
                    "args": ["mcp", "--root", "/work/app"],
                },
            },
        })
    );
}

#[test]
fn keeps_other_servers_and_the_existing_wrapper() {
    let existing = r#"{"servers": {"github": {"command": "gh-mcp"}}}"#;

    let text = update_mcp_config(Some(existing), Some(&entry()))
        .unwrap()
        .unwrap();

    let config = parsed(&text);
    assert_eq!(config["servers"]["github"], json!({ "command": "gh-mcp" }));
    assert_eq!(config["servers"]["warpium-logcat"], entry());
    assert!(config.get("mcpServers").is_none());
}

#[test]
fn adds_to_a_bare_map_of_servers() {
    let existing = r#"{"github": {"command": "gh-mcp"}}"#;

    let text = update_mcp_config(Some(existing), Some(&entry()))
        .unwrap()
        .unwrap();

    assert_eq!(parsed(&text)["warpium-logcat"], entry());
}

#[test]
fn reports_no_change_when_the_entry_is_already_there() {
    let existing = update_mcp_config(None, Some(&entry())).unwrap().unwrap();

    assert_eq!(update_mcp_config(Some(&existing), Some(&entry())), Ok(None));
}

#[test]
fn removes_only_this_server() {
    let existing = r#"{"mcpServers": {"github": {"command": "gh-mcp"}, "warpium-logcat": {"command": "x"}}}"#;

    let text = update_mcp_config(Some(existing), None).unwrap().unwrap();

    assert_eq!(
        parsed(&text),
        json!({ "mcpServers": { "github": { "command": "gh-mcp" } } })
    );
}

#[test]
fn removing_from_a_config_without_the_entry_changes_nothing() {
    assert_eq!(update_mcp_config(None, None), Ok(None));
    assert_eq!(
        update_mcp_config(Some(r#"{"mcpServers": {}}"#), None),
        Ok(None)
    );
}

#[test]
fn leaves_an_unreadable_config_alone() {
    assert!(update_mcp_config(Some("{ not json"), Some(&entry())).is_err());
    assert!(update_mcp_config(Some("[]"), Some(&entry())).is_err());
}

#[test]
fn reads_back_the_registered_root() {
    let existing = update_mcp_config(None, Some(&entry())).unwrap().unwrap();

    assert_eq!(registered_root(&existing), Some(PathBuf::from("/work/app")));
}

#[test]
fn claude_command_quotes_paths_with_spaces() {
    let command = claude_mcp_add_command(
        Path::new("/Applications/My Apps/Warpium.app/Contents/MacOS/warpium-logcat"),
        Some(Path::new("/work/app")),
    );

    assert_eq!(
        command,
        "claude mcp add warpium-logcat -- '/Applications/My Apps/Warpium.app/Contents/MacOS/warpium-logcat' mcp --root /work/app"
    );
}
