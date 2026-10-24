mod common;

use std::path::Path;

use common::TestRepo;
use serde_json::Value;

fn assert_orbit_command(value: &Value) {
    let command = value.as_str().expect("command is a string");
    assert!(
        Path::new(command).is_absolute(),
        "{command} is not an absolute path"
    );
    let name = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str());
    assert!(
        name == Some("orbit") || name == Some("orbit.exe"),
        "unexpected command {command}"
    );
}

fn assert_orbit_toml_command(value: &toml::Value) {
    assert_orbit_command(&Value::String(
        value.as_str().expect("command is a string").to_string(),
    ));
}

#[test]
fn setup_claude_preserves_settings_and_is_idempotent() {
    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::write(
        repo.path().join(".claude/settings.json"),
        "{\n  \"permissions\": {\n    \"allow\": [\"Bash(git status)\"]\n  }\n}\n",
    )
    .unwrap();

    repo.ok_json(&["setup", "claude", "--hooks", "--json"]);

    let instructions = repo.read("CLAUDE.md");
    assert!(instructions.contains("BEGIN ORBIT INTEGRATION"));
    assert!(instructions.contains("orbit ready --claim --by <agent>"));
    assert!(instructions.contains("never edit task files"));

    let settings = repo.read_json(".claude/settings.json");
    assert_eq!(settings["permissions"]["allow"][0], "Bash(git status)");
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "orbit prime --hook-json --by claude"
    );

    repo.ok_json(&["setup", "claude", "--hooks", "--json"]);
    let instructions = repo.read("CLAUDE.md");
    assert_eq!(instructions.matches("BEGIN ORBIT INTEGRATION").count(), 1);
    let settings = repo.read_json(".claude/settings.json");
    assert_eq!(
        settings["hooks"]["SessionStart"].as_array().unwrap().len(),
        1
    );

    repo.ok_json(&["setup", "claude", "--remove", "--json"]);
    assert!(!repo.exists("CLAUDE.md"));
    let settings = repo.read_json(".claude/settings.json");
    assert_eq!(settings["permissions"]["allow"][0], "Bash(git status)");
    assert!(settings.get("hooks").is_none());
}

#[test]
fn setup_cursor_writes_and_removes_rule_and_skill() {
    let repo = TestRepo::new();
    repo.ok_json(&["setup", "cursor", "--json"]);

    let rule = repo.read(".cursor/rules/orbit.mdc");
    assert!(rule.starts_with("---\n"));
    assert!(rule.contains("alwaysApply: true"));
    assert!(rule.contains("BEGIN ORBIT INTEGRATION"));
    assert!(rule.contains("orbit ready --claim"));

    let skill = repo.read(".agents/skills/orbit/SKILL.md");
    assert!(skill.starts_with("---\nname: orbit\n"));
    assert!(skill.contains("BEGIN ORBIT INTEGRATION"));

    repo.ok_json(&["setup", "cursor", "--remove", "--json"]);
    assert!(!repo.exists(".cursor/rules/orbit.mdc"));
    assert!(!repo.exists(".agents/skills/orbit/SKILL.md"));
}

#[test]
fn setup_codex_and_opencode_share_agents_md() {
    let repo = TestRepo::new();
    std::fs::write(repo.path().join("AGENTS.md"), "# Project\n\nKeep this.\n").unwrap();

    repo.ok_json(&["setup", "codex", "--json"]);
    assert!(repo.exists(".agents/skills/orbit/SKILL.md"));
    let agents = repo.read("AGENTS.md");
    assert!(agents.contains("Keep this."));
    assert!(agents.contains("BEGIN ORBIT INTEGRATION"));

    repo.ok_json(&["setup", "opencode", "--json"]);
    let agents = repo.read("AGENTS.md");
    assert_eq!(agents.matches("BEGIN ORBIT INTEGRATION").count(), 1);

    repo.ok_json(&["setup", "codex", "--remove", "--json"]);
    repo.ok_json(&["setup", "opencode", "--remove", "--json"]);
    let agents = repo.read("AGENTS.md");
    assert!(agents.contains("Keep this."));
    assert!(!agents.contains("BEGIN ORBIT INTEGRATION"));
    assert!(!repo.exists(".agents/skills/orbit/SKILL.md"));
}

