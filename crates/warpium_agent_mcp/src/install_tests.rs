use std::fs;

use serde_json::json;
use tempfile::TempDir;

use super::*;

struct Fixture {
    _dir: TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("work/app");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&project).unwrap();
        Self {
            _dir: dir,
            home,
            project,
        }
    }

    /// No CLI on the search path, so only config directories count.
    fn env(&self, project: bool) -> Environment {
        Environment {
            home: self.home.clone(),
            project_root: project.then(|| self.project.clone()),
            codex_home: None,
            kimi_home: None,
            search_path: Vec::new(),
        }
    }

    fn read(&self, path: impl AsRef<Path>) -> String {
        fs::read_to_string(path).unwrap()
    }
}

fn servers() -> Vec<McpServer> {
    vec![
        McpServer {
            name: "warpium-logcat".to_owned(),
            command: PathBuf::from("/Apps/Warpium.app/Contents/MacOS/warpium-logcat"),
            args: vec!["mcp".to_owned()],
            project_args: vec!["--root".to_owned(), "/work/app".to_owned()],
        },
        McpServer {
            name: "warpium-code-index".to_owned(),
            command: PathBuf::from("/Apps/Warpium.app/Contents/MacOS/warpium-code-index"),
            args: vec!["mcp".to_owned()],
            project_args: vec!["--root".to_owned(), "/work/app".to_owned()],
        },
    ]
}

fn all(change: Change) -> Result<Vec<(String, Change)>, String> {
    Ok(vec![
        ("warpium-logcat".to_owned(), change),
        ("warpium-code-index".to_owned(), change),
    ])
}

#[test]
fn finds_no_agent_on_a_bare_machine() {
    let fixture = Fixture::new();

    assert_eq!(install(&fixture.env(true), &servers()), Vec::new());
    assert!(summary(&[], true).starts_with("No coding agent found"));
}

#[test]
fn an_arc_project_gets_the_servers_in_its_private_layer_only() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.project.join(".arc")).unwrap();
    fs::create_dir_all(fixture.home.join(".codex")).unwrap();
    fs::write(fixture.home.join(".claude.json"), "{}").unwrap();
    let private = fixture.project.join(".arc/private/mcp.json");
    fs::create_dir_all(private.parent().unwrap()).unwrap();
    fs::write(&private, r#"{"mcpServers": {"github": {"command": "gh"}}}"#).unwrap();

    let reports = install(&fixture.env(true), &servers());

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].agent, "arc");
    assert_eq!(reports[0].result, all(Change::Added));
    let config: Value = serde_json::from_str(&fixture.read(&private)).unwrap();
    assert_eq!(config["mcpServers"]["github"], json!({ "command": "gh" }));
    assert_eq!(
        config["mcpServers"]["warpium-logcat"],
        json!({
            "command": "/Apps/Warpium.app/Contents/MacOS/warpium-logcat",
            "args": ["mcp", "--root", "/work/app"],
        })
    );
    assert_eq!(fixture.read(fixture.home.join(".claude.json")), "{}");
    assert!(!fixture.home.join(".codex/config.toml").exists());
}

#[test]
fn installs_into_every_agent_found_and_reinstalling_changes_nothing() {
    let fixture = Fixture::new();
    fs::write(
        fixture.home.join(".claude.json"),
        r#"{"numStartups": 1, "projects": {}}"#,
    )
    .unwrap();
    fs::create_dir_all(fixture.home.join(".codex")).unwrap();
    fs::create_dir_all(fixture.home.join(".kimi-code")).unwrap();
    let env = fixture.env(true);

    let first = install(&env, &servers());
    let second = install(&env, &servers());

    let agents: Vec<&str> = first.iter().map(|report| report.agent).collect();
    assert_eq!(agents, vec!["Claude Code", "Codex", "Kimi Code"]);
    assert!(first.iter().all(|report| report.result == all(Change::Added)));
    assert!(second.iter().all(|report| report.result == all(Change::Unchanged)));

    let claude: Value =
        serde_json::from_str(&fixture.read(fixture.home.join(".claude.json"))).unwrap();
    let project_key = fixture.project.to_string_lossy();
    assert_eq!(claude["numStartups"], json!(1));
    assert_eq!(
        claude["projects"][project_key.as_ref()]["mcpServers"]["warpium-logcat"]["args"],
        json!(["mcp", "--root", "/work/app"])
    );
    let codex = fixture.read(fixture.home.join(".codex/config.toml"));
    assert!(codex.contains("[mcp_servers.warpium-code-index]"), "{codex}");
    assert!(codex.contains("args = [\"mcp\"]"), "shared configs leave out --root: {codex}");
    let kimi: Value =
        serde_json::from_str(&fixture.read(fixture.home.join(".kimi-code/mcp.json"))).unwrap();
    assert_eq!(kimi["mcpServers"]["warpium-logcat"]["args"], json!(["mcp"]));
    assert!(summary(&second, true).contains("Codex: already installed in ~/.codex/config.toml"));
}

