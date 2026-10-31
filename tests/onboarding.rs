mod common;

use common::TestRepo;

#[test]
fn init_is_idempotent_and_reports_unchanged_artifacts() {
    let repo = TestRepo::new();

    let first = repo.ok_json(&["init", "--agents", "none", "--json"]);
    assert_eq!(first["data"]["created"], true);
    assert_eq!(first["data"]["storage"], "worktree");

    let second = repo.ok_json(&["init", "--agents", "none", "--json"]);
    assert_eq!(second["data"]["created"], false);
    assert_eq!(second["data"]["board"], first["data"]["board"]);
}

#[test]
fn init_without_agents_writes_no_harness_files() {
    let repo = TestRepo::new();
    repo.ok_json(&["init", "--agents", "none", "--json"]);

    assert!(!repo.exists("CLAUDE.md"));
    assert!(!repo.exists("AGENTS.md"));
    assert!(!repo.exists(".mcp.json"));
    assert!(!repo.exists("opencode.json"));
    assert!(repo.board_path().join("config.toml").exists());
}

#[test]
fn init_wires_only_detected_harnesses() {
    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::create_dir_all(repo.path().join(".cursor")).unwrap();

    repo.ok_json(&["init", "--agents", "auto", "--json"]);

    assert!(repo.exists("CLAUDE.md"));
    assert!(repo.exists(".mcp.json"));
    assert!(repo.exists(".cursor/rules/orbit.mdc"));
    assert!(repo.exists(".cursor/mcp.json"));
    assert!(!repo.exists("opencode.json"));
    assert!(!repo.exists(".codex/config.toml"));
    assert!(!repo.exists("docs/agents/issue-tracker.md"));
}

#[test]
fn init_wires_an_explicit_list() {
    let repo = TestRepo::new();

    repo.ok_json(&["init", "--agents", "claude,cursor", "--json"]);

    assert!(repo.exists("CLAUDE.md"));
    assert!(repo.exists(".cursor/mcp.json"));
    assert!(!repo.exists("AGENTS.md"));
    assert!(!repo.exists("opencode.json"));
}

#[test]
fn init_writes_absolute_mcp_paths_by_default() {
    let repo = TestRepo::new();

    repo.ok_json(&["init", "--agents", "claude", "--json"]);

    let claude = repo.read_json(".mcp.json");
    let command = claude["mcpServers"]["orbit"]["command"]
        .as_str()
        .expect("command is a string");
    assert!(std::path::Path::new(command).is_absolute(), "{command}");
}

#[test]
fn init_mcp_configs_never_record_the_board_path() {
    let repo = TestRepo::new();
    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    let board_arg = board.to_str().unwrap();

    let value = repo.ok_json(&[
        "init",
        board_arg,
        "--agents",
        "claude,cursor,codex,opencode",
        "--json",
    ]);

    let files = value["data"]["files"].as_array().unwrap();
    let rendered = serde_json::to_string(files).unwrap();
    assert!(!rendered.contains(board_arg), "{rendered}");
    assert!(!rendered.contains("--dir"), "{rendered}");

    let claude = repo.read_json(".mcp.json");
    assert_eq!(
        claude["mcpServers"]["orbit"]["args"],
        serde_json::json!(["mcp"])
    );
    let opencode = repo.read_json("opencode.json");
    let command = opencode["mcp"]["servers"]["orbit"]["command"]
        .as_array()
        .unwrap();
    assert_eq!(
        command.last().and_then(serde_json::Value::as_str),
        Some("mcp")
    );
    assert!(
        !command
            .iter()
            .any(|arg| arg.as_str().is_some_and(|arg| arg.contains(board_arg))),
        "{opencode}"
    );
}

#[test]
fn init_portable_mcp_writes_bare_commands_and_explains_the_path_requirement() {
    let repo = TestRepo::new();

    let value = repo.ok_json(&[
        "init",
        "--agents",
        "claude,opencode",
        "--portable-mcp",
        "--json",
    ]);

    let claude = repo.read_json(".mcp.json");
    assert_eq!(claude["mcpServers"]["orbit"]["command"], "orbit");
    let opencode = repo.read_json("opencode.json");
    assert_eq!(opencode["mcp"]["servers"]["orbit"]["command"][0], "orbit");

    let warnings = value["meta"]["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|warning| {
            let warning = warning.as_str().unwrap();
            warning.contains("portable") && warning.contains("PATH")
        }),
        "{value}"
    );

    let unchanged = repo.ok_json(&[
        "init",
        "--agents",
        "claude,opencode",
        "--portable-mcp",
        "--json",
    ]);
    assert!(
        unchanged["data"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|file| file["action"] == "unchanged"),
        "{unchanged}"
    );
}

