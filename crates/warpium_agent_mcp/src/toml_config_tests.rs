use super::*;

fn args() -> Vec<String> {
    vec!["mcp".to_owned()]
}

fn set(existing: Option<&str>, command: &str) -> (Vec<Change>, Option<String>) {
    let args = args();
    update_codex_servers(
        existing,
        &[(
            "warpium-logcat",
            Some(TomlServer {
                command,
                args: &args,
            }),
        )],
    )
    .unwrap()
}

#[test]
fn adds_a_server_table_to_an_empty_config() {
    let (changes, text) = set(None, "/bin/warpium-logcat");

    assert_eq!(changes, vec![Change::Added]);
    assert_eq!(
        text.unwrap(),
        "[mcp_servers.warpium-logcat]\ncommand = \"/bin/warpium-logcat\"\nargs = [\"mcp\"]\n"
    );
}

#[test]
fn keeps_comments_settings_and_other_servers() {
    let existing = "# my settings\nmodel = \"o3\"\n\n[mcp_servers.github]\ncommand = \"gh-mcp\" # keep\n";

    let (_, text) = set(Some(existing), "/bin/warpium-logcat");

    let text = text.unwrap();
    assert!(text.starts_with(existing), "{text}");
    assert!(text.contains("[mcp_servers.warpium-logcat]"), "{text}");
}

#[test]
fn installing_twice_changes_nothing_the_second_time() {
    let (_, first) = set(None, "/bin/warpium-logcat");

    let (changes, second) = set(first.as_deref(), "/bin/warpium-logcat");

    assert_eq!(changes, vec![Change::Unchanged]);
    assert_eq!(second, None);
}

#[test]
fn updating_keeps_extra_keys_of_the_server() {
    let existing =
        "[mcp_servers.warpium-logcat]\ncommand = \"/old\"\nargs = []\nstartup_timeout_sec = 20\n";

    let (changes, text) = set(Some(existing), "/new");

    let text = text.unwrap();
    assert_eq!(changes, vec![Change::Updated]);
    assert!(text.contains("command = \"/new\""), "{text}");
    assert!(text.contains("startup_timeout_sec = 20"), "{text}");
}

#[test]
fn removes_only_the_named_server() {
    let existing = "[mcp_servers.github]\ncommand = \"gh-mcp\"\n\n[mcp_servers.warpium-logcat]\ncommand = \"/x\"\n";

    let (changes, text) = update_codex_servers(
        Some(existing),
        &[("warpium-logcat", None), ("warpium-code-index", None)],
    )
    .unwrap();

    assert_eq!(changes, vec![Change::Removed, Change::Absent]);
    assert_eq!(text.unwrap(), "[mcp_servers.github]\ncommand = \"gh-mcp\"\n");
}

#[test]
fn removing_from_a_config_without_servers_writes_nothing() {
    let result = update_codex_servers(Some("model = \"o3\"\n"), &[("warpium-logcat", None)]);

    assert_eq!(result, Ok((vec![Change::Absent], None)));
}

#[test]
fn leaves_an_unreadable_config_alone() {
    let args = args();
    let server = || {
        Some(TomlServer {
            command: "/x",
            args: &args,
        })
    };

    assert!(update_codex_servers(Some("model = "), &[("warpium-logcat", server())]).is_err());
    assert!(update_codex_servers(Some("mcp_servers = 1\n"), &[("warpium-logcat", server())]).is_err());
}
