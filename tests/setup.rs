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

fn assert_hook_command(value: &Value) {
    let command = value.as_str().expect("command is a string");
    let suffix = " prime --hook-json --by claude";
    let binary = command
        .strip_suffix(suffix)
        .unwrap_or_else(|| panic!("unexpected hook command {command}"));
    assert!(
        Path::new(binary).is_absolute(),
        "{binary} is not an absolute path"
    );
    assert!(
        Path::new(binary)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "orbit" || name == "orbit.exe")
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
    assert_hook_command(&settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]);

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
    assert!(agents.contains(
        r#"`orbit done <id> --by <agent> --comment "..."` completes it and records the note with the status change; `orbit cancel <id> --by <agent>` abandons it."#
    ));

    repo.ok_json(&["setup", "opencode", "--json"]);
    assert_eq!(repo.read("AGENTS.md"), agents);

    let (status, value) = repo.json(&["setup", "opencode", "--no-mcp", "--check", "--json"]);
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["status"], "installed");

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
    assert!(tracker.contains(
        r#"`orbit done <ID> --by <agent> --comment "..."` or `orbit cancel <ID> --by <agent>`; the note is recorded with the status change."#
    ));

    repo.ok_json(&["setup", "mattpocock", "--json"]);
    let tracker = repo.read("docs/agents/issue-tracker.md");
    assert_eq!(tracker.matches("BEGIN ORBIT INTEGRATION").count(), 1);

    repo.ok_json(&["setup", "mattpocock", "--remove", "--json"]);
    assert!(!repo.exists("docs/agents/issue-tracker.md"));
}