#[test]
fn init_repair_without_the_flag_restores_the_absolute_default() {
    let repo = TestRepo::new();
    let first = repo.ok_json(&["init", "--agents", "opencode", "--portable-mcp", "--json"]);
    assert_eq!(first["data"]["portable_mcp"], true);

    let second = repo.ok_json(&["init", "--agents", "opencode", "--json"]);
    assert_eq!(second["data"]["portable_mcp"], false);
    let opencode = repo.read_json("opencode.json");
    let command = opencode["mcp"]["servers"]["orbit"]["command"][0]
        .as_str()
        .expect("command is a string");
    assert!(std::path::Path::new(command).is_absolute(), "{command}");
}

#[test]
fn init_reports_the_actual_mcp_mode_for_mcp_bin_orbit() {
    let repo = TestRepo::new();

    let value = repo.ok_json(&[
        "init",
        "--agents",
        "opencode",
        "--mcp-bin",
        "orbit",
        "--json",
    ]);

    assert_eq!(value["data"]["portable_mcp"], true);
    let opencode = repo.read_json("opencode.json");
    assert_eq!(opencode["mcp"]["servers"]["orbit"]["command"][0], "orbit");
}

#[test]
fn init_without_agents_does_not_report_portable_mcp() {
    let repo = TestRepo::new();

    let value = repo.ok_json(&["init", "--agents", "none", "--portable-mcp", "--json"]);

    assert_eq!(value["data"]["portable_mcp"], false);
}

#[test]
fn init_warns_when_no_selected_harness_uses_mcp() {
    let repo = TestRepo::new();

    let value = repo.ok_json(&["init", "--agents", "none", "--portable-mcp", "--json"]);

    assert_eq!(value["data"]["portable_mcp"], false);
    assert!(
        value["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap()
                .contains("no selected harness uses MCP")),
        "{value}"
    );
}

#[test]
fn link_preserves_a_portable_mcp_configuration() {
    let repo = TestRepo::new();
    repo.ok_json(&["init", "--agents", "opencode", "--portable-mcp", "--json"]);

    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    let other = TestRepo::new();
    other.init_at(&board);

    repo.ok_json(&["link", board.to_str().unwrap(), "--json"]);

    let opencode = repo.read_json("opencode.json");
    assert_eq!(
        opencode["mcp"]["servers"]["orbit"]["command"],
        serde_json::json!(["orbit", "mcp"])
    );
}

#[test]
fn link_refresh_preserves_each_clients_mcp_mode() {
    let repo = TestRepo::new();
    repo.ok_json(&["setup", "claude", "--portable-mcp", "--json"]);
    repo.ok_json(&["setup", "codex", "--json"]);

    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    let other = TestRepo::new();
    other.init_at(&board);

    repo.ok_json(&["link", board.to_str().unwrap(), "--json"]);

    let claude = repo.read_json(".mcp.json");
    assert_eq!(claude["mcpServers"]["orbit"]["command"], "orbit");
    let codex: toml::Value = toml::from_str(&repo.read(".codex/config.toml")).unwrap();
    let command = codex["mcp_servers"]["orbit"]["command"]
        .as_str()
        .expect("command is a string");
    assert!(std::path::Path::new(command).is_absolute(), "{command}");
}

#[test]
fn init_all_wires_every_harness() {
    let repo = TestRepo::new();

    repo.ok_json(&["init", "--agents", "all", "--json"]);

    assert!(repo.exists("CLAUDE.md"));
    assert!(repo.exists(".cursor/mcp.json"));
    assert!(repo.exists(".agents/skills/orbit/SKILL.md"));
    assert!(repo.exists(".codex/config.toml"));
    assert!(repo.exists("opencode.json"));
    assert!(!repo.exists("docs/agents/issue-tracker.md"));
    assert!(!repo.exists("docs/agents/triage-labels.md"));
}

