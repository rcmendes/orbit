mod common;

use common::TestRepo;
use std::path::Path;

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
    assert!(workspace.join(".orbit/config.toml").is_file());
    assert!(!board.exists());
    let pointer = std::fs::read_to_string(&link).unwrap();
    assert_eq!(
        pointer.trim(),
        dunce::canonicalize(workspace.join(".orbit"))
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
    std::fs::write(
        repo.path().join(".mcp.json"),
        r#"{"mcpServers":{"orbit":{"command":"orbit","args":["--dir","/tmp/board","mcp"]}}}"#,
    )
    .unwrap();

    let value = doctor_json(&repo, &["doctor", "--json"]);

    assert!(issue_codes(&value).contains(&"ABSOLUTE_BOARD_PATH".to_string()));
}

#[test]
fn doctor_reports_absolute_paths_in_user_scope_configs() {
    let repo = TestRepo::new();
    repo.init();
    std::fs::write(
        repo.home().join(".claude.json"),
        r#"{"mcpServers":{"orbit":{"command":"orbit","args":["--dir","/tmp/board","mcp"]}}}"#,
    )
    .unwrap();

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
