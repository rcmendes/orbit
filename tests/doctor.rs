mod common;

use common::TestRepo;
use std::path::{Path, PathBuf};

fn trace_sources(value: &serde_json::Value) -> Vec<String> {
    value["data"]["discovery"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["source"].as_str().unwrap().to_string())
        .collect()
}

fn doctor_json(repo: &TestRepo, args: &[&str]) -> serde_json::Value {
    let (status, value) = repo.json(args);
    assert!(
        status == 0 || status == 1,
        "unexpected status {status}: {value}"
    );
    assert_eq!(value["ok"], true, "{value}");
    value
}

fn issue_codes(value: &serde_json::Value) -> Vec<String> {
    value["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|issue| issue["code"].as_str().map(str::to_string))
        .collect()
}

fn write_absolute_board_config(path: &Path) {
    let board = tempfile::tempdir().unwrap();
    let value = serde_json::json!({
        "mcpServers": {
            "orbit": {
                "command": "orbit",
                "args": ["--dir", board.path().to_str().unwrap(), "mcp"]
            }
        }
    });
    std::fs::write(path, value.to_string()).unwrap();
}

fn write_legacy_board_config(board: &Path, prefix: &str) {
    std::fs::create_dir_all(board.join("tasks")).unwrap();
    std::fs::create_dir_all(board.join("comments")).unwrap();
    std::fs::write(
        board.join("config.toml"),
        format!("[board]\nbranch = \"orbit\"\n\n[id]\nprefix = \"{prefix}\"\n"),
    )
    .unwrap();
}

fn migrate_hidden_board(repo: &TestRepo) {
    let old = repo.legacy_board_path();
    let new = repo.board_path();
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    let moved = repo.git(&[
        "worktree",
        "move",
        new.to_str().unwrap(),
        old.to_str().unwrap(),
    ]);
    assert!(
        moved.status.success(),
        "{}",
        String::from_utf8_lossy(&moved.stderr)
    );
    let config = std::fs::read_to_string(old.join("config.toml")).unwrap();
    std::fs::write(
        old.join("config.toml"),
        config.replace("storage = \"worktree\"\n", ""),
    )
    .unwrap();
    for args in [
        &["add", "config.toml"][..],
        &["commit", "-q", "-m", "legacy config"][..],
    ] {
        let output = repo.git_in(&old, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn doctor_runs_without_a_board_and_reports_the_discovery_trace() {
    let repo = TestRepo::new();

    let value = doctor_json(&repo, &["doctor", "--json"]);

    assert_eq!(value["data"]["healthy"], true);
    assert_eq!(value["data"]["tasks"], 0);
    assert!(value["data"]["issues"].as_array().unwrap().is_empty());
    assert!(value["data"]["discovery"]["root"].is_null());
    assert_eq!(value["data"]["discovery"]["source"], "none");
    let sources = trace_sources(&value);
    assert!(sources.contains(&"link".to_string()), "{sources:?}");
    assert!(sources.contains(&"nearest".to_string()), "{sources:?}");
    assert!(
        sources.contains(&"main_checkout".to_string()),
        "{sources:?}"
    );
    assert!(sources.contains(&"legacy_local".to_string()), "{sources:?}");
    assert!(sources.contains(&"global".to_string()), "{sources:?}");
}

#[test]
fn doctor_reports_legacy_layout_and_migrates_a_hidden_board() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Legacy task", &[]);
    migrate_hidden_board(&repo);

    let before = doctor_json(&repo, &["doctor", "--json"]);
    assert!(issue_codes(&before).contains(&"LEGACY_LAYOUT".to_string()));

    let value = doctor_json(&repo, &["doctor", "--migrate", "--json"]);

    assert_eq!(value["data"]["migration"]["migrated"], true);
    assert_eq!(value["data"]["migration"]["resumed"], false);
    assert_eq!(value["data"]["migration"]["storage"], "worktree");
    assert_eq!(value["data"]["migration"]["tasks"], 1);
    assert!(repo.board_path().join("config.toml").is_file());
    assert!(!repo.legacy_board_path().exists());
    let listed = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}

#[test]
fn doctor_migrates_a_legacy_workspace_and_updates_the_pointer() {
    let creator = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    let board = workspace.join("board");
    creator.ok_json_in(
        parent.path(),
        &["init", "acme/board", "--agents", "none", "--json"],
    );
    let id = creator.ok_json(&["--dir", board.to_str().unwrap(), "add", "Shared", "--json"])
        ["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let config = std::fs::read_to_string(board.join("config.toml")).unwrap();
    std::fs::write(
        board.join("config.toml"),
        config.replace("storage = \"repository\"\n", ""),
    )
    .unwrap();

    let joiner = TestRepo::new();
    let link = joiner.path().join(".git/orbit/link");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::fs::write(&link, format!("{}\n", workspace.display())).unwrap();

    let value = doctor_json(&joiner, &["doctor", "--migrate", "--json"]);

    assert_eq!(value["data"]["migration"]["migrated"], true);
    assert_eq!(value["data"]["migration"]["storage"], "repository");
    assert!(
        value["data"]["migration"]["notes"][0]
            .as_str()
            .unwrap()
            .contains("orbit link"),
        "{value}"
    );
    assert!(workspace.join(".orbit/board/config.toml").is_file());
    assert!(workspace.join(".orbit/cache").is_dir());
    assert!(!board.exists());
    let pointer = std::fs::read_to_string(&link).unwrap();
    assert_eq!(
        pointer.trim(),
        dunce::canonicalize(workspace.join(".orbit/board"))
            .unwrap()
            .to_str()
            .unwrap()
    );
    let listed = joiner.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}

#[test]
fn doctor_migrate_resumes_an_interrupted_hidden_board_move() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Resumed task", &[]);
    migrate_hidden_board(&repo);
    let old = repo.legacy_board_path();
    let new = repo.board_path();
    let moved = repo.git(&[
        "worktree",
        "move",
        old.to_str().unwrap(),
        new.to_str().unwrap(),
    ]);
    assert!(
        moved.status.success(),
        "{}",
        String::from_utf8_lossy(&moved.stderr)
    );

    write_legacy_board_config(&old, "orb-");
    std::fs::copy(
        new.join("tasks").join(format!("{id}.md")),
        old.join("tasks").join(format!("{id}.md")),
    )
    .unwrap();

    let value = doctor_json(&repo, &["doctor", "--migrate", "--json"]);

    assert_eq!(value["data"]["migration"]["migrated"], true);
    assert_eq!(value["data"]["migration"]["resumed"], true);
    assert!(!old.exists());
    assert!(new.join("config.toml").is_file());
    let listed = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}

#[test]
fn doctor_reports_ambiguous_boards() {
    let repo = TestRepo::new();
    repo.init();
    write_legacy_board_config(&repo.legacy_board_path(), "orb-");

    let value = doctor_json(&repo, &["doctor", "--json"]);

    assert!(issue_codes(&value).contains(&"AMBIGUOUS_BOARD".to_string()));
}

#[test]
fn doctor_reports_and_fixes_an_orphan_worktree() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Orphaned task", &[]);
    std::fs::remove_dir_all(repo.board_path()).unwrap();

    let before = doctor_json(&repo, &["doctor", "--json"]);
    assert!(issue_codes(&before).contains(&"ORPHAN_WORKTREE".to_string()));

    let value = doctor_json(&repo, &["doctor", "--fix", "--json"]);
    assert!(
        value["data"]["fixed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fixed| fixed.as_str().unwrap().contains("reattached"))
    );
    assert!(repo.board_path().join("config.toml").is_file());
    let listed = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}

#[test]
fn doctor_fix_reattaches_a_legacy_orphan_at_its_original_path() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Legacy orphan", &[]);
    migrate_hidden_board(&repo);
    std::fs::remove_dir_all(repo.legacy_board_path()).unwrap();

    let before = doctor_json(&repo, &["doctor", "--json"]);
    assert!(issue_codes(&before).contains(&"ORPHAN_WORKTREE".to_string()));

    doctor_json(&repo, &["doctor", "--fix", "--json"]);

    assert!(repo.legacy_board_path().join("config.toml").is_file());
    assert!(!repo.board_path().exists());
}

#[test]
fn doctor_reports_storage_mismatch() {
    let repo = TestRepo::new();
    repo.init();
    let config = repo.read(".orbit/config.toml");
    std::fs::write(
        repo.path().join(".orbit/config.toml"),
        config.replace("storage = \"worktree\"", "storage = \"repository\""),
    )
    .unwrap();

    let value = doctor_json(&repo, &["doctor", "--json"]);

    assert!(issue_codes(&value).contains(&"STORAGE_MISMATCH".to_string()));
}

#[test]
fn doctor_reports_absolute_board_paths_in_manifests() {
    let repo = TestRepo::new();
    repo.init();
    write_absolute_board_config(&repo.path().join(".mcp.json"));

    let value = doctor_json(&repo, &["doctor", "--json"]);

    assert!(issue_codes(&value).contains(&"ABSOLUTE_BOARD_PATH".to_string()));
}

#[test]
fn doctor_reports_absolute_paths_in_user_scope_configs() {
    let repo = TestRepo::new();
    repo.init();
    write_absolute_board_config(&repo.home().join(".claude.json"));

    let value = doctor_json(&repo, &["doctor", "--json"]);

    assert!(issue_codes(&value).contains(&"ABSOLUTE_BOARD_PATH".to_string()));
}

#[test]
fn doctor_migrate_reports_when_there_is_nothing_to_move() {
    let repo = TestRepo::new();
    repo.init();

    let value = doctor_json(&repo, &["doctor", "--migrate", "--json"]);

    assert_eq!(value["data"]["migration"]["migrated"], false);
    assert!(
        value["data"]["migration"]["reason"]
            .as_str()
            .unwrap()
            .contains("no legacy")
    );
}

#[test]
fn doctor_migrate_refuses_when_the_legacy_board_diverged() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Diverged task", &[]);
    migrate_hidden_board(&repo);
    let old = repo.legacy_board_path();
    let new = repo.board_path();
    let moved = repo.git(&[
        "worktree",
        "move",
        old.to_str().unwrap(),
        new.to_str().unwrap(),
    ]);
    assert!(moved.status.success());

    write_legacy_board_config(&old, "orb-");
    let task = std::fs::read_to_string(new.join("tasks").join(format!("{id}.md"))).unwrap();
    std::fs::write(
        old.join("tasks").join(format!("{id}.md")),
        format!("{task}\nedited only in the legacy copy\n"),
    )
    .unwrap();

    let (status, value) = repo.json(&["doctor", "--migrate", "--json"]);

    assert_ne!(status, 0, "{value}");
    assert_eq!(value["error"]["code"], "MIGRATION_BLOCKED", "{value}");
    assert!(old.join("tasks").join(format!("{id}.md")).is_file());
}

#[test]
fn commands_reattach_an_orphan_worktree_on_their_own() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Auto repaired", &[]);
    std::fs::remove_dir_all(repo.board_path()).unwrap();

    let listed = repo.ok_json(&["list", "--all", "--json"]);

    assert_eq!(listed["data"]["tasks"][0]["id"], id);
    assert!(repo.board_path().join("config.toml").is_file());
}