#[test]
fn init_rejects_mattpocock_as_a_harness() {
    let repo = TestRepo::new();

    let (status, value) = repo.json(&["init", "--agents", "mattpocock", "--json"]);

    assert_eq!(status, 1, "{value}");
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("expected claude, cursor, codex, or opencode"),
        "{value}"
    );
}

#[test]
fn init_auto_ignores_mattpocock_skills() {
    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".agents/skills/setup-matt-pocock-skills")).unwrap();

    let value = repo.ok_json(&["init", "--agents", "auto", "--json"]);

    assert_eq!(value["data"]["agents"], serde_json::json!([]));
    assert!(!repo.exists("docs/agents/issue-tracker.md"));
    assert!(!repo.exists("docs/agents/triage-labels.md"));
}

#[test]
fn init_print_writes_nothing() {
    let repo = TestRepo::new();

    repo.ok_json(&["init", "--agents", "all", "--print", "--json"]);

    assert!(!repo.board_path().join("config.toml").exists());
    assert!(!repo.exists("CLAUDE.md"));
    assert!(!repo.exists("AGENTS.md"));
}

#[test]
fn init_print_defaults_to_detection_without_prompting() {
    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::create_dir_all(repo.path().join(".cursor")).unwrap();

    // A prompt answer is present, but --print must select by detection.
    let (status, value) = repo.json_with_stdin(&["init", "--print", "--json"], "1\n");

    assert_eq!(status, 0, "{value}");
    assert_eq!(
        value["data"]["agents"],
        serde_json::json!(["claude", "cursor"])
    );
    assert_eq!(value["data"]["preview"], true);
    assert!(!repo.board_path().join("config.toml").exists());
    assert!(!repo.exists("CLAUDE.md"));
    assert!(!repo.exists(".mcp.json"));
    assert!(!repo.exists(".cursor/rules/orbit.mdc"));
    assert!(!repo.exists(".cursor/mcp.json"));
}

#[test]
fn init_print_with_agents_ask_does_not_prompt() {
    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::create_dir_all(repo.path().join(".cursor")).unwrap();

    let (status, value) =
        repo.json_with_stdin(&["init", "--print", "--agents", "ask", "--json"], "1\n");

    assert_eq!(status, 0, "{value}");
    assert_eq!(
        value["data"]["agents"],
        serde_json::json!(["claude", "cursor"])
    );
    assert_eq!(value["data"]["preview"], true);
    assert!(!repo.board_path().join("config.toml").exists());
    assert!(!repo.exists("CLAUDE.md"));
    assert!(!repo.exists(".mcp.json"));
}

#[test]
fn init_outside_a_git_repository_creates_a_workspace_board() {
    let repo = TestRepo::new();
    let outside = tempfile::tempdir().unwrap();

    let (status, value) = repo.json_in(outside.path(), &["init", "--json"]);

    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["storage"], "repository");
    assert_eq!(value["data"]["global"], false);
    assert!(outside.path().join(".orbit/board/config.toml").is_file());
    assert!(outside.path().join(".orbit/cache").is_dir());
}

#[test]
fn init_ask_prompts_and_accepts_a_piped_answer() {
    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();

    let (status, value) = repo.json_with_stdin(&["init", "--agents", "ask", "--json"], "1\n");

    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["agents"], serde_json::json!(["claude"]));
    assert!(repo.exists(".mcp.json"));
    assert!(!repo.exists(".cursor/mcp.json"));
}

#[test]
fn init_ask_empty_answer_uses_detected_harnesses() {
    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::create_dir_all(repo.path().join(".cursor")).unwrap();

    let (status, value) = repo.json_with_stdin(&["init", "--agents", "ask", "--json"], "\n");

    assert_eq!(status, 0, "{value}");
    assert_eq!(
        value["data"]["agents"],
        serde_json::json!(["claude", "cursor"])
    );
}

#[test]
fn init_ask_rejects_an_unknown_answer_then_accepts() {
    let repo = TestRepo::new();

    let (status, value) =
        repo.json_with_stdin(&["init", "--agents", "ask", "--json"], "nope\ncodex\n");

    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["agents"], serde_json::json!(["codex"]));
    assert!(repo.exists(".codex/config.toml"));
}

#[test]
fn link_shares_one_board_between_repositories() {
    let first = TestRepo::new();
    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    let board_arg = board.to_str().unwrap();

    first.init_at(&board);
    first.link_at(&board);
    let id = first.add("Cross-repo task", &[]);

    let second = TestRepo::new();
    second.ok_json(&["link", board_arg, "--json"]);

    let listed = second.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);

    second.ok_json(&["done", &id, "--by", "test", "--json"]);
    let listed = first.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["status"], "done");
}

