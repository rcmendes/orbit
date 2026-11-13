//! `orbit board move DIR`: relocate the Board folder and keep every anchor
//! (the `.orbit/board` pointer, member link pointers, git excludes) pointing
//! at it.

mod common;

use common::TestRepo;

fn exclude(repo: &TestRepo) -> String {
    std::fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap_or_default()
}

fn canonical(path: &std::path::Path) -> String {
    std::fs::canonicalize(path)
        .unwrap()
        .to_string_lossy()
        .to_string()
}

#[test]
fn move_hidden_board_to_visible_folder_and_back() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("First task", &[]);

    let moved = repo.ok_json(&["board", "move", "issues", "--json"]);
    let target = repo.path().join("issues");
    assert_eq!(moved["data"]["to"], canonical(&target));
    assert!(target.join("config.toml").exists());
    // `.orbit/board` is now a pointer file naming the visible folder.
    assert!(repo.board_path().is_file());
    assert!(
        exclude(&repo).lines().any(|line| line == "/issues/"),
        "{}",
        exclude(&repo)
    );

    // Discovery and history follow the Board.
    let shown = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(shown["data"]["id"], id.as_str());
    let log = repo.git_in(&target, &["log", "--oneline"]);
    assert!(log.status.success());
    let second = repo.add("Second task", &[]);
    assert!(target.join("tasks").join(format!("{second}.md")).exists());

    // Moving to `.orbit/board` restores the hidden layout.
    let back = repo.ok_json(&["board", "move", ".orbit/board", "--json"]);
    assert_eq!(back["data"]["to"], canonical(&repo.board_path()));
    assert!(repo.board_path().join("config.toml").exists());
    assert!(!target.exists());
    repo.ok_json(&["show", &second, "--json"]);
}

#[test]
fn move_refuses_an_existing_non_empty_target() {
    let repo = TestRepo::new();
    repo.init();
    std::fs::create_dir_all(repo.path().join("issues")).unwrap();
    std::fs::write(repo.path().join("issues/README.md"), "x").unwrap();

    let (_, error) = repo.err_json(&["board", "move", "issues", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
    assert!(repo.board_path().join("config.toml").exists());
}

#[test]
fn move_rewrites_member_link_pointers() {
    let repo = TestRepo::new();
    repo.init();
    // Outside the anchor, so membership comes from the link pointer.
    let member = repo.home().join("member");
    std::fs::create_dir_all(&member).unwrap();
    for args in [
        &["init", "-q"][..],
        &["commit", "--allow-empty", "-q", "-m", "init"][..],
    ] {
        assert!(repo.git_in(&member, args).status.success());
    }
    repo.ok_json(&["repo", "add", member.to_str().unwrap(), "--json"]);
    let id = repo.add("Shared task", &[]);

    repo.ok_json(&["board", "move", "tracker", "--json"]);

    // The member still resolves the moved Board.
    let shown = repo.ok_json_in(&member, &["show", &id, "--json"]);
    assert_eq!(shown["data"]["id"], id.as_str());
}
