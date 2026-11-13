//! `orbit board host` / `orbit board sync` / `orbit init --visible|--host`
//! against local bare-repository hosts (ADR 0019).

mod common;

use std::path::{Path, PathBuf};

use common::TestRepo;

fn bare_host(repo: &TestRepo, name: &str) -> PathBuf {
    let output = repo.git(&["init", "--bare", "-q", name]);
    assert!(
        output.status.success(),
        "git init --bare failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.path().join(name)
}

fn host_arg(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// `board.host` stores canonicalized paths (macOS `/var` → `/private/var`).
fn canonical(path: &Path) -> String {
    dunce::canonicalize(path)
        .unwrap()
        .to_string_lossy()
        .to_string()
}

/// A non-bard clone of the host used to make "remote" commits, as another
/// machine would. Requires the host to already serve `orbit`.
fn peer_clone(repo: &TestRepo, host: &Path) -> PathBuf {
    let peer = repo.path().join("peer");
    let output = repo.git(&["clone", "-q", host_arg(host).as_str(), "peer"]);
    assert!(
        output.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = repo.git_in(&peer, &["checkout", "-q", "orbit"]);
    assert!(
        output.status.success(),
        "checkout orbit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    peer
}

fn peer_commit(repo: &TestRepo, peer: &Path, message: &str) {
    for args in [
        &["add", "-A"][..],
        &["commit", "-q", "-m", message][..],
        &["push", "-q", "origin", "orbit"][..],
    ] {
        let output = repo.git_in(peer, args);
        assert!(
            output.status.success(),
            "peer git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn peer_write(peer: &Path, relative: &str, content: &str) {
    let path = peer.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn peer_read(peer: &Path, relative: &str) -> String {
    std::fs::read_to_string(peer.join(relative)).unwrap()
}

fn remote_tip(repo: &TestRepo, host: &Path) -> Option<String> {
    let output = repo.git(&["ls-remote", host_arg(host).as_str(), "refs/heads/orbit"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .map(str::to_string)
}

#[test]
fn host_is_absent_and_manual_by_default() {
    let repo = TestRepo::new();
    repo.init();

    let shown = repo.ok_json(&["board", "host", "--json"]);
    assert_eq!(shown["data"]["host"], serde_json::Value::Null);
    assert_eq!(shown["data"]["host_sync"], "manual");

    let config = repo.read(".orbit/board/config.toml");
    assert!(!config.contains("host"), "config.toml: {config}");
}

#[test]
fn board_host_sets_and_clears_without_touching_remote() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("First task", &[]);
    let host = bare_host(&repo, "host.git");

    let set = repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);
    assert_eq!(set["data"]["host"], canonical(&host));
    assert_eq!(set["data"]["joined"], false);

    let shown = repo.ok_json(&["board", "host", "--json"]);
    assert_eq!(shown["data"]["host"], canonical(&host));
    assert_eq!(shown["data"]["url"], canonical(&host));

    // Publish so the remote has an `orbit` branch, then clear the host.
    repo.ok_json(&["board", "sync", "--json"]);
    let tip = remote_tip(&repo, &host).expect("remote orbit tip");

    let cleared = repo.ok_json(&["board", "host", "--none", "--json"]);
    assert_eq!(cleared["data"]["host"], serde_json::Value::Null);
    assert_eq!(cleared["data"]["remote_branch_deleted"], false);

    // Clearing never deletes the remote `orbit` branch.
    assert_eq!(remote_tip(&repo, &host).as_deref(), Some(tip.as_str()));

    let shown = repo.ok_json(&["board", "host", "--json"]);
    assert_eq!(shown["data"]["host"], serde_json::Value::Null);

    // Sync now refuses: no host configured.
    let (_, error) = repo.err_json(&["board", "sync", "--json"]);
    assert_eq!(error["error"]["code"], "BOARD_HOST_NOT_CONFIGURED");
    let _ = id;
}

#[test]
fn board_sync_requires_a_host() {
    let repo = TestRepo::new();
    repo.init();
    let (_, error) = repo.err_json(&["board", "sync", "--json"]);
    assert_eq!(error["error"]["code"], "BOARD_HOST_NOT_CONFIGURED");
}

#[test]
fn board_sync_publishes_then_reports_up_to_date() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("First task", &[]);
    let host = bare_host(&repo, "host.git");
    repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);

    let published = repo.ok_json(&["board", "sync", "--json"]);
    assert_eq!(published["data"]["state"], "published");
    assert!(remote_tip(&repo, &host).is_some());

    let again = repo.ok_json(&["board", "sync", "--json"]);
    assert_eq!(again["data"]["state"], "up_to_date");
}

#[test]
fn board_sync_ignores_the_previous_host_tracking_ref() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("First task", &[]);
    let first = bare_host(&repo, "first.git");
    let second = bare_host(&repo, "second.git");
    let output = repo.git(&["remote", "add", "origin", host_arg(&first).as_str()]);
    assert!(output.status.success());
    repo.ok_json(&["board", "host", &host_arg(repo.path()), "--json"]);
    repo.ok_json(&["board", "sync", "--json"]);
    assert!(remote_tip(&repo, &first).is_some());

    let output = repo.git(&["remote", "set-url", "origin", host_arg(&second).as_str()]);
    assert!(output.status.success());
    let published = repo.ok_json(&["board", "sync", "--json"]);

    assert_eq!(published["data"]["state"], "published");
    assert!(
        remote_tip(&repo, &second).is_some(),
        "sync trusted the stale host/orbit ref instead of publishing to the new host"
    );
}

#[test]
fn board_sync_fetches_only_the_board_branch() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("First task", &[]);
    let host = bare_host(&repo, "host.git");
    repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);
    repo.ok_json(&["board", "sync", "--json"]);

    let peer = peer_clone(&repo, &host);
    let output = repo.git_in(&peer, &["checkout", "-q", "-b", "extra"]);
    assert!(output.status.success());
    peer_write(&peer, "extra.txt", "not part of the Board\n");
    for args in [
        &["add", "-A"][..],
        &["commit", "-q", "-m", "extra branch"][..],
        &["push", "-q", "origin", "extra"][..],
    ] {
        let output = repo.git_in(&peer, args);
        assert!(
            output.status.success(),
            "peer git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    repo.ok_json(&["board", "sync", "--json"]);
    let extra = repo.git_in(
        &repo.board_path(),
        &["show-ref", "--verify", "--quiet", "refs/remotes/host/extra"],
    );
    assert!(
        !extra.status.success(),
        "board sync fetched a non-Board host branch"
    );
}

#[test]
fn local_repository_host_publishes_through_its_origin() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("First task", &[]);
    let origin = bare_host(&repo, "origin.git");
    let output = repo.git(&["remote", "add", "origin", host_arg(&origin).as_str()]);
    assert!(output.status.success());

    // The code repository itself hosts the Board; its origin stores `orbit`.
    let set = repo.ok_json(&["board", "host", &host_arg(repo.path()), "--json"]);
    assert_eq!(set["data"]["host"], canonical(repo.path()));
    assert_eq!(set["data"]["url"], host_arg(&origin));

    let published = repo.ok_json(&["board", "sync", "--json"]);
    assert_eq!(published["data"]["state"], "published");
    assert!(remote_tip(&repo, &origin).is_some());
    // Nothing lands in the code repository's own branches.
    let local = repo.git(&["branch", "--list", "orbit"]);
    assert!(String::from_utf8_lossy(&local.stdout).trim().is_empty());
}

#[test]
fn sync_unrelated_history_merges_different_tasks() {
    let repo = TestRepo::new();
    repo.init();
    let local_id = repo.add("Local task", &[]);
    let host = bare_host(&repo, "host.git");
    repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);
    repo.ok_json(&["board", "sync", "--json"]);

    // A second Board with the same Slug joins through `init --host`.
    let second = repo.path().join("second");
    std::fs::create_dir_all(&second).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Test"][..],
        &["config", "user.email", "test@example.com"][..],
        &["commit", "--allow-empty", "-q", "-m", "init"][..],
    ] {
        let output = repo.git_in(&second, args);
        assert!(output.status.success());
    }
    let joined = repo.ok_json_in(
        repo.path(),
        &[
            "init",
            "second",
            "--slug",
            "orb",
            &format!("--host={}", host_arg(&host)),
            "--agents",
            "none",
            "--json",
        ],
    );
    assert_eq!(joined["data"]["host_joined"], true);
    let second_board = second.join(".orbit").join("board");
    assert!(
        second_board
            .join("tasks")
            .join(format!("{local_id}.md"))
            .exists(),
        "joined Board adopted the host's Tasks"
    );

    // Diverge: the second Board adds a Task while the first adds another.
    let peer_id = repo.ok_json_in(&second, &["add", "Peer task", "--json"])["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let other_local = repo.add("Other local task", &[]);

    // Each side publishes its own commit, then merges the other's.
    repo.ok_json(&["board", "sync", "--json"]);
    let second_sync = repo.ok_json_in(&second, &["board", "sync", "--json"]);
    assert_eq!(second_sync["data"]["state"], "merged");
    let first_sync = repo.ok_json(&["board", "sync", "--json"]);
    assert_eq!(first_sync["data"]["state"], "fast_forwarded");

    // Both Boards now hold every Task.
    for board in [repo.board_path(), second_board.clone()] {
        assert!(board.join("tasks").join(format!("{local_id}.md")).exists());
        assert!(board.join("tasks").join(format!("{peer_id}.md")).exists());
        assert!(
            board
                .join("tasks")
                .join(format!("{other_local}.md"))
                .exists()
        );
    }
}

#[test]
fn init_host_refuses_a_different_slug() {
    let repo = TestRepo::new();
    repo.init();
    let host = bare_host(&repo, "host.git");
    repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);
    repo.ok_json(&["board", "sync", "--json"]);

    let second = repo.path().join("second");
    std::fs::create_dir_all(&second).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Test"][..],
        &["config", "user.email", "test@example.com"][..],
        &["commit", "--allow-empty", "-q", "-m", "init"][..],
    ] {
        let output = repo.git_in(&second, args);
        assert!(output.status.success());
    }

    let (_, error) = repo.json_in(
        repo.path(),
        &[
            "init",
            "second",
            "--slug",
            "other",
            &format!("--host={}", host_arg(&host)),
            "--agents",
            "none",
            "--json",
        ],
    );
    assert_eq!(error["error"]["code"], "BOARD_HOST_CONFLICT");

    // Retrying with the matching Slug joins the existing host branch.
    let joined = repo.ok_json_in(
        repo.path(),
        &[
            "init",
            "second",
            "--slug",
            "orb",
            &format!("--host={}", host_arg(&host)),
            "--agents",
            "none",
            "--json",
        ],
    );
    assert_eq!(joined["data"]["host_joined"], true);
}

