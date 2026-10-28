mod common;

use common::TestRepo;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn workspace(repo: &TestRepo) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    repo.init_at(&root);
    let config = root.join(".orbit/board/config.toml");
    let mut contents = std::fs::read_to_string(&config).unwrap();
    contents.push_str("\n[github]\norganization = \"acme\"\n");
    std::fs::write(config, contents).unwrap();

    let child = root.join("api");
    std::fs::create_dir_all(&child).unwrap();
    for args in [
        &(["init", "-q"][..]),
        &(["config", "user.name", "Test"][..]),
        &(["config", "user.email", "test@example.com"][..]),
        &(["commit", "--allow-empty", "-q", "-m", "init"][..]),
        &(["remote", "add", "origin", "https://github.com/acme/api.git"][..]),
    ] {
        let output = repo.git_in(&child, args);
        assert!(output.status.success(), "git {args:?} failed");
    }
    (dir, root, child)
}

fn refresh(repo: &TestRepo, root: &Path) {
    let fixture = repo.home().join("github.json");
    std::fs::write(
        &fixture,
        json!({"repositories": {"acme/api": {"owner_type": "Organization", "accessible": true}}})
            .to_string(),
    )
    .unwrap();
    let status = repo.json_with_env(
        &[
            "--dir",
            root.to_str().unwrap(),
            "workspace",
            "refresh",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status.0, 0, "{}", status.1);
}

fn setup(repo: &TestRepo, root: &Path, args: &[&str]) -> (i32, Value) {
    let mut command = vec!["--dir", root.to_str().unwrap(), "workspace", "setup"];
    command.extend_from_slice(args);
    command.push("--json");
    repo.json(&command)
}

#[test]
fn workspace_setup_installs_selected_harnesses_in_children_and_is_idempotent() {
    let repo = TestRepo::new();
    let (_dir, root, child) = workspace(&repo);
    std::fs::write(root.join("AGENTS.md"), "# Workspace rules\n\nKeep this.\n").unwrap();
    std::fs::write(root.join("CLAUDE.md"), "# Claude rules\n\nKeep this too.\n").unwrap();
    let workspace_mcp = r#"{"mcpServers":{"custom":{"command":"custom-server"}}}"#;
    std::fs::write(root.join(".mcp.json"), workspace_mcp).unwrap();
    let nested = child.join("tools");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(repo.git_in(&nested, &["init", "-q"]).status.success());
    refresh(&repo, &root);
    let task = repo.ok_json_in(&root, &["add", "Shared workspace task", "--json"]);
    let task_id = task["data"]["id"].as_str().unwrap();
    let from_nested = repo.ok_json_in(&nested, &["show", task_id, "--json"]);
    assert_eq!(from_nested["data"]["title"], "Shared workspace task");

    let (status, preview) = setup(&repo, &root, &["--agents", "all", "--print"]);
    assert_eq!(status, 0, "{preview}");
    assert_eq!(preview["data"]["preview"], true);
    assert!(!child.join("AGENTS.md").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
        "# Workspace rules\n\nKeep this.\n"
    );

    let (status, result) = setup(&repo, &root, &["--agents", "all"]);
    assert_eq!(status, 0, "{result}");
    assert_eq!(
        result["data"]["agents"],
        json!(["claude", "cursor", "codex", "opencode"])
    );
    for path in [
        "CLAUDE.md",
        "AGENTS.md",
        ".mcp.json",
        ".cursor/rules/orbit.mdc",
        ".cursor/mcp.json",
        ".agents/skills/orbit/SKILL.md",
        ".codex/config.toml",
        "opencode.json",
    ] {
        assert!(
            child.join(path).exists(),
            "missing child integration {path}"
        );
    }
    for path in ["AGENTS.md", "CLAUDE.md"] {
        assert!(
            root.join(path).exists(),
            "missing Workspace entry point {path}"
        );
    }
    assert!(
        std::fs::read_to_string(root.join("AGENTS.md"))
            .unwrap()
            .contains("Keep this.")
    );
    assert!(
        std::fs::read_to_string(root.join("CLAUDE.md"))
            .unwrap()
            .contains("Keep this too.")
    );
    assert!(!nested.join("AGENTS.md").exists());

    let contents = std::fs::read_to_string(child.join("AGENTS.md")).unwrap();
    assert!(contents.contains("BEGIN ORBIT INTEGRATION"));
    assert!(!contents.contains(root.to_str().unwrap()));
    for path in [
        ".mcp.json",
        ".cursor/mcp.json",
        ".codex/config.toml",
        "opencode.json",
    ] {
        let contents = std::fs::read_to_string(child.join(path)).unwrap();
        assert!(
            !contents.contains(root.to_str().unwrap()),
            "Board path leaked to {path}"
        );
    }
    let first_agents = contents;
    let first_claude = std::fs::read_to_string(child.join("CLAUDE.md")).unwrap();
    let first_mcp = std::fs::read_to_string(child.join(".mcp.json")).unwrap();

    let (status, rerun) = setup(&repo, &root, &[]);
    assert_eq!(status, 0, "{rerun}");
    assert_eq!(
        std::fs::read_to_string(child.join("AGENTS.md")).unwrap(),
        first_agents
    );
    assert_eq!(
        std::fs::read_to_string(child.join("CLAUDE.md")).unwrap(),
        first_claude
    );
    assert_eq!(
        std::fs::read_to_string(child.join(".mcp.json")).unwrap(),
        first_mcp
    );

    let child_agents = child.join("AGENTS.md");
    std::fs::write(&child_agents, format!("# Repo rules\n\n{first_agents}")).unwrap();
    let root_claude = root.join("CLAUDE.md");
    let workspace_claude = std::fs::read_to_string(&root_claude).unwrap();
    std::fs::write(
        &root_claude,
        format!("# Workspace rules\n\n{workspace_claude}"),
    )
    .unwrap();
    let mut exclude = vec![
        "--dir",
        root.to_str().unwrap(),
        "workspace",
        "exclude",
        "acme/api",
    ];
    exclude.push("--json");
    repo.ok_json(&exclude);
    let (status, removed) = setup(&repo, &root, &["--remove"]);
    assert_eq!(status, 0, "{removed}");
    let child_agents = std::fs::read_to_string(child_agents).unwrap();
    assert!(child_agents.contains("# Repo rules"));
    assert!(!child_agents.contains("BEGIN ORBIT INTEGRATION"));
    let root_claude = std::fs::read_to_string(root_claude).unwrap();
    assert!(root_claude.contains("# Workspace rules"));
    assert!(!root_claude.contains("BEGIN ORBIT INTEGRATION"));
    assert_eq!(
        std::fs::read_to_string(root.join(".mcp.json")).unwrap(),
        workspace_mcp
    );
    assert!(!child.join(".mcp.json").exists());
}

#[test]
fn workspace_setup_detects_harnesses_across_discovered_repositories() {
    let repo = TestRepo::new();
    let (_dir, root, child) = workspace(&repo);
    std::fs::create_dir_all(child.join(".cursor")).unwrap();
    refresh(&repo, &root);

    let (status, result) = setup(&repo, &root, &["--print"]);
    assert_eq!(status, 0, "{result}");
    assert_eq!(result["data"]["agents"], json!(["cursor"]));
    assert_eq!(result["data"]["preview"], true);
    assert!(!child.join(".cursor/rules/orbit.mdc").exists());
    assert!(!root.join("AGENTS.md").exists());
}

#[test]
fn workspace_setup_reconciles_newly_discovered_repositories_using_saved_selection() {
    let repo = TestRepo::new();
    let (_dir, root, api) = workspace(&repo);
    refresh(&repo, &root);
    let (status, result) = setup(&repo, &root, &["--agents", "all"]);
    assert_eq!(status, 0, "{result}");

    let worker = root.join("worker");
    std::fs::create_dir_all(&worker).unwrap();
    for args in [
        &(["init", "-q"][..]),
        &(["config", "user.name", "Test"][..]),
        &(["config", "user.email", "test@example.com"][..]),
        &(["commit", "--allow-empty", "-q", "-m", "init"][..]),
        &([
            "remote",
            "add",
            "origin",
            "https://github.com/acme/worker.git",
        ][..]),
    ] {
        assert!(repo.git_in(&worker, args).status.success());
    }
    let fixture = repo.home().join("github.json");
    std::fs::write(
        &fixture,
        json!({"repositories": {
            "acme/api": {"owner_type": "Organization", "accessible": true},
            "acme/worker": {"owner_type": "Organization", "accessible": true}
        }})
        .to_string(),
    )
    .unwrap();
    let refreshed = repo.json_with_env(
        &[
            "--dir",
            root.to_str().unwrap(),
            "workspace",
            "refresh",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(refreshed.0, 0, "{}", refreshed.1);

    let (status, result) = setup(&repo, &root, &[]);
    assert_eq!(status, 0, "{result}");
    for path in [
        "CLAUDE.md",
        "AGENTS.md",
        ".mcp.json",
        ".cursor/rules/orbit.mdc",
        ".cursor/mcp.json",
        ".agents/skills/orbit/SKILL.md",
        ".codex/config.toml",
        "opencode.json",
    ] {
        assert!(worker.join(path).exists(), "missing reconciled file {path}");
    }
    assert!(api.join("AGENTS.md").exists());
}

#[test]
fn workspace_setup_detects_matt_skills_separately_and_installs_tracker_guidance() {
    let repo = TestRepo::new();
    let (_dir, root, child) = workspace(&repo);
    refresh(&repo, &root);
    let skill = child.join(".agents/skills/setup-matt-pocock-skills");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "Matt Pocock skills").unwrap();
    let workspace_skill = root.join(".agents/skills/setup-matt-pocock-skills");
    std::fs::create_dir_all(&workspace_skill).unwrap();
    std::fs::write(workspace_skill.join("SKILL.md"), "Matt Pocock skills").unwrap();

    let (status, result) = setup(&repo, &root, &["--agents", "none"]);
    assert_eq!(status, 0, "{result}");
    assert!(child.join("docs/agents/issue-tracker.md").exists());
    let labels = std::fs::read_to_string(child.join("docs/agents/triage-labels.md")).unwrap();
    assert!(labels.contains("BEGIN ORBIT INTEGRATION"));
    assert!(labels.contains("`needs-triage`"));
    assert_eq!(result["data"]["agents"], json!([]));
    assert_eq!(result["data"]["mattpocock_skills"], true);
    assert_eq!(result["data"]["repositories"][0]["mattpocock_skills"], true);
    assert!(root.join("docs/agents/issue-tracker.md").exists());

    let (status, rerun) = setup(&repo, &root, &["--agents", "none"]);
    assert_eq!(status, 0, "{rerun}");
    assert_eq!(
        std::fs::read_to_string(child.join("docs/agents/triage-labels.md")).unwrap(),
        labels
    );
}

#[test]
fn workspace_setup_reports_custom_matt_guides_without_overwriting_them() {
    let repo = TestRepo::new();
    let (_dir, root, child) = workspace(&repo);
    refresh(&repo, &root);
    let skill = child.join(".agents/skills/setup-matt-pocock-skills");
    std::fs::create_dir_all(&skill).unwrap();
    let guides = [
        ("docs/agents/issue-tracker.md", "Custom tracker guidance\n"),
        ("docs/agents/triage-labels.md", "Custom triage labels\n"),
    ];
    for (path, contents) in guides {
        let path = child.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    let (status, result) = setup(&repo, &root, &["--agents", "none"]);
    assert_eq!(status, 0, "{result}");
    for (path, contents) in guides {
        assert_eq!(std::fs::read_to_string(child.join(path)).unwrap(), contents);
    }
    let warnings = result["meta"]["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 2);
    assert!(
        warnings
            .iter()
            .all(|warning| warning.as_str().unwrap().contains("not managed by Orbit"))
    );
}

#[test]
fn workspace_setup_does_not_accept_matt_skills_as_a_harness() {
    let repo = TestRepo::new();
    let (_dir, root, _) = workspace(&repo);
    refresh(&repo, &root);

    let (status, result) = setup(&repo, &root, &["--agents", "mattpocock"]);
    assert_eq!(status, 1, "{result}");
    assert_eq!(result["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("detected and configured separately")
    );
}