#[test]
fn honors_codex_and_kimi_homes_and_the_legacy_kimi_dir() {
    let fixture = Fixture::new();
    let codex_home = fixture.home.join("custom-codex");
    fs::create_dir_all(&codex_home).unwrap();
    fs::create_dir_all(fixture.home.join(".kimi")).unwrap();
    let mut env = fixture.env(false);
    env.codex_home = Some(codex_home.clone());

    let reports = install(&env, &servers());

    assert_eq!(reports.len(), 2);
    assert!(codex_home.join("config.toml").is_file());
    assert!(fixture.home.join(".kimi/mcp.json").is_file());
}

#[test]
fn claude_without_a_project_uses_the_user_scope() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.home.join(".claude")).unwrap();

    let reports = install(&fixture.env(false), &servers());

    assert_eq!(reports[0].location, "~/.claude.json (user scope)");
    let claude: Value =
        serde_json::from_str(&fixture.read(fixture.home.join(".claude.json"))).unwrap();
    assert_eq!(
        claude["mcpServers"]["warpium-logcat"],
        json!({
            "type": "stdio",
            "command": "/Apps/Warpium.app/Contents/MacOS/warpium-logcat",
            "args": ["mcp"],
            "env": {},
        })
    );
}

#[test]
fn uninstall_removes_only_warpium_servers_from_existing_configs() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.home.join(".codex")).unwrap();
    fs::create_dir_all(fixture.home.join(".kimi-code")).unwrap();
    fs::write(
        fixture.home.join(".kimi-code/mcp.json"),
        r#"{"mcpServers": {"github": {"command": "gh"}}}"#,
    )
    .unwrap();
    let env = fixture.env(true);
    install(&env, &servers());

    let removed = uninstall(&env, &servers());
    let again = uninstall(&env, &servers());

    let agents: Vec<&str> = removed.iter().map(|report| report.agent).collect();
    assert_eq!(agents, vec!["Codex", "Kimi Code"]);
    assert!(removed.iter().all(|report| report.result == all(Change::Removed)));
    assert_eq!(again, Vec::new());
    let kimi: Value =
        serde_json::from_str(&fixture.read(fixture.home.join(".kimi-code/mcp.json"))).unwrap();
    assert_eq!(kimi, json!({ "mcpServers": { "github": { "command": "gh" } } }));
    assert!(!fixture.home.join(".claude.json").exists());
}

#[test]
fn a_broken_config_is_reported_and_left_alone() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.home.join(".codex")).unwrap();
    fs::write(fixture.home.join(".codex/config.toml"), "model = ").unwrap();

    let reports = install(&fixture.env(true), &servers());

    assert!(reports[0].result.is_err());
    assert_eq!(fixture.read(fixture.home.join(".codex/config.toml")), "model = ");
    assert!(summary(&reports, true).starts_with("Codex: failed"));
}

#[cfg(unix)]
#[test]
fn rewriting_keeps_the_file_permissions() {
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = Fixture::new();
    let config = fixture.home.join(".claude.json");
    fs::write(&config, "{}").unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();

    install(&fixture.env(true), &servers());

    let mode = fs::metadata(&config).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