#[test]
fn sync_field_conflict_stops_until_resolved() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Contested task", &[]);
    let host = bare_host(&repo, "host.git");
    repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);
    repo.ok_json(&["board", "sync", "--json"]);
    let peer = peer_clone(&repo, &host);

    // Both sides retitle the same Task differently.
    let task_relative = format!("tasks/{id}.md");
    peer_write(
        &peer,
        &task_relative,
        &peer_read(&peer, &task_relative).replace("Contested task", "Host title"),
    );
    peer_commit(&repo, &peer, "host retitle");
    repo.ok_json(&["update", &id, "--title", "Local title", "--json"]);

    let (status, error) = repo.json(&["board", "sync", "--json"]);
    assert_ne!(status, 0);
    assert_eq!(error["error"]["code"], "BOARD_SYNC_CONFLICT");
    let conflicts = error["error"]["details"]["conflicts"].as_array().unwrap();
    assert!(
        conflicts
            .iter()
            .any(|conflict| conflict["field"] == format!("{id}.title")),
        "{conflicts:?}"
    );

    // The losing side stays untouched until a resolution is given.
    let resolved = repo.ok_json(&[
        "board",
        "sync",
        "--resolve",
        &format!("{id}.title=local"),
        "--json",
    ]);
    assert_eq!(resolved["data"]["state"], "merged");
    assert!(
        resolved["data"]["conflicts_resolved"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == &format!("{id}.title"))
    );

    let task = repo.read_task(&id);
    assert!(task.contains("title: Local title"), "{task}");
    assert!(remote_tip(&repo, &host).is_some());
}