#[test]
fn dir_overrides_a_link_for_one_command() {
    let linked = TestRepo::new();
    let shared = tempfile::tempdir().unwrap();
    let shared_board = shared.path().join("board");
    linked.init_at(&shared_board);
    linked.link_at(&shared_board);

    let other = TestRepo::new();
    other.init();
    let other_board = other.board_path();
    let other_id = other.add("Other board task", &[]);
    let other_board_arg = other_board.to_str().unwrap();

    let listed = linked.ok_json(&["--dir", other_board_arg, "list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], other_id);
}

#[test]
fn orbit_dir_overrides_a_link() {
    let linked = TestRepo::new();
    let shared = tempfile::tempdir().unwrap();
    let shared_board = shared.path().join("board");
    linked.init_at(&shared_board);
    linked.link_at(&shared_board);

    let other = TestRepo::new();
    other.init();
    let other_id = other.add("Other board task", &[]);
    let other_board = other.board_path();

    let (status, value) = linked.json_with_env(
        &["list", "--all", "--json"],
        &[("ORBIT_DIR", other_board.to_str().unwrap())],
    );
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["tasks"][0]["id"], other_id);
}

#[test]
fn a_linked_worktree_resolves_the_shared_board() {
    let repo = TestRepo::new();
    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    repo.init_at(&board);
    repo.link_at(&board);

    let worktree_dir = tempfile::tempdir().unwrap();
    let worktree = worktree_dir.path().join("wt");
    repo.add_worktree(&worktree, "feature");

    let id = repo.add("From main", &[]);
    let listed = repo.ok_json_in(&worktree, &["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);

    let added = repo.ok_json_in(&worktree, &["add", "From worktree", "--json"]);
    let worktree_id = added["data"]["id"].as_str().unwrap();
    let listed = repo.ok_json(&["list", "--all", "--json"]);
    assert!(
        listed["data"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["id"] == worktree_id)
    );
}

#[test]
fn unlink_restores_the_local_board() {
    let repo = TestRepo::new();
    repo.init();
    let local_id = repo.add("Local task", &[]);

    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    repo.init_at(&board);
    repo.link_at(&board);
    let shared_id = repo.add("Shared task", &[]);
    assert_ne!(local_id, shared_id);

    repo.ok_json(&["unlink", "--json"]);

    let listed = repo.ok_json(&["list", "--all", "--json"]);
    let ids: Vec<&str> = listed["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert_eq!(ids, vec![local_id.as_str()]);
}

#[test]
fn link_rejects_a_directory_that_is_not_a_board() {
    let repo = TestRepo::new();
    let empty = tempfile::tempdir().unwrap();

    let (status, value) = repo.json(&["link", empty.path().to_str().unwrap(), "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "LINK_NOT_FOUND");
}

#[test]
fn link_warns_when_it_shadows_a_local_board() {
    let repo = TestRepo::new();
    repo.init();
    let local_id = repo.add("Local task", &[]);

    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    let other = TestRepo::new();
    other.init_at(&board);

    let value = repo.ok_json(&["link", board.to_str().unwrap(), "--json"]);
    assert_eq!(value["data"]["shadowed"], true);
    let warnings = value["meta"]["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("shadowed"))
    );

    // The local Board is untouched.
    let local = repo.ok_json(&[
        "--dir",
        repo.board_path().to_str().unwrap(),
        "list",
        "--all",
        "--json",
    ]);
    assert_eq!(local["data"]["tasks"][0]["id"], local_id);
}

#[test]
fn init_in_a_linked_repository_requires_unlink() {
    let repo = TestRepo::new();
    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    let other = TestRepo::new();
    other.init_at(&board);
    repo.link_at(&board);

    let (status, value) = repo.json(&["init", "--agents", "none", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "ALREADY_INITIALIZED");

    repo.ok_json(&["unlink", "--json"]);
    repo.ok_json(&["init", "--agents", "none", "--json"]);
    assert!(repo.board_path().join("config.toml").exists());
}

#[test]
fn init_dir_is_idempotent() {
    let repo = TestRepo::new();
    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");

    let first = repo.init_at(&board);
    assert_eq!(first["data"]["created"], true);
    assert_eq!(first["data"]["storage"], "repository");

    let second = repo.init_at(&board);
    assert_eq!(second["data"]["created"], false);
}

#[test]
fn link_refreshes_an_absolute_board_path_out_of_existing_configs() {
    let repo = TestRepo::new();
    repo.ok_json(&["init", "--agents", "claude", "--json"]);
    std::fs::write(
        repo.path().join(".mcp.json"),
        "{\n  \"mcpServers\": {\n    \"orbit\": {\n      \"command\": \"orbit\",\n      \"args\": [\"--dir\", \"/old/board\", \"mcp\"]\n    }\n  }\n}\n",
    )
    .unwrap();

    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    let other = TestRepo::new();
    other.init_at(&board);

    repo.ok_json(&["link", board.to_str().unwrap(), "--json"]);

    let after = repo.read_json(".mcp.json");
    let args = after["mcpServers"]["orbit"]["args"].as_array().unwrap();
    assert_eq!(args, &vec![serde_json::json!("mcp")], "{after}");
    let command = after["mcpServers"]["orbit"]["command"]
        .as_str()
        .expect("command is a string");
    assert_eq!(command, "orbit");
}

#[test]
fn init_does_not_touch_a_custom_mattpocock_tracker_doc() {
    let repo = TestRepo::new();
    let custom = "# Issue tracker: Orbit\n\nHand-written guidance.\n";
    std::fs::create_dir_all(repo.path().join("docs/agents")).unwrap();
    std::fs::write(repo.path().join("docs/agents/issue-tracker.md"), custom).unwrap();

    let value = repo.ok_json(&["init", "--agents", "all", "--json"]);
    assert_eq!(repo.read("docs/agents/issue-tracker.md"), custom);
    assert!(!repo.exists("docs/agents/triage-labels.md"));
    assert!(
        !value["data"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "docs/agents/issue-tracker.md")
    );
}

#[test]
fn link_recognizes_and_preserves_a_custom_mattpocock_tracker_doc() {
    let repo = TestRepo::new();
    repo.ok_json(&["init", "--agents", "none", "--json"]);
    let custom = "# Issue tracker: Orbit\n\nHand-written guidance.\n";
    std::fs::create_dir_all(repo.path().join("docs/agents")).unwrap();
    std::fs::write(repo.path().join("docs/agents/issue-tracker.md"), custom).unwrap();

    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    let other = TestRepo::new();
    other.init_at(&board);

    let value = repo.ok_json(&["link", board.to_str().unwrap(), "--json"]);
    assert_eq!(repo.read("docs/agents/issue-tracker.md"), custom);
    assert!(
        value["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("not managed")),
        "{value}"
    );
}

#[test]
fn setup_check_reports_missing_and_installed() {
    let repo = TestRepo::new();
    repo.init();

    let (status, value) = repo.json(&["setup", "claude", "--check", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["data"]["status"], "missing");

    repo.ok_json(&["setup", "claude", "--json"]);

    let (status, value) = repo.json(&["setup", "claude", "--check", "--json"]);
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["status"], "installed");
}

#[test]
fn setup_check_reports_stale_after_an_edit() {
    let repo = TestRepo::new();
    repo.init();
    repo.ok_json(&["setup", "claude", "--json"]);

    let instructions = repo
        .read("CLAUDE.md")
        .replace("never edit task files", "sometimes edit task files");
    std::fs::write(repo.path().join("CLAUDE.md"), instructions).unwrap();

    let (status, value) = repo.json(&["setup", "claude", "--check", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["data"]["status"], "stale");
}

#[test]
fn setup_refuses_an_npx_cache_binary() {
    let repo = TestRepo::new();
    repo.init();

    let npx_bin = repo
        .path()
        .join(".npm/_npx/86e7861870a34cc4/node_modules/@rcmendes/orbit/orbit");
    std::fs::create_dir_all(npx_bin.parent().unwrap()).unwrap();
    std::fs::write(&npx_bin, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&npx_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&npx_bin, permissions).unwrap();
    }

    let (status, value) = repo.json(&[
        "setup",
        "claude",
        "--mcp-bin",
        npx_bin.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "EPHEMERAL_BIN");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("npm install -g"), "{message}");
    assert!(message.contains("--mcp-bin orbit"), "{message}");
}

#[test]
fn setup_accepts_the_portable_orbit_command() {
    let repo = TestRepo::new();
    repo.init();

    let value = repo.ok_json(&["setup", "claude", "--mcp-bin", "orbit", "--json"]);
    let warnings = value["meta"]["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("PATH"))
    );

    let mcp = repo.read_json(".mcp.json");
    assert_eq!(mcp["mcpServers"]["orbit"]["command"], "orbit");
}

#[test]
fn claude_hook_records_the_absolute_binary_and_removes_cleanly() {
    let repo = TestRepo::new();
    repo.init();

    repo.ok_json(&["setup", "claude", "--hooks", "--json"]);
    let settings = repo.read_json(".claude/settings.json");
    let command = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let suffix = " prime --hook-json --by claude";
    let binary = command.strip_suffix(suffix).unwrap();
    assert!(
        std::path::Path::new(binary).is_absolute(),
        "{command} is not absolute"
    );

    repo.ok_json(&["setup", "claude", "--remove", "--json"]);
    assert!(!repo.exists(".claude/settings.json"));
}

#[test]
fn claude_hook_replaces_a_legacy_bare_command() {
    let repo = TestRepo::new();
    repo.init();
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::write(
        repo.path().join(".claude/settings.json"),
        r#"{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"orbit prime --hook-json --by claude"}]}]}}"#,
    )
    .unwrap();

    repo.ok_json(&["setup", "claude", "--hooks", "--json"]);

    let settings = repo.read_json(".claude/settings.json");
    let handlers = settings["hooks"]["SessionStart"][0]["hooks"]
        .as_array()
        .unwrap();
    assert_eq!(handlers.len(), 1, "{settings}");
    let command = handlers[0]["command"].as_str().unwrap();
    assert!(command.ends_with(" prime --hook-json --by claude"));
    assert!(!command.starts_with("orbit "), "{command}");
}

#[test]
fn doctor_warns_when_orbit_is_not_on_path() {
    let repo = TestRepo::new();
    repo.init();
    repo.ok_json(&["setup", "claude", "--hooks", "--json"]);

    let value = repo.ok_json(&["doctor", "--json"]);

    assert_eq!(value["data"]["path_on_path"], false);
    assert!(
        value["data"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("PATH")),
        "{value}"
    );
}

#[test]
fn init_without_agents_and_without_a_tty_writes_nothing_and_prints_the_hint() {
    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();

    let output = repo.orbit(&["init"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("2. Coding agents (skipped)"), "{stdout}");
    assert!(stdout.contains("orbit setup <client>"), "{stdout}");
    assert!(!repo.exists("CLAUDE.md"));
    assert!(!repo.exists(".mcp.json"));
    assert!(!repo.exists(".claude/settings.json"));
}

#[test]
fn init_no_agents_is_an_alias_for_none() {
    let repo = TestRepo::new();

    let value = repo.ok_json(&["init", "--no-agents", "--json"]);

    assert_eq!(value["data"]["agents"], serde_json::json!([]));
    assert!(!repo.exists("CLAUDE.md"));
}

#[test]
fn init_dir_works_outside_a_git_repository() {
    let repo = TestRepo::new();
    let outside = tempfile::tempdir().unwrap();
    let board = outside.path().join("board");

    let value = repo.ok_json_in(
        outside.path(),
        &[
            "init",
            board.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );

    assert_eq!(value["data"]["storage"], "repository");
    assert_eq!(value["data"]["global"], false);
    assert!(board.join("config.toml").is_file());
}

#[test]
fn unlink_refreshes_agent_configuration_back_to_the_local_board() {
    let repo = TestRepo::new();
    repo.ok_json(&["init", "--agents", "claude", "--json"]);

    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    let other = TestRepo::new();
    other.init_at(&board);

    repo.ok_json(&["link", board.to_str().unwrap(), "--json"]);
    let linked = repo.read_json(".mcp.json");
    assert_eq!(
        linked["mcpServers"]["orbit"]["args"],
        serde_json::json!(["mcp"])
    );

    std::fs::write(
        repo.path().join(".mcp.json"),
        "{\n  \"mcpServers\": {\n    \"orbit\": {\n      \"command\": \"orbit\",\n      \"args\": [\"--dir\", \"/old/board\", \"mcp\"]\n    }\n  }\n}\n",
    )
    .unwrap();

    repo.ok_json(&["unlink", "--json"]);
    let local = repo.read_json(".mcp.json");
    assert_eq!(
        local["mcpServers"]["orbit"]["args"],
        serde_json::json!(["mcp"])
    );
    let command = local["mcpServers"]["orbit"]["command"]
        .as_str()
        .expect("command is a string");
    assert_eq!(command, "orbit");
}

#[test]
fn setup_refuses_a_temporary_binary() {
    let repo = TestRepo::new();
    repo.init();

    let temp_bin = repo.home().join("tmp/orbit-temp/orbit");
    std::fs::create_dir_all(temp_bin.parent().unwrap()).unwrap();
    std::fs::write(&temp_bin, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&temp_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&temp_bin, permissions).unwrap();
    }

    let (status, value) = repo.json(&[
        "setup",
        "claude",
        "--mcp-bin",
        temp_bin.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "EPHEMERAL_BIN");
}

#[test]
fn link_reports_a_vanished_board() {
    let repo = TestRepo::new();
    repo.init();
    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");
    repo.init_at(&board);
    repo.link_at(&board);

    std::fs::remove_dir_all(&board).unwrap();

    let (status, value) = repo.json(&["list", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "LINK_NOT_FOUND");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("re-link")
    );
}

#[test]
fn init_reports_unchanged_artifacts_on_a_second_run() {
    let repo = TestRepo::new();
    repo.ok_json(&["init", "--agents", "claude", "--json"]);

    let second = repo.ok_json(&["init", "--agents", "claude", "--json"]);
    let files = second["data"]["files"].as_array().unwrap();

    assert!(!files.is_empty(), "{second}");
    assert!(
        files.iter().all(|file| file["action"] == "unchanged"),
        "{second}"
    );
    let paths = files
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect::<Vec<_>>();
    assert!(paths.contains(&"CLAUDE.md"), "{second}");
    assert!(paths.contains(&".mcp.json"), "{second}");
}

#[test]
fn init_human_report_describes_each_setup_step() {
    let repo = TestRepo::new();

    let output = repo.orbit(&["init", "--agents", "claude", "--mcp-bin", "orbit"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    for expected in [
        "Orbit setup complete.",
        "1. Board (created)",
        "storage  worktree",
        "exclude  /.orbit/",
        "2. Coding agents",
        "wired    claude",
        "mcp      orbit mcp (portable",
        "CLAUDE.md",
        ".mcp.json",
        "3. Verify",
        "orbit doctor",
        "Next steps:",
        "orbit add \"First task\" -p 1",
        "orbit ready --claim --by claude",
    ] {
        assert!(
            stdout.contains(expected),
            "missing {expected:?} in:\n{stdout}"
        );
    }
}

#[test]
fn init_preview_says_nothing_was_written_and_uses_future_tense() {
    let repo = TestRepo::new();

    let output = repo.orbit(&["init", "--agents", "claude", "--print"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("Orbit setup preview (nothing was written)."),
        "{stdout}"
    );
    assert!(stdout.contains("1. Board (would create)"), "{stdout}");
    assert!(stdout.contains("would create CLAUDE.md"), "{stdout}");
    assert!(
        stdout.contains("Re-run the same command without --print"),
        "{stdout}"
    );
    assert!(!repo.exists("CLAUDE.md"), "preview wrote a file");
    assert!(!repo.exists(".mcp.json"), "preview wrote a file");
}

#[test]
fn init_detects_a_harness_from_path() {
    let repo = TestRepo::new();
    repo.fake_command("cursor");

    let value = repo.ok_json(&["init", "--agents", "auto", "--json"]);

    assert_eq!(value["data"]["agents"], serde_json::json!(["cursor"]));
    assert!(repo.exists(".cursor/mcp.json"));
    assert!(!repo.exists(".mcp.json"));
}

#[test]
fn init_dir_creates_an_external_board_without_relinking() {
    let repo = TestRepo::new();
    let shared = tempfile::tempdir().unwrap();
    let board = shared.path().join("board");

    let value = repo.ok_json(&[
        "init",
        board.to_str().unwrap(),
        "--agents",
        "none",
        "--json",
    ]);
    assert_eq!(value["data"]["storage"], "repository");
    assert!(!repo.path().join(".git/orbit/link").exists());

    let listed = repo.ok_json(&["--dir", board.to_str().unwrap(), "list", "--json"]);
    assert_eq!(listed["data"]["count"], 0);
}
