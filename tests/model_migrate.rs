mod common;

use common::TestRepo;
use std::path::{Path, PathBuf};

fn commit_all(repo: &TestRepo, cwd: &Path, message: &str) {
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", message][..]] {
        let output = repo.git_in(cwd, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Rewrites the freshly-initialized Board config into the pre-ADR-0022
/// shape: no `board.priorities`, numeric `default_priority`, `board.areas`,
/// and no `[workflow]` section.
fn make_legacy_config(board: &Path) {
    let path = board.join("config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let board_table = config
        .get_mut("board")
        .and_then(toml::Value::as_table_mut)
        .unwrap();
    board_table.remove("priorities");
    board_table.remove("components");
    board_table.remove("types");
    board_table.remove("default_type");
    board_table.remove("hierarchy");
    board_table.insert("default_priority".to_string(), toml::Value::Integer(2));
    board_table.insert(
        "areas".to_string(),
        toml::Value::Array(
            ["board", "cli", "docs"]
                .iter()
                .map(|area| toml::Value::String(area.to_string()))
                .collect(),
        ),
    );
    if let Some(table) = config.as_table_mut() {
        table.remove("workflow");
    }
    std::fs::write(&path, toml::to_string_pretty(&config).unwrap()).unwrap();
}

fn write_task(board: &Path, id: &str, content: &str) {
    std::fs::write(board.join("tasks").join(format!("{id}.md")), content).unwrap();
}

/// An initialized Board rewritten to the legacy model, committed so the
/// migration can build its temporary worktree on top.
fn legacy_board() -> (TestRepo, PathBuf) {
    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    make_legacy_config(&board);
    commit_all(&repo, &board, "legacy board");
    (repo, board)
}

#[test]
fn model_migrate_previews_without_writing() {
    let (repo, board) = legacy_board();
    write_task(
        &board,
        "orb-aaa111",
        "---\ntitle: One\nstatus: todo\npriority: 1\ntype: task\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody\n",
    );
    commit_all(&repo, &board, "task");
    let task_before = std::fs::read_to_string(board.join("tasks/orb-aaa111.md")).unwrap();
    let config_before = std::fs::read_to_string(board.join("config.toml")).unwrap();

    let value = repo.ok_json(&["migrate", "model", "--json"]);

    assert_eq!(value["data"]["migrated"], true, "{value}");
    assert_eq!(value["data"]["applied"], false, "{value}");
    assert_eq!(value["data"]["tasks"], 1, "{value}");
    assert_eq!(value["data"]["keys_allocated"], 1, "{value}");
    assert_eq!(
        std::fs::read_to_string(board.join("tasks/orb-aaa111.md")).unwrap(),
        task_before
    );
    assert_eq!(
        std::fs::read_to_string(board.join("config.toml")).unwrap(),
        config_before
    );
    assert!(!board.join("keys.toml").exists());
}

#[test]
fn model_migrate_apply_converts_the_issue_model() {
    let (repo, board) = legacy_board();
    write_task(
        &board,
        "orb-aaa111",
        "---\ntitle: Parent\nstatus: todo\npriority: 1\ntype: epic\ndepends_on:\n  - orb-ccc333\nwaiting_on: \"vendor API key\"\nannotations:\n  devin:\n    score: 3\n    tags:\n      - a\n      - b\narea: board\nrepo: orbit\ncustom_field: keepme\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody of Parent\n",
    );
    write_task(
        &board,
        "orb-bbb222",
        "---\ntitle: Child\nstatus: in_progress\npriority: 3\ntype: task\nparent: orb-aaa111\nclaimed_by: devin\nclaimed_at: 2026-01-02T00:00:00Z\nlease_until: 2026-01-02T00:30:00Z\nlabels:\n  - repo:orbit\n  - bug\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody of Child\n",
    );
    write_task(
        &board,
        "orb-ccc333",
        "---\ntitle: Other\nstatus: done\npriority: 0\ntype: task\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody of Other\n",
    );
    commit_all(&repo, &board, "tasks");

    let value = repo.ok_json(&["migrate", "model", "--apply", "--json"]);

    assert_eq!(value["data"]["applied"], true, "{value}");
    assert!(value["data"]["commit"].as_str().is_some(), "{value}");
    assert_eq!(value["data"]["keys_allocated"], 3, "{value}");

    let parent = std::fs::read_to_string(board.join("tasks/orb-aaa111.md")).unwrap();
    assert!(parent.contains("status: ready"), "{parent}");
    assert!(parent.contains("priority: high"), "{parent}");
    assert!(parent.contains("key: ORB-1"), "{parent}");
    assert!(parent.contains("type: epic"), "{parent}");
    assert!(parent.contains("component: board"), "{parent}");
    assert!(parent.contains("repository: orbit"), "{parent}");
    assert!(parent.contains("custom_field: keepme"), "{parent}");
    assert!(!parent.contains("area:"), "{parent}");
    assert!(!parent.contains("repo:"), "{parent}");
    assert!(!parent.contains("annotations:"), "{parent}");
    assert!(!parent.contains("depends_on:"), "{parent}");
    assert!(!parent.contains("waiting_on:"), "{parent}");
    assert!(parent.contains("- type: blocks"), "{parent}");
    assert!(parent.contains("target: orb-ccc333"), "{parent}");
    assert!(parent.contains("vendor API key"), "{parent}");
    assert!(parent.contains("score: 3"), "{parent}");
    assert!(parent.contains("Body of Parent"), "{parent}");

    let child = std::fs::read_to_string(board.join("tasks/orb-bbb222.md")).unwrap();
    assert!(child.contains("status: in_progress"), "{child}");
    assert!(child.contains("priority: low"), "{child}");
    assert!(child.contains("key: ORB-2"), "{child}");
    assert!(child.contains("type: sub-task"), "{child}");
    assert!(child.contains("parent: orb-aaa111"), "{child}");
    assert!(child.contains("name: devin"), "{child}");
    assert!(child.contains("kind: agent"), "{child}");
    assert!(
        child.contains("claimed_at: 2026-01-02T00:00:00Z"),
        "{child}"
    );
    assert!(
        child.contains("lease_until: 2026-01-02T00:30:00Z"),
        "{child}"
    );
    assert!(child.contains("- bug"), "{child}");
    assert!(!child.contains("repo:orbit"), "{child}");
    assert!(!child.contains("claimed_by:"), "{child}");

    let other = std::fs::read_to_string(board.join("tasks/orb-ccc333.md")).unwrap();
    assert!(other.contains("status: done"), "{other}");
    assert!(other.contains("priority: highest"), "{other}");
    assert!(other.contains("key: ORB-3"), "{other}");

    // keys.toml records the allocator state.
    let keys: toml::Value =
        toml::from_str(&std::fs::read_to_string(board.join("keys.toml")).unwrap()).unwrap();
    assert_eq!(keys["prefix"].as_str().unwrap(), "ORB");
    assert_eq!(keys["next_key"].as_integer().unwrap(), 4);

    // config.toml gained the model sections.
    let config: toml::Value =
        toml::from_str(&std::fs::read_to_string(board.join("config.toml")).unwrap()).unwrap();
    let board_table = config.get("board").unwrap();
    assert!(board_table.get("priorities").is_some());
    assert_eq!(board_table["default_priority"].as_str().unwrap(), "medium");
    assert!(board_table.get("areas").is_none());
    assert!(board_table.get("components").is_some());
    assert!(config.get("workflow").is_some());

    // One commit carries the whole migration.
    let log = repo.git_in(&board, &["log", "--format=%s", "-1"]);
    assert_eq!(
        String::from_utf8_lossy(&log.stdout).trim(),
        "orbit: migrate board model"
    );

    // The migrated Board verifies clean.
    let (status, doctor) = repo.json(&["doctor", "--json"]);
    assert!(status == 0 || status == 1, "{doctor}");
    assert_eq!(doctor["data"]["healthy"], true, "{doctor}");
    assert!(
        doctor["data"]["issues"].as_array().unwrap().is_empty(),
        "{doctor}"
    );
}

#[test]
fn model_migrate_apply_is_idempotent() {
    let (repo, board) = legacy_board();
    write_task(
        &board,
        "orb-aaa111",
        "---\ntitle: One\nstatus: todo\npriority: 1\ntype: task\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody\n",
    );
    commit_all(&repo, &board, "task");

    repo.ok_json(&["migrate", "model", "--apply", "--json"]);
    let task_after = std::fs::read_to_string(board.join("tasks/orb-aaa111.md")).unwrap();
    let config_after = std::fs::read_to_string(board.join("config.toml")).unwrap();

    let again = repo.ok_json(&["migrate", "model", "--apply", "--json"]);
    assert_eq!(again["data"]["migrated"], false, "{again}");
    assert_eq!(again["data"]["already_migrated"], true, "{again}");
    assert_eq!(
        std::fs::read_to_string(board.join("tasks/orb-aaa111.md")).unwrap(),
        task_after
    );
    assert_eq!(
        std::fs::read_to_string(board.join("config.toml")).unwrap(),
        config_after
    );
}

#[test]
fn model_migrate_refuses_a_legacy_layout_board() {
    let repo = TestRepo::new();
    let board = repo.legacy_board_path();
    repo.legacy_board_files(&board, "orb-");
    write_task(
        &board,
        "orb-a7b2c9",
        "---\ntitle: Legacy\nstatus: todo\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody\n",
    );

    let (_, value) = repo.err_json(&["migrate", "model", "--json"]);
    assert_eq!(value["error"]["code"], "MIGRATION_BLOCKED", "{value}");
}

/// Copies this repository's real Board into the fixture and proves the
/// migration converts it with every Task still readable and doctor-clean.
/// Skips when the Board is not present (e.g. CI checkouts).
#[test]
fn model_migrate_converts_this_repositorys_board() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".orbit/board");
    if !source.join("config.toml").is_file() {
        eprintln!("skipping: real Board not present at {}", source.display());
        return;
    }

    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    // Replace the fixture Board with a copy of the real one.
    let _ = repo.git_in(&board, &["rm", "-rq", "."]);
    copy_dir(&source, &board);
    commit_all(&repo, &board, "real board");

    let before: Vec<String> = std::fs::read_dir(board.join("tasks"))
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("md"))
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    assert!(before.len() > 50, "expected the real Board's Tasks");

    let value = repo.ok_json(&["migrate", "model", "--apply", "--json"]);
    assert_eq!(value["data"]["applied"], true, "{value}");
    assert_eq!(
        value["data"]["keys_allocated"].as_u64().unwrap() as usize,
        before.len(),
        "{value}"
    );

    // Every Task still parses and reports through the Issue model.
    for id in &before {
        let stem = id.trim_end_matches(".md");
        let shown = repo.ok_json(&["show", stem, "--json"]);
        assert!(shown["data"]["key"].as_str().is_some(), "{stem}: {shown}");
    }

    let (status, doctor) = repo.json(&["doctor", "--json"]);
    assert!(status == 0 || status == 1, "{doctor}");
    assert_eq!(doctor["data"]["healthy"], true, "{doctor}");
    assert!(
        doctor["data"]["issues"].as_array().unwrap().is_empty(),
        "{doctor}"
    );
}

fn copy_dir(source: &Path, target: &Path) {
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            std::fs::create_dir_all(&to).unwrap();
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}