fn workspace_with_github(repo: &TestRepo) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    repo.init_at(&workspace);
    let config_path = workspace.join(".orbit/board/config.toml");
    let mut config: toml::Value =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    let root = config.as_table_mut().unwrap();
    let github = root
        .entry("github")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .unwrap();
    github.insert(
        "organization".to_string(),
        toml::Value::String("acme".to_string()),
    );
    let fields = github
        .entry("issue_fields")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .unwrap();
    fields.insert(
        "area_id".to_string(),
        toml::Value::String("AREA_ID".to_string()),
    );
    fields.insert(
        "verification_id".to_string(),
        toml::Value::String("VERIFICATION_ID".to_string()),
    );
    std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    (dir, workspace)
}

fn workspace_child(repo: &TestRepo, workspace: &Path, folder: &str) -> PathBuf {
    let path = workspace.join(folder);
    std::fs::create_dir_all(&path).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Test"][..],
        &["config", "user.email", "test@example.com"][..],
        &["commit", "--allow-empty", "-q", "-m", "init"][..],
        &[
            "remote",
            "add",
            "origin",
            &format!("https://github.com/acme/{folder}.git"),
        ][..],
    ] {
        let output = repo.git_in(&path, args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    path
}

fn base_workspace_fixture() -> serde_json::Value {
    serde_json::json!({
        "authenticated": true,
        "auth_scopes": ["repo", "read:org"],
        "repositories": {
            "acme/api": { "owner_type": "Organization", "accessible": true }
        },
        "org_fields": {
            "acme/Area": "AREA_ID",
            "acme/Verification": "VERIFICATION_ID"
        },
        "issues": {
            "acme/api#1": {
                "repo": "acme/api",
                "number": 1,
                "title": "Catalog issue",
                "body": "Remote body",
                "state": "open",
                "labels": ["bug"],
                "assignees": [],
                "updated_at": "2026-10-23T13:00:00Z"
            }
        },
        "catalog_issues": {
            "acme/api": [
                {
                    "number": 1,
                    "title": "Catalog issue",
                    "state": "open",
                    "labels": ["bug"]
                }
            ]
        }
    })
}

fn run_workspace(
    repo: &TestRepo,
    cwd: &Path,
    workspace: &Path,
    args: &[&str],
    fixture: &Path,
) -> (i32, serde_json::Value) {
    let mut command = vec!["--dir", workspace.to_str().unwrap()];
    command.extend_from_slice(args);
    command.push("--json");
    repo.json_in_with_env(
        cwd,
        &command,
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    )
}

fn workspace_doctor(
    repo: &TestRepo,
    cwd: &Path,
    workspace: &Path,
    fixture: &Path,
) -> serde_json::Value {
    let (status, value) = run_workspace(repo, cwd, workspace, &["doctor"], fixture);
    assert!(status == 0 || status == 1, "{value}");
    assert_eq!(value["ok"], true, "{value}");
    value
}

fn prepare_workspace(repo: &TestRepo) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let (dir, workspace) = workspace_with_github(repo);
    let child = workspace_child(repo, &workspace, "api");
    let fixture = repo.home().join("github.json");
    std::fs::write(
        &fixture,
        serde_json::to_vec(&base_workspace_fixture()).unwrap(),
    )
    .unwrap();
    let (status, value) = run_workspace(
        repo,
        &workspace,
        &workspace,
        &["workspace", "refresh"],
        &fixture,
    );
    assert_eq!(status, 0, "{value}");
    let (status, value) = run_workspace(
        repo,
        &workspace,
        &workspace,
        &["workspace", "issues", "refresh"],
        &fixture,
    );
    assert_eq!(status, 0, "{value}");
    (dir, workspace, child, fixture)
}

