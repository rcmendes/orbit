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

/// A pre-ADR-0019 hidden Board: `.orbit` itself holds the Board files — no
/// `board/` subdirectory and no standalone `.git` of its own.
fn legacy_orbit_board(repo: &TestRepo) -> PathBuf {
    let orbit = repo.orbit_path();
    repo.legacy_board_files(&orbit, "orb-");
    orbit
}

/// A pre-ADR-0019 worktree Board on branch `orbit` at `path`.
fn legacy_worktree_board(repo: &TestRepo, path: &Path) {
    repo.legacy_worktree_board(path);
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
}

#[test]
fn doctor_reports_a_legacy_board_layout() {
    let repo = TestRepo::new();
    legacy_orbit_board(&repo);

    let value = doctor_json(&repo, &["doctor", "--json"]);

    assert!(issue_codes(&value).contains(&"LEGACY_LAYOUT".to_string()));

    // Commands refuse the pre-ADR-0019 Board until `orbit migrate` converts it.
    let (status, listed) = repo.json(&["list", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(listed["error"]["code"], "BOARD_NOT_MIGRATED");
}

#[test]
fn doctor_reports_ambiguous_boards() {
    let repo = TestRepo::new();
    repo.init();
    repo.legacy_board_files(&repo.legacy_board_path(), "orb-");

    let value = doctor_json(&repo, &["doctor", "--json"]);

    assert!(issue_codes(&value).contains(&"AMBIGUOUS_BOARD".to_string()));
}

#[test]
fn doctor_reports_and_fixes_an_orphan_worktree() {
    let repo = TestRepo::new();
    let board = repo.orbit_path();
    legacy_worktree_board(&repo, &board);
    std::fs::write(
        board.join("tasks").join("orb-a7b2c9.md"),
        "---\ntitle: Orphaned task\nstatus: todo\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody\n",
    )
    .unwrap();
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", "task"][..]] {
        let output = repo.git_in(&board, args);
        assert!(output.status.success());
    }
    std::fs::remove_dir_all(&board).unwrap();

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
    assert!(board.join("config.toml").is_file());
    assert!(board.join("tasks").join("orb-a7b2c9.md").is_file());
}

#[test]
fn doctor_fix_reattaches_a_legacy_orphan_at_its_original_path() {
    let repo = TestRepo::new();
    let board = repo.legacy_board_path();
    legacy_worktree_board(&repo, &board);
    // Git does not track empty directories; a Task file keeps `tasks/` in the
    // Board branch so the re-attached worktree is whole again.
    std::fs::write(
        board.join("tasks").join("orb-a7b2c9.md"),
        "---\ntitle: Legacy orphan\nstatus: todo\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody\n",
    )
    .unwrap();
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", "task"][..]] {
        let output = repo.git_in(&board, args);
        assert!(output.status.success());
    }
    std::fs::remove_dir_all(&board).unwrap();

    let before = doctor_json(&repo, &["doctor", "--json"]);
    assert!(issue_codes(&before).contains(&"ORPHAN_WORKTREE".to_string()));

    doctor_json(&repo, &["doctor", "--fix", "--json"]);

    assert!(board.join("config.toml").is_file());
    assert!(!repo.board_path().exists());
}

#[test]
fn doctor_reports_storage_mismatch() {
    let repo = TestRepo::new();
    repo.init();
    let config_path = repo.board_path().join("config.toml");
    let mut config: toml::Value =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    config
        .as_table_mut()
        .unwrap()
        .get_mut("board")
        .and_then(toml::Value::as_table_mut)
        .unwrap()
        .insert(
            "storage".to_string(),
            toml::Value::String("worktree".to_string()),
        );
    std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();

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
fn commands_reattach_an_orphan_worktree_on_their_own() {
    let repo = TestRepo::new();
    let board = repo.orbit_path();
    legacy_worktree_board(&repo, &board);
    std::fs::remove_dir_all(&board).unwrap();

    // Discovery repairs the missing worktree, then refuses the still-legacy
    // Board until `orbit migrate` converts it.
    let (status, value) = repo.json(&["list", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "BOARD_NOT_MIGRATED");
    assert!(board.join("config.toml").is_file());
}

#[test]
fn doctor_fix_honours_dir_from_another_directory() {
    let repo = TestRepo::new();
    let board = repo.orbit_path();
    legacy_worktree_board(&repo, &board);
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
}