#[test]
fn setup_mattpocock_updates_and_removes_a_managed_tracker_doc() {
    let repo = TestRepo::new();
    let (status, value) = repo.json(&["setup", "mattpocock", "--check", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["data"]["status"], "missing");

    repo.ok_json(&["setup", "mattpocock", "--json"]);
    let tracker = repo.read("docs/agents/issue-tracker.md");
    let edited = tracker.replace("We use Orbit", "We use Orbit daily");
    std::fs::write(repo.path().join("docs/agents/issue-tracker.md"), &edited).unwrap();

    let (status, value) = repo.json(&["setup", "mattpocock", "--check", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["data"]["status"], "stale");

    let value = repo.ok_json(&["setup", "mattpocock", "--json"]);
    let files = value["data"]["files"].as_array().unwrap();
    assert!(files.iter().any(|file| {
        file["path"] == "docs/agents/issue-tracker.md" && file["action"] == "updated"
    }));
    let tracker = repo.read("docs/agents/issue-tracker.md");
    assert!(tracker.contains("We use Orbit"));
    assert!(!tracker.contains("We use Orbit daily"));

    repo.ok_json(&["setup", "mattpocock", "--remove", "--json"]);
    assert!(!repo.exists("docs/agents/issue-tracker.md"));
}

#[test]
fn setup_mattpocock_leaves_a_custom_tracker_doc_alone() {
    let repo = TestRepo::new();
    let custom = "# Issue tracker: Orbit\n\nHand-written guidance.\n";
    std::fs::create_dir_all(repo.path().join("docs/agents")).unwrap();
    std::fs::write(repo.path().join("docs/agents/issue-tracker.md"), custom).unwrap();

    let value = repo.ok_json(&["setup", "mattpocock", "--json"]);
    assert_eq!(repo.read("docs/agents/issue-tracker.md"), custom);
    let files = value["data"]["files"].as_array().unwrap();
    assert!(files.iter().any(|file| {
        file["path"] == "docs/agents/issue-tracker.md" && file["action"] == "skipped"
    }));
    assert!(
        value["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("not managed"))
    );

    let (status, value) = repo.json(&["setup", "mattpocock", "--check", "--json"]);
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["status"], "installed");

    repo.ok_json(&["setup", "mattpocock", "--remove", "--json"]);
    assert_eq!(repo.read("docs/agents/issue-tracker.md"), custom);
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
    let orbit = &opencode["mcp"]["servers"]["orbit"];
    assert_eq!(orbit["type"], "local");
    assert_orbit_command(&orbit["command"][0]);
    assert_eq!(orbit["command"][1], "mcp");
    assert!(orbit.get("enabled").is_none());

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
    let orbit = &opencode["mcp"]["servers"]["orbit"];
    assert_eq!(orbit["type"], "local");
    assert_orbit_command(&orbit["command"][0]);
    assert_eq!(orbit["command"][1], "mcp");
    assert!(orbit.get("enabled").is_none());
    assert!(opencode["mcp"].get("orbit").is_none());
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
fn setup_opencode_merges_an_existing_v2_config() {
    let repo = TestRepo::new();
    std::fs::write(
        repo.path().join("opencode.json"),
        "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"theme\": \"dark\",\n  \"mcp\": {\n    \"servers\": {\n      \"other\": {\n        \"type\": \"local\",\n        \"command\": [\"other\", \"mcp\"]\n      }\n    }\n  }\n}\n",
    )
    .unwrap();

    let value = repo.ok_json(&["setup", "opencode", "--json"]);
    let files = value["data"]["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .any(|file| { file["path"] == "opencode.json" && file["action"] == "updated" })
    );

    let config = repo.read_json("opencode.json");
    let orbit = &config["mcp"]["servers"]["orbit"];
    assert_eq!(orbit["type"], "local");
    assert_orbit_command(&orbit["command"][0]);
    assert!(orbit.get("enabled").is_none());
    assert_eq!(config["mcp"]["servers"]["other"]["command"][0], "other");
    assert_eq!(config["theme"], "dark");
    let before = repo.read("opencode.json");

    repo.ok_json(&["setup", "opencode", "--json"]);
    assert_eq!(repo.read("opencode.json"), before);
}

#[test]
fn setup_migrates_a_legacy_opencode_mcp_entry() {
    let repo = TestRepo::new();
    std::fs::write(
        repo.path().join("opencode.json"),
        "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"mcp\": {\n    \"orbit\": {\n      \"type\": \"local\",\n      \"command\": [\"orbit\", \"mcp\"],\n      \"enabled\": true\n    },\n    \"other\": {\n      \"type\": \"local\",\n      \"command\": [\"other\", \"mcp\"]\n    }\n  }\n}\n",
    )
    .unwrap();

    repo.ok_json(&["setup", "opencode", "--json"]);
    let config = repo.read_json("opencode.json");
    let orbit = &config["mcp"]["servers"]["orbit"];
    assert_eq!(orbit["type"], "local");
    assert_orbit_command(&orbit["command"][0]);
    assert_eq!(orbit["command"][1], "mcp");
    assert!(orbit.get("enabled").is_none());
    assert!(config["mcp"].get("orbit").is_none());
    assert_eq!(config["mcp"]["other"]["command"][0], "other");
    assert_eq!(config["$schema"], "https://opencode.ai/config.json");
}

#[test]
fn setup_opencode_remove_preserves_unrelated_servers() {
    let repo = TestRepo::new();
    std::fs::write(
        repo.path().join("opencode.json"),
        "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"theme\": \"dark\",\n  \"mcp\": {\n    \"orbit\": {\n      \"type\": \"local\",\n      \"command\": [\"orbit\", \"mcp\"],\n      \"enabled\": true\n    },\n    \"legacy-other\": {\n      \"type\": \"local\",\n      \"command\": [\"legacy-other\", \"mcp\"]\n    },\n    \"servers\": {\n      \"orbit\": {\n        \"type\": \"local\",\n        \"command\": [\"orbit\", \"mcp\"]\n      },\n      \"other\": {\n        \"type\": \"local\",\n        \"command\": [\"other\", \"mcp\"]\n      }\n    }\n  }\n}\n",
    )
    .unwrap();

    repo.ok_json(&["setup", "opencode", "--remove", "--json"]);
    let config = repo.read_json("opencode.json");
    assert!(config["mcp"].get("orbit").is_none());
    assert!(config["mcp"]["servers"].get("orbit").is_none());
    assert_eq!(config["mcp"]["servers"]["other"]["command"][0], "other");
    assert_eq!(config["mcp"]["legacy-other"]["command"][0], "legacy-other");
    assert_eq!(config["theme"], "dark");
    assert_eq!(config["$schema"], "https://opencode.ai/config.json");
}

#[test]
fn setup_opencode_remove_deletes_a_legacy_only_config() {
    let repo = TestRepo::new();
    std::fs::write(
        repo.path().join("opencode.json"),
        "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"mcp\": {\n    \"orbit\": {\n      \"type\": \"local\",\n      \"command\": [\"orbit\", \"mcp\"],\n      \"enabled\": true\n    }\n  }\n}\n",
    )
    .unwrap();

    repo.ok_json(&["setup", "opencode", "--remove", "--json"]);
    assert!(!repo.exists("opencode.json"));
}

#[test]
fn setup_opencode_refuses_a_foreign_legacy_orbit_entry() {
    let repo = TestRepo::new();
    let content = "{\n  \"mcp\": {\n    \"orbit\": {\n      \"type\": \"local\",\n      \"command\": [\"some-other-orbit\", \"mcp\"]\n    }\n  }\n}\n";
    std::fs::write(repo.path().join("opencode.json"), content).unwrap();

    let (_, error) = repo.err_json(&["setup", "opencode", "--json"]);
    assert_eq!(error["error"]["code"], "CONFIG_ERROR");
    assert_eq!(repo.read("opencode.json"), content);
}

#[test]
fn setup_mcp_bin_can_write_a_portable_command() {
    let repo = TestRepo::new();
    let value = repo.ok_json(&["setup", "opencode", "--mcp-bin", "orbit", "--json"]);

    let opencode = repo.read_json("opencode.json");
    assert_eq!(opencode["mcp"]["servers"]["orbit"]["command"][0], "orbit");
    assert!(
        value["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("PATH"))
    );
}

#[test]
fn setup_portable_mcp_writes_bare_commands_and_is_idempotent() {
    let repo = TestRepo::new();
    std::fs::write(
        repo.path().join(".mcp.json"),
        "{\n  \"mcpServers\": {\n    \"other\": {\n      \"command\": \"other-server\"\n    }\n  }\n}\n",
    )
    .unwrap();

    repo.ok_json(&["setup", "all", "--portable-mcp", "--json"]);

    let claude = repo.read_json(".mcp.json");
    assert_eq!(claude["mcpServers"]["orbit"]["command"], "orbit");
    assert_eq!(claude["mcpServers"]["other"]["command"], "other-server");

    let cursor = repo.read_json(".cursor/mcp.json");
    assert_eq!(cursor["mcpServers"]["orbit"]["command"], "orbit");

    let codex: toml::Value = toml::from_str(&repo.read(".codex/config.toml")).unwrap();
    assert_eq!(
        codex["mcp_servers"]["orbit"]["command"].as_str(),
        Some("orbit")
    );

    let opencode = repo.read_json("opencode.json");
    assert_eq!(opencode["mcp"]["servers"]["orbit"]["command"][0], "orbit");

    let claude_before = repo.read(".mcp.json");
    let cursor_before = repo.read(".cursor/mcp.json");
    let codex_before = repo.read(".codex/config.toml");
    let opencode_before = repo.read("opencode.json");
    let second = repo.ok_json(&["setup", "all", "--portable-mcp", "--json"]);
    assert_eq!(repo.read(".mcp.json"), claude_before);
    assert_eq!(repo.read(".cursor/mcp.json"), cursor_before);
    assert_eq!(repo.read(".codex/config.toml"), codex_before);
    assert_eq!(repo.read("opencode.json"), opencode_before);
    assert!(
        second["data"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|file| { file["action"] == "unchanged" })
    );
}

#[test]
fn setup_portable_mcp_keeps_the_claude_hook_absolute() {
    let repo = TestRepo::new();
    repo.ok_json(&["setup", "claude", "--hooks", "--portable-mcp", "--json"]);

    let mcp = repo.read_json(".mcp.json");
    assert_eq!(mcp["mcpServers"]["orbit"]["command"], "orbit");
    let settings = repo.read_json(".claude/settings.json");
    assert_hook_command(&settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]);
}

#[test]
fn setup_mcp_bin_orbit_keeps_the_claude_hook_absolute() {
    let repo = TestRepo::new();
    repo.ok_json(&["setup", "claude", "--hooks", "--mcp-bin", "orbit", "--json"]);

    let mcp = repo.read_json(".mcp.json");
    assert_eq!(mcp["mcpServers"]["orbit"]["command"], "orbit");
    let settings = repo.read_json(".claude/settings.json");
    assert_hook_command(&settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]);
}

#[test]
fn setup_repair_without_the_flag_restores_the_absolute_default() {
    let repo = TestRepo::new();
    repo.ok_json(&["setup", "opencode", "--portable-mcp", "--json"]);

    repo.ok_json(&["setup", "opencode", "--json"]);
    let opencode = repo.read_json("opencode.json");
    let command = opencode["mcp"]["servers"]["orbit"]["command"][0]
        .as_str()
        .expect("command is a string");
    assert!(Path::new(command).is_absolute(), "{command}");

    let (status, value) = repo.json(&["setup", "opencode", "--check", "--json"]);
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["status"], "installed");
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
    let instructions = value["data"]["instructions"].as_str().unwrap();
    assert!(instructions.contains(
        r#"`orbit done <id> --by <agent> --comment "..."` completes it and records the note with the status change; `orbit cancel <id> --by <agent>` abandons it."#
    ));

    let (_, error) = repo.err_json(&["--json", "prime", "--hook-json", "--by", "claude"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
}
