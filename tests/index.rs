//! The opt-in derived index (`[index] enabled = true`): a rebuildable cache
//! under the Board's cache directory that accelerates the read scans
//! (`list`, `ready`, `search`) on large Boards. Task files and Git history
//! remain the only source of truth; the index is validated against file
//! metadata on every indexed load and rebuilt from the files whenever it is
//! missing, stale, or corrupt.

mod common;

use std::path::PathBuf;

use common::TestRepo;

/// Appends `[index] enabled = true` to the Board config and commits it.
fn enable_index(repo: &TestRepo) {
    let board = repo.board_path();
    let path = board.join("config.toml");
    let mut config = std::fs::read_to_string(&path).unwrap();
    config.push_str("\n[index]\nenabled = true\n");
    std::fs::write(&path, config).unwrap();
    for args in [
        &["add", "-A"][..],
        &["commit", "-q", "-m", "enable index"][..],
    ] {
        let output = repo.git_in(&board, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn index_path(repo: &TestRepo) -> PathBuf {
    repo.orbit_path().join("cache").join("index.json")
}

fn titles(repo: &TestRepo) -> Vec<String> {
    let value = repo.ok_json(&["list", "--include-closed", "--json"]);
    value["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["title"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn index_status_reports_disabled_board() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("No index", &[]);

    let value = repo.ok_json(&["index", "status", "--json"]);
    assert_eq!(value["data"]["enabled"], false);
    assert_eq!(value["data"]["present"], false);
}

#[test]
fn indexed_reads_build_and_reuse_the_cache() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Indexed one", &[]);
    repo.add("Indexed two", &[]);
    enable_index(&repo);

    assert!(!index_path(&repo).exists());
    let listed = titles(&repo);
    assert_eq!(listed.len(), 2);

    // The indexed read built the cache and reports every file fresh.
    assert!(index_path(&repo).exists());
    let status = repo.ok_json(&["index", "status", "--json"]);
    assert_eq!(status["data"]["enabled"], true);
    assert_eq!(status["data"]["present"], true);
    assert_eq!(status["data"]["entries"], 2);
    assert_eq!(status["data"]["stale"], 0);
    assert!(status["data"]["head"].as_str().unwrap().len() >= 7);

    // Cached results match a fresh scan exactly.
    assert_eq!(titles(&repo), listed);
}

#[test]
fn index_rebuilds_after_being_deleted_or_corrupted() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Survives rebuild", &[]);
    enable_index(&repo);
    repo.ok_json(&["index", "rebuild", "--json"]);
    assert!(index_path(&repo).exists());

    std::fs::remove_file(index_path(&repo)).unwrap();
    assert_eq!(titles(&repo), vec!["Survives rebuild".to_string()]);
    assert!(index_path(&repo).exists(), "deleted index was rebuilt");

    std::fs::write(index_path(&repo), "this is not json {").unwrap();
    assert_eq!(titles(&repo), vec!["Survives rebuild".to_string()]);

    // A valid JSON document of the wrong shape is also rebuilt.
    std::fs::write(index_path(&repo), "{\"version\": 99}").unwrap();
    assert_eq!(titles(&repo), vec!["Survives rebuild".to_string()]);
}

#[test]
fn index_tracks_file_edits_and_deletes() {
    let repo = TestRepo::new();
    repo.init();
    let keep = repo.add("Keep me", &[]);
    let gone = repo.add("Delete me", &[]);
    enable_index(&repo);
    assert_eq!(titles(&repo).len(), 2);

    // An edit made outside Orbit is picked up on the next indexed read.
    let task_file = repo.board_path().join("tasks").join(format!("{keep}.md"));
    let contents = std::fs::read_to_string(&task_file).unwrap();
    std::fs::write(&task_file, contents.replace("Keep me", "Renamed outside")).unwrap();
    std::fs::remove_file(repo.board_path().join("tasks").join(format!("{gone}.md"))).unwrap();

    let mut listed = titles(&repo);
    listed.sort();
    assert_eq!(listed, vec!["Renamed outside".to_string()]);

    // A normal Orbit write afterwards stays authoritative too.
    repo.add("Added after", &[]);
    assert_eq!(titles(&repo).len(), 2);
}

#[test]
fn index_rebuild_command_reports_fresh_counts() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("One", &[]);
    repo.add("Two", &[]);
    repo.add("Three", &[]);
    enable_index(&repo);

    let value = repo.ok_json(&["index", "rebuild", "--json"]);
    assert_eq!(value["data"]["rebuilt"], true);
    assert_eq!(value["data"]["entries"], 3);
    assert!(value["data"]["head"].as_str().unwrap().len() >= 7);
    assert!(index_path(&repo).exists());
}

#[test]
fn indexed_search_matches_plain_search() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Fix login redirect loop", &["--label", "backend"]);
    repo.add("Unrelated chore", &[]);

    let plain = repo.ok_json(&["search", "login", "--json"]);
    enable_index(&repo);
    let indexed = repo.ok_json(&["search", "login", "--json"]);
    assert_eq!(plain["data"]["results"], indexed["data"]["results"]);
}