#[test]
fn sync_never_rebases_or_force_pushes() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("History task", &[]);
    let host = bare_host(&repo, "host.git");
    repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);
    repo.ok_json(&["board", "sync", "--json"]);
    let peer = peer_clone(&repo, &host);

    let task_relative = format!("tasks/{id}.md");
    peer_write(
        &peer,
        &task_relative,
        &peer_read(&peer, &task_relative).replace("priority: 2", "priority: 1"),
    );
    peer_commit(&repo, &peer, "host edit");
    let peer_tip = repo.git_in(&peer, &["rev-parse", "HEAD"]).stdout;
    let peer_tip = String::from_utf8_lossy(&peer_tip).trim().to_string();

    repo.ok_json(&["update", &id, "--title", "Local title", "--json"]);
    repo.ok_json(&["board", "sync", "--json"]);

    // The remote tip is a merge commit whose second parent is the peer's work.
    let output = repo.git(&["ls-remote", host_arg(&host).as_str(), "refs/heads/orbit"]);
    let remote = String::from_utf8_lossy(&output.stdout);
    let remote_tip = remote
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .unwrap()
        .to_string();
    let log = repo.git_in(
        &repo.board_path(),
        &["log", "--format=%P", "-1", &remote_tip],
    );
    let parents = String::from_utf8_lossy(&log.stdout);
    assert_eq!(parents.split_whitespace().count(), 2, "{parents}");

    // The peer's commit remains an ancestor: history was merged, not rewritten.
    let check = repo.git_in(&peer, &["fetch", "-q", "origin"]);
    assert!(check.status.success());
    let ancestor = repo.git_in(
        &peer,
        &["merge-base", "--is-ancestor", &peer_tip, "origin/orbit"],
    );
    assert!(
        ancestor.status.success(),
        "peer commit was rewritten: {}",
        String::from_utf8_lossy(&ancestor.stderr)
    );
}