#[test]
fn doctor_resolves_a_healthy_workspace_from_its_root_and_child_repository() {
    let repo = TestRepo::new();
    let (_dir, workspace, child, fixture) = prepare_workspace(&repo);

    let root = workspace_doctor(&repo, &workspace, &workspace, &fixture);
    assert_eq!(root["data"]["healthy"], true, "{root}");
    assert_eq!(root["data"]["workspace"]["included_repositories"], 1);
    assert_eq!(
        root["data"]["workspace"]["root"].as_str().unwrap(),
        dunce::canonicalize(&workspace).unwrap().to_str().unwrap()
    );
    assert!(
        std::path::Path::new(root["data"]["workspace"]["board"].as_str().unwrap())
            .ends_with(std::path::Path::new(".orbit").join("board"))
    );

    let nested = workspace_doctor(&repo, &child, &workspace, &fixture);
    assert_eq!(nested["data"]["healthy"], true, "{nested}");
    assert_eq!(
        nested["data"]["workspace"]["root"],
        root["data"]["workspace"]["root"]
    );
}

#[test]
fn doctor_treats_plain_non_git_child_directories_as_warnings() {
    let repo = TestRepo::new();
    let (_dir, workspace, _child, fixture) = prepare_workspace(&repo);
    std::fs::create_dir_all(workspace.join("notes")).unwrap();
    let (status, refreshed) = run_workspace(
        &repo,
        &workspace,
        &workspace,
        &["workspace", "refresh"],
        &fixture,
    );
    assert_eq!(status, 0, "{refreshed}");

    let doctor = workspace_doctor(&repo, &workspace, &workspace, &fixture);
    assert_eq!(doctor["data"]["healthy"], true, "{doctor}");
    assert!(
        !issue_codes(&doctor).contains(&"WORKSPACE_NON_GIT_DIRECTORY".to_string()),
        "{doctor}"
    );
    assert!(
        doctor["data"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("notes")),
        "{doctor}"
    );

    let status = repo.ok_json_in(&workspace, &["workspace", "status", "--json"]);
    assert!(
        status["data"]["non_git_directories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|directory| directory == "notes"),
        "{status}"
    );
}

#[test]
fn doctor_reports_malformed_workspace_layout_without_creating_a_board() {
    let repo = TestRepo::new();
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    std::fs::create_dir_all(workspace.join(".orbit/cache")).unwrap();
    let fixture = repo.home().join("github.json");
    std::fs::write(&fixture, "{}").unwrap();

    let (status, value) = repo.json_in_with_env(
        workspace,
        &["--dir", workspace.to_str().unwrap(), "doctor", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    assert_eq!(status, 1, "{value}");
    assert!(
        issue_codes(&value).contains(&"WORKSPACE_LAYOUT".to_string()),
        "{value}"
    );
    assert_eq!(
        value["data"]["workspace"]["root"].as_str().unwrap(),
        dunce::canonicalize(workspace).unwrap().to_str().unwrap()
    );
    assert!(!workspace.join(".orbit/board").exists());
}

#[test]
fn doctor_reports_unsupported_origin_and_repository_owner() {
    let repo = TestRepo::new();
    let (_dir, workspace, _child, fixture) = prepare_workspace(&repo);
    let gitlab = workspace_child(&repo, &workspace, "gitlab-project");
    let output = repo.git_in(
        &gitlab,
        &[
            "remote",
            "set-url",
            "origin",
            "git@gitlab.com:acme/project.git",
        ],
    );
    assert!(output.status.success());
    let outside = workspace_child(&repo, &workspace, "outside");
    let output = repo.git_in(
        &outside,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/someone/outside.git",
        ],
    );
    assert!(output.status.success());
    let (status, value) = run_workspace(
        &repo,
        &workspace,
        &workspace,
        &["workspace", "refresh"],
        &fixture,
    );
    assert_eq!(status, 0, "{value}");

    let doctor = workspace_doctor(&repo, &workspace, &workspace, &fixture);
    let codes = issue_codes(&doctor);
    assert!(
        codes.contains(&"WORKSPACE_UNSUPPORTED_ORIGIN".to_string()),
        "{doctor}"
    );
    assert!(
        codes.contains(&"WORKSPACE_REPOSITORY_UNSUPPORTED".to_string()),
        "{doctor}"
    );
}

#[test]
fn doctor_reports_a_missing_child_repository_without_deleting_its_cache() {
    let repo = TestRepo::new();
    let (_dir, workspace, child, fixture) = prepare_workspace(&repo);
    std::fs::remove_dir_all(&child).unwrap();

    let value = workspace_doctor(&repo, &workspace, &workspace, &fixture);

    let codes = issue_codes(&value);
    assert!(
        codes.contains(&"WORKSPACE_REPOSITORY_MISSING".to_string()),
        "{value}"
    );
    let issue = value["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == "WORKSPACE_REPOSITORY_MISSING")
        .unwrap();
    assert_eq!(issue["severity"], "error");
    assert_eq!(issue["repository"], "acme/api");
    assert!(
        issue["next_action"]
            .as_str()
            .unwrap()
            .contains("workspace refresh")
    );
    assert!(workspace.join(".orbit/cache/issues.json").is_file());
}

#[test]
fn doctor_reports_stale_repository_access_and_stale_issue_cache_separately() {
    let repo = TestRepo::new();
    let (_dir, workspace, _child, fixture) = prepare_workspace(&repo);
    let mut next = base_workspace_fixture();
    next["repositories"]["acme/api"]["accessible"] = serde_json::json!(false);
    next["catalog_errors"] = serde_json::json!({
        "acme/api": "catalog unavailable during refresh"
    });
    std::fs::write(&fixture, serde_json::to_vec(&next).unwrap()).unwrap();
    let (status, value) = run_workspace(
        &repo,
        &workspace,
        &workspace,
        &["workspace", "refresh"],
        &fixture,
    );
    assert_eq!(status, 0, "{value}");
    let (status, value) = run_workspace(
        &repo,
        &workspace,
        &workspace,
        &["workspace", "issues", "refresh"],
        &fixture,
    );
    assert_eq!(status, 0, "{value}");

    let doctor = workspace_doctor(&repo, &workspace, &workspace, &fixture);

    let codes = issue_codes(&doctor);
    assert!(
        codes.contains(&"WORKSPACE_REPOSITORY_STALE".to_string()),
        "{doctor}"
    );
    assert!(
        codes.contains(&"WORKSPACE_ISSUES_STALE".to_string()),
        "{doctor}"
    );
    assert_eq!(doctor["data"]["healthy"], false);
}

#[test]
fn doctor_reports_duplicate_github_identity_and_orphaned_remote_issue_links() {
    let repo = TestRepo::new();
    let (_dir, workspace, _child, fixture) = prepare_workspace(&repo);
    let first = repo.ok_json_in(&workspace, &["add", "First", "--json"])["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let second = repo.ok_json_in(&workspace, &["add", "Second", "--json"])["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tasks = workspace.join(".orbit/board/tasks");
    let first_path = tasks.join(format!("{first}.md"));
    let mut content = std::fs::read_to_string(&first_path).unwrap();
    content = content.replace(
        "created_at:",
        "github:\n  repo: acme/api\n  number: 1\n  last_sync:\n    at: 2026-10-23T13:00:00Z\n    issue_updated_at: 2026-10-23T13:00:00Z\n    orbit_updated_at: 2026-10-23T12:00:00Z\n    title: Catalog issue\n    body: \"\"\n    labels:\n    - bug\n    status: todo\n    issue_state: open\ncreated_at:",
    );
    std::fs::write(&first_path, content).unwrap();
    let second_path = tasks.join(format!("{second}.md"));
    let mut content = std::fs::read_to_string(&second_path).unwrap();
    content = content.replace(
        "created_at:",
        "github:\n  repo: acme/api\n  number: 1\ncreated_at:",
    );
    std::fs::write(&second_path, content).unwrap();

    let duplicate = workspace_doctor(&repo, &workspace, &workspace, &fixture);
    let codes = issue_codes(&duplicate);
    assert!(
        codes.contains(&"GITHUB_DUPLICATE_LINK".to_string()),
        "{duplicate}"
    );

    let cache_path = workspace.join(".orbit/cache/issues.json");
    let mut cache: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cache_path).unwrap()).unwrap();
    cache["repositories"]["acme/api"]["issues"] = serde_json::json!([]);
    std::fs::write(&cache_path, serde_json::to_vec_pretty(&cache).unwrap()).unwrap();

    let orphan = workspace_doctor(&repo, &workspace, &workspace, &fixture);
    let codes = issue_codes(&orphan);
    assert!(
        codes.contains(&"GITHUB_ORPHAN_LINK".to_string()),
        "{orphan}"
    );
}

#[test]
fn doctor_reports_field_level_sync_conflicts_from_the_local_baseline() {
    let repo = TestRepo::new();
    let (_dir, workspace, _child, fixture) = prepare_workspace(&repo);
    let imported = repo.ok_json_in_with_env(
        &workspace,
        &[
            "--dir",
            workspace.to_str().unwrap(),
            "workspace",
            "issues",
            "import",
            "acme/api#1",
            "--apply",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let task = imported["data"]["imported"][0]["task"]
        .as_str()
        .unwrap()
        .to_string();
    repo.ok_json_in(
        &workspace,
        &["update", &task, "--title", "Local title", "--json"],
    );

    let cache_path = workspace.join(".orbit/cache/issues.json");
    let mut cache: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cache_path).unwrap()).unwrap();
    cache["repositories"]["acme/api"]["issues"][0]["title"] = serde_json::json!("Remote title");
    std::fs::write(&cache_path, serde_json::to_vec_pretty(&cache).unwrap()).unwrap();

    let value = workspace_doctor(&repo, &workspace, &workspace, &fixture);

    let conflict = value["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == "GITHUB_SYNC_CONFLICT")
        .unwrap_or_else(|| panic!("missing sync conflict: {value}"));
    assert_eq!(conflict["task"], task);
    assert_eq!(conflict["issue"], "acme/api#1");
    assert!(conflict["cause"].as_str().unwrap().contains("title"));
}

#[test]
fn doctor_reports_interrupted_migration_and_local_board_remnants() {
    let repo = TestRepo::new();
    let (_dir, workspace, _child, fixture) = prepare_workspace(&repo);
    std::fs::write(
        workspace.join(".orbit/workspace-migration-state.json"),
        "{}",
    )
    .unwrap();
    let old = workspace.join(".board");
    std::fs::create_dir_all(old.join("tasks")).unwrap();
    std::fs::write(
        old.join("config.toml"),
        "[board]\nstorage = \"repository\"\n\n[id]\nprefix = \"old-\"\n",
    )
    .unwrap();

    let value = workspace_doctor(&repo, &workspace, &workspace, &fixture);

    let codes = issue_codes(&value);
    assert!(
        codes.contains(&"WORKSPACE_MIGRATION_INCOMPLETE".to_string()),
        "{value}"
    );
    assert!(
        value["data"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("local Board at")),
        "{value}"
    );
    let issue = value["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == "WORKSPACE_MIGRATION_INCOMPLETE")
        .unwrap();
    assert!(issue["next_action"].as_str().unwrap().contains("--resume"));
}

#[test]
fn doctor_reports_managed_agent_setup_drift_for_workspace_repositories() {
    let repo = TestRepo::new();
    let (_dir, workspace, _child, fixture) = prepare_workspace(&repo);
    let config_path = workspace.join(".orbit/board/config.toml");
    let mut config: toml::Value =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    let workspace_config = config
        .as_table_mut()
        .unwrap()
        .entry("workspace")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .unwrap();
    workspace_config.insert(
        "agent_harnesses".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("claude".to_string()),
            toml::Value::String("opencode".to_string()),
        ]),
    );
    std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();

    let value = workspace_doctor(&repo, &workspace, &workspace, &fixture);

    let drift = value["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|issue| issue["code"] == "WORKSPACE_AGENT_DRIFT")
        .collect::<Vec<_>>();
    assert!(!drift.is_empty(), "{value}");
    assert!(
        drift
            .iter()
            .any(|issue| issue["repository"].as_str() == Some("acme/api")),
        "{value}"
    );
    assert!(
        drift.iter().any(|issue| {
            issue["next_action"]
                .as_str()
                .is_some_and(|action| action.contains("workspace setup"))
        }),
        "{value}"
    );
    // Doctor must not create managed files.
    assert!(!workspace.join("acme/api/CLAUDE.md").exists());
    assert!(!workspace.join("acme/api/opencode.json").exists());
}

#[test]
fn doctor_fix_honours_dir_from_another_directory() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Elsewhere", &[]);
    let board = repo.board_path();
    std::fs::remove_dir_all(&board).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();

    let (status, value) = repo.json_in(
        elsewhere.path(),
        &[
            "--dir",
            board.to_str().unwrap(),
            "doctor",
            "--fix",
            "--json",
        ],
    );

    assert!(status == 0 || status == 1, "{value}");
    assert!(board.join("config.toml").is_file(), "{value}");
    let listed = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}