#[test]
fn setup_mattpocock_writes_the_issue_tracker_bridge() {
    let repo = TestRepo::new();
    repo.ok_json(&["setup", "mattpocock", "--json"]);

    let tracker = repo.read("docs/agents/issue-tracker.md");
    assert!(tracker.contains("We use Orbit"));
    assert!(tracker.contains("orbit ready --json"));
    assert!(tracker.contains("Never edit task frontmatter directly"));

    repo.ok_json(&["setup", "mattpocock", "--json"]);
    let tracker = repo.read("docs/agents/issue-tracker.md");
    assert_eq!(tracker.matches("BEGIN ORBIT INTEGRATION").count(), 1);

    repo.ok_json(&["setup", "mattpocock", "--remove", "--json"]);
    assert!(!repo.exists("docs/agents/issue-tracker.md"));
}

#[test]
fn setup_all_writes_every_integration_and_removes_it() {
    let repo = TestRepo::new();
    repo.ok_json(&["setup", "all", "--hooks", "--json"]);

    for path in [
        "CLAUDE.md",
        "AGENTS.md",
        ".claude/settings.json",
        ".cursor/rules/orbit.mdc",
        ".agents/skills/orbit/SKILL.md",
        "docs/agents/issue-tracker.md",
        ".mcp.json",
        ".cursor/mcp.json",
        ".codex/config.toml",
        "opencode.json",
    ] {
        assert!(repo.exists(path), "{path} was not written");
    }

    let agents = repo.read("AGENTS.md");
    assert_eq!(agents.matches("BEGIN ORBIT INTEGRATION").count(), 1);

    repo.ok_json(&["setup", "all", "--remove", "--json"]);

    for path in [
        "CLAUDE.md",
        "AGENTS.md",
        ".cursor/rules/orbit.mdc",
        ".agents/skills/orbit/SKILL.md",
        "docs/agents/issue-tracker.md",
        ".mcp.json",
        ".cursor/mcp.json",
        ".codex/config.toml",
        "opencode.json",
    ] {
        assert!(!repo.exists(path), "{path} was not removed");
    }
    assert!(!repo.exists(".claude/settings.json"));
}