#[test]
fn init_defaults_to_the_hidden_board_folder() {
    let repo = TestRepo::new();
    repo.init();
    assert!(repo.board_path().join("config.toml").exists());
    assert!(repo.board_path().ends_with(".orbit/board"));
}

#[test]
fn init_visible_uses_a_visible_board_folder() {
    let repo = TestRepo::new();
    let visible = repo.ok_json(&[
        "init",
        "--visible",
        "--agents",
        "none",
        "--slug",
        "vis",
        "--json",
    ]);
    let board = visible["data"]["board"].as_str().unwrap().to_string();
    assert!(Path::new(&board).ends_with("issues"), "{board}");
    assert!(Path::new(&board).join("config.toml").exists());
    // The Orbit folder anchors it through the `board` pointer file.
    let pointer = repo.orbit_path().join("board");
    assert!(pointer.is_file(), "pointer file missing at {pointer:?}");
    // Discovery still finds the Board from the repository root.
    let listed = repo.ok_json(&["list", "--json"]);
    assert_eq!(listed["ok"], true);
}

#[test]
fn init_visible_named_picks_the_folder_name() {
    let repo = TestRepo::new();
    let named = repo.ok_json(&[
        "init",
        "--visible=tracker",
        "--agents",
        "none",
        "--slug",
        "trk",
        "--json",
    ]);
    let board = named["data"]["board"].as_str().unwrap().to_string();
    assert!(Path::new(&board).ends_with("tracker"), "{board}");
    assert!(Path::new(&board).join("config.toml").exists());
    let listed = repo.ok_json(&["list", "--json"]);
    assert_eq!(listed["ok"], true);
}

#[test]
fn init_host_flag_sets_host_without_prompting() {
    let repo = TestRepo::new();
    let host = bare_host(&repo, "host.git");

    let init = repo.ok_json(&[
        "init",
        &format!("--host={}", host_arg(&host)),
        "--agents",
        "none",
        "--slug",
        "orb",
        "--json",
    ]);
    assert_eq!(init["data"]["host"], canonical(&host));
    assert_eq!(init["data"]["host_joined"], false);

    let config = repo.read(".orbit/board/config.toml");
    assert!(config.contains(&canonical(&host)), "{config}");
}

#[test]
fn host_sync_auto_publishes_after_commits() {
    let repo = TestRepo::new();
    repo.init();
    let host = bare_host(&repo, "host.git");
    repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);

    // Opt in to automatic sync.
    let config_path = repo.board_path().join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        config.replace("auto_commit", "host_sync = \"auto\"\nauto_commit"),
    )
    .unwrap();

    // The next committed mutation publishes the Board without `board sync`.
    repo.add("Auto task", &[]);
    assert!(
        remote_tip(&repo, &host).is_some(),
        "host_sync=auto did not publish after the commit"
    );
}

#[test]
fn host_sync_auto_failures_are_warnings_not_errors() {
    let repo = TestRepo::new();
    repo.init();
    let host = bare_host(&repo, "host.git");
    repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);

    let config_path = repo.board_path().join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        // Line-based: Windows paths are TOML-escaped, so a substring match misses.
        config
            .lines()
            .map(|line| {
                if line.starts_with("host = ") {
                    "host = \"file:///nonexistent/host/path\"".to_string()
                } else if line.starts_with("auto_commit") {
                    format!("host_sync = \"auto\"\n{line}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();

    // A Write command still succeeds; the host failure is a warning.
    let output = repo.orbit(&["add", "Still works", "--json"]);
    assert!(
        output.status.success(),
        "auto host sync failure was fatal: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("BOARD_HOST_SYNC_FAILED"), "{stdout}");
}

#[test]
fn no_sync_disables_host_auto_sync() {
    let repo = TestRepo::new();
    repo.init();
    let host = bare_host(&repo, "host.git");
    repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);

    let config_path = repo.board_path().join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        config.replace("auto_commit", "host_sync = \"auto\"\nauto_commit"),
    )
    .unwrap();

    repo.orbit(&["add", "Manual only", "--no-sync", "--json"]);
    assert!(
        remote_tip(&repo, &host).is_none(),
        "--no-sync did not suppress host auto-sync"
    );
}

#[test]
fn host_sync_auto_runs_at_prime() {
    let repo = TestRepo::new();
    repo.init();
    let host = bare_host(&repo, "host.git");
    repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);

    let config_path = repo.board_path().join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        config.replace("auto_commit", "host_sync = \"auto\"\nauto_commit"),
    )
    .unwrap();

    // Publish once so the host serves `orbit`, then a peer adds a Task.
    repo.add("Local task", &[]);
    assert!(remote_tip(&repo, &host).is_some());
    let peer = peer_clone(&repo, &host);
    let peer_id = "orb-ffffff";
    peer_write(
        &peer,
        &format!("tasks/{peer_id}.md"),
        "---\ntitle: Peer task\nstatus: todo\ntype: task\npriority: 2\nlabels: []\ndepends_on: []\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody.\n",
    );
    peer_commit(&repo, &peer, "peer task");

    // `orbit prime` syncs first, so the primed Board already has the Task.
    repo.ok_json(&["prime", "--json"]);
    assert!(
        repo.board_path()
            .join("tasks")
            .join(format!("{peer_id}.md"))
            .exists(),
        "prime did not pull the host's Task"
    );
}

#[test]
fn board_host_rejects_a_spec_that_looks_like_a_git_option() {
    let repo = TestRepo::new();
    repo.init();

    let (_, error) = repo.err_json(&["board", "host", "--json", "--", "-x.y:z"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT", "{error}");
}

#[test]
fn sync_refuses_a_host_branch_replaced_by_another_board() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("First task", &[]);
    let host = bare_host(&repo, "host.git");
    repo.ok_json(&["board", "host", &host_arg(&host), "--json"]);
    repo.ok_json(&["board", "sync", "--json"]);

    // Another Board takes over the host's `orbit` branch.
    let peer = peer_clone(&repo, &host);
    let config: String = peer_read(&peer, "config.toml")
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("prefix") {
                "prefix = \"zzz-\"".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    peer_write(&peer, "config.toml", &config);
    peer_commit(&repo, &peer, "other board");

    let (_, error) = repo.err_json(&["board", "sync", "--json"]);
    assert_eq!(error["error"]["code"], "BOARD_HOST_CONFLICT", "{error}");
}