#[test]
fn setup_print_does_not_write() {
    let repo = TestRepo::new();
    let value = repo.ok_json(&["setup", "claude", "--hooks", "--print", "--json"]);

    assert_eq!(value["data"]["preview"], true);
    let files = value["data"]["files"].as_array().unwrap();
    assert!(files.iter().any(|file| {
        file["path"] == "CLAUDE.md" && file["content"].as_str().unwrap().contains("Orbit")
    }));
    assert!(!repo.exists("CLAUDE.md"));
    assert!(!repo.exists(".claude/settings.json"));

    let output = repo.orbit(&["setup", "claude", "--print"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("setup preview"));
    assert!(stdout.contains("BEGIN ORBIT INTEGRATION"));
    assert!(!repo.exists("CLAUDE.md"));
}

#[test]
fn setup_mcp_configs_are_merged_and_removed() {
    let repo = TestRepo::new();
    std::fs::write(
        repo.path().join(".mcp.json"),
        "{\n  \"mcpServers\": {\n    \"other\": {\n      \"command\": \"other-server\"\n    }\n  }\n}\n",
    )
    .unwrap();

    repo.ok_json(&["setup", "claude", "--mcp", "--json"]);
    let config = repo.read_json(".mcp.json");
    assert_orbit_command(&config["mcpServers"]["orbit"]["command"]);
    assert_eq!(config["mcpServers"]["orbit"]["args"][0], "mcp");
    assert_eq!(config["mcpServers"]["other"]["command"], "other-server");

    repo.ok_json(&["setup", "claude", "--remove", "--json"]);
    let config = repo.read_json(".mcp.json");
    assert!(config["mcpServers"].get("orbit").is_none());
    assert_eq!(config["mcpServers"]["other"]["command"], "other-server");
}

#[test]
fn setup_all_writes_mcp_configs_for_every_client() {
    let repo = TestRepo::new();
    repo.ok_json(&["setup", "all", "--json"]);

    let claude = repo.read_json(".mcp.json");
    assert_orbit_command(&claude["mcpServers"]["orbit"]["command"]);
    assert_eq!(claude["mcpServers"]["orbit"]["args"][0], "mcp");

    let cursor = repo.read_json(".cursor/mcp.json");
    assert_orbit_command(&cursor["mcpServers"]["orbit"]["command"]);
    assert_eq!(cursor["mcpServers"]["orbit"]["args"][0], "mcp");

    let codex: toml::Value = toml::from_str(&repo.read(".codex/config.toml")).unwrap();
    assert_orbit_toml_command(&codex["mcp_servers"]["orbit"]["command"]);
    assert_eq!(
        codex["mcp_servers"]["orbit"]["args"][0].as_str(),
        Some("mcp")
    );

    let opencode = repo.read_json("opencode.json");
    assert_eq!(opencode["mcp"]["orbit"]["type"], "local");
    assert_orbit_command(&opencode["mcp"]["orbit"]["command"][0]);
    assert_eq!(opencode["mcp"]["orbit"]["command"][1], "mcp");
    assert_eq!(opencode["mcp"]["orbit"]["enabled"], true);

    repo.ok_json(&["setup", "all", "--remove", "--json"]);
    assert!(!repo.exists(".mcp.json"));
    assert!(!repo.exists(".cursor/mcp.json"));
    assert!(!repo.exists(".codex/config.toml"));
    assert!(!repo.exists(".claude/settings.json"));
    assert!(!repo.exists("opencode.json"));
}

#[test]
fn setup_opencode_installs_mcp_by_default() {
    let repo = TestRepo::new();
    repo.ok_json(&["setup", "opencode", "--json"]);

    assert!(repo.exists("AGENTS.md"));
    let opencode = repo.read_json("opencode.json");
    assert_orbit_command(&opencode["mcp"]["orbit"]["command"][0]);
    assert_eq!(opencode["mcp"]["orbit"]["command"][1], "mcp");
}

#[test]
fn setup_no_mcp_writes_instructions_only() {
    let repo = TestRepo::new();
    repo.ok_json(&["setup", "opencode", "--no-mcp", "--json"]);

    assert!(repo.exists("AGENTS.md"));
    assert!(!repo.exists("opencode.json"));
}

#[test]
fn setup_migrates_a_bare_orbit_mcp_entry() {
    let repo = TestRepo::new();
    std::fs::write(
        repo.path().join(".mcp.json"),
        "{\n  \"mcpServers\": {\n    \"orbit\": {\n      \"command\": \"orbit\",\n      \"args\": [\"mcp\"]\n    }\n  }\n}\n",
    )
    .unwrap();

    repo.ok_json(&["setup", "claude", "--json"]);
    let config = repo.read_json(".mcp.json");
    assert_orbit_command(&config["mcpServers"]["orbit"]["command"]);
}

#[test]
fn setup_mcp_bin_can_write_a_portable_command() {
    let repo = TestRepo::new();
    let value = repo.ok_json(&["setup", "opencode", "--mcp-bin", "orbit", "--json"]);

    let opencode = repo.read_json("opencode.json");
    assert_eq!(opencode["mcp"]["orbit"]["command"][0], "orbit");
    assert!(
        value["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("PATH"))
    );
}

#[test]
fn setup_warns_when_an_mcp_config_is_tracked() {
    let repo = TestRepo::new();
    std::fs::write(repo.path().join("opencode.json"), "{}\n").unwrap();
    repo.git(&["add", "opencode.json"]);

    let value = repo.ok_json(&["setup", "opencode", "--json"]);
    assert!(
        value["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("tracked by git"))
    );
}

#[test]
fn setup_mcp_refuses_to_overwrite_a_foreign_orbit_server() {
    let repo = TestRepo::new();
    std::fs::write(
        repo.path().join(".mcp.json"),
        "{\n  \"mcpServers\": {\n    \"orbit\": {\n      \"command\": \"some-other-orbit\"\n    }\n  }\n}\n",
    )
    .unwrap();

    let (_, error) = repo.err_json(&["setup", "claude", "--mcp", "--json"]);
    assert_eq!(error["error"]["code"], "CONFIG_ERROR");
    let config = repo.read_json(".mcp.json");
    assert_eq!(config["mcpServers"]["orbit"]["command"], "some-other-orbit");
}

#[test]
fn setup_mcp_passes_an_external_board_dir() {
    let repo = TestRepo::new();
    let external = tempfile::tempdir().unwrap();
    let board = external.path().join("board");
    let board_arg = board.to_str().unwrap();

    let output = repo.orbit(&["--dir", board_arg, "init", "--json"]);
    assert!(output.status.success());

    let output = repo.orbit(&["--dir", board_arg, "setup", "claude", "--mcp", "--json"]);
    assert!(output.status.success());

    let config = repo.read_json(".mcp.json");
    assert_eq!(config["mcpServers"]["orbit"]["args"][0], "--dir");
    let expected = dunce::canonicalize(&board).unwrap();
    assert_eq!(
        config["mcpServers"]["orbit"]["args"][1],
        expected.to_string_lossy().as_ref()
    );
    assert_eq!(config["mcpServers"]["orbit"]["args"][2], "mcp");
}

#[test]
fn setup_mattpocock_ignores_mcp_with_a_warning() {
    let repo = TestRepo::new();
    let value = repo.ok_json(&["setup", "mattpocock", "--mcp", "--json"]);
    assert!(
        value["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("not an MCP client"))
    );
    assert!(repo.exists("docs/agents/issue-tracker.md"));
    assert!(!repo.exists(".mcp.json"));
}

#[test]
fn setup_refuses_to_overwrite_unmanaged_files() {
    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".cursor/rules")).unwrap();
    std::fs::write(repo.path().join(".cursor/rules/orbit.mdc"), "my own rule\n").unwrap();

    let (_, error) = repo.err_json(&["setup", "cursor", "--json"]);
    assert_eq!(error["error"]["code"], "CONFIG_ERROR");
    assert_eq!(repo.read(".cursor/rules/orbit.mdc"), "my own rule\n");
    assert!(!repo.exists(".agents/skills/orbit/SKILL.md"));

    let value = repo.ok_json(&["setup", "cursor", "--remove", "--json"]);
    assert_eq!(repo.read(".cursor/rules/orbit.mdc"), "my own rule\n");
    assert!(
        value["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("not managed"))
    );
}

#[test]
fn setup_hooks_are_claude_only() {
    let repo = TestRepo::new();
    let value = repo.ok_json(&["setup", "cursor", "--hooks", "--json"]);
    assert!(
        value["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap()
                .contains("only supported for Claude"))
    );
    assert!(repo.exists(".cursor/rules/orbit.mdc"));
    assert!(!repo.exists(".claude/settings.json"));
}

#[test]
fn prime_hook_json_emits_the_session_start_envelope() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Ready task", &["-p", "0"]);

    let output = repo.orbit(&["prime", "--hook-json", "--by", "claude"]);
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let context = value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("agent: claude"));
    assert!(context.contains("Ready task"));

    let value = repo.ok_json(&["prime", "--json", "--by", "claude"]);
    assert_eq!(value["data"]["agent"], "claude");
    assert_eq!(value["data"]["ready"].as_array().unwrap().len(), 1);

    let (_, error) = repo.err_json(&["--json", "prime", "--hook-json", "--by", "claude"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
}
