mod common;

use common::TestRepo;

fn canonical(path: &std::path::Path) -> String {
    path.canonicalize().unwrap().to_str().unwrap().to_string()
}

#[test]
fn workspace_creates_an_external_board() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("acme");

    let value = repo.ok_json_in(parent.path(), &["workspace", "acme", "--json"]);

    assert_eq!(value["data"]["created"], true);
    assert_eq!(value["data"]["branch"], "orbit");
    assert_eq!(value["data"]["workspace"], canonical(&folder));
    assert_eq!(value["data"]["board"], canonical(&folder.join("board")));
    assert!(folder.join("board/config.toml").exists());
    assert!(folder.join("board/.git").exists());
}

#[test]
fn workspace_is_idempotent() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();

    let first = repo.ok_json_in(parent.path(), &["workspace", "acme", "--json"]);
    let second = repo.ok_json_in(parent.path(), &["workspace", "acme", "--json"]);

    assert_eq!(first["data"]["created"], true);
    assert_eq!(second["data"]["created"], false);
    assert_eq!(second["data"]["board"], first["data"]["board"]);
}

#[test]
fn workspace_rejects_a_directory_inside_a_git_working_tree() {
    let repo = TestRepo::new();

    let (status, value) = repo.json_in(repo.path(), &["workspace", "acme", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "WORKSPACE_INSIDE_REPO");
    assert!(!repo.path().join("acme").exists());
}

#[test]
fn workspace_rejects_a_directory_inside_the_git_directory() {
    let repo = TestRepo::new();

    let (status, value) = repo.json_in(repo.path(), &["workspace", ".git/acme", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "WORKSPACE_INSIDE_REPO");
}

#[test]
fn workspace_does_not_link_or_configure_the_current_repository() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("acme");

    repo.ok_json(&["workspace", folder.to_str().unwrap(), "--json"]);

    assert!(!repo.path().join(".git/orbit/link").exists());
    assert!(!repo.exists("CLAUDE.md"));
    assert!(!repo.exists("AGENTS.md"));
    assert!(!repo.exists(".mcp.json"));
}

#[test]
fn workspace_prints_the_link_instruction() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("acme");

    let output = repo.orbit_in(parent.path(), &["workspace", "acme"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(stdout.contains("created workspace at"), "{stdout}");
    assert!(stdout.contains("orbit link"), "{stdout}");
    assert!(stdout.contains(&canonical(&folder)), "{stdout}");
}

#[test]
fn workspace_refuses_a_non_empty_board_directory() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("acme");
    std::fs::create_dir_all(folder.join("board")).unwrap();
    std::fs::write(folder.join("board/keep.txt"), "keep").unwrap();

    let (status, value) = repo.json_in(parent.path(), &["workspace", "acme", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "CONFIG_ERROR");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("must be empty")
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("board/keep.txt")).unwrap(),
        "keep"
    );
    assert!(!folder.join("board/config.toml").exists());
}

#[test]
fn workspace_refuses_a_path_that_is_a_file() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    std::fs::write(parent.path().join("acme"), "not a directory").unwrap();

    let (status, value) = repo.json_in(parent.path(), &["workspace", "acme", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "CONFIG_ERROR");
    assert_eq!(
        std::fs::read_to_string(parent.path().join("acme")).unwrap(),
        "not a directory"
    );
}

#[test]
fn link_accepts_a_workspace_root() {
    let creator = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("acme");
    creator.ok_json_in(parent.path(), &["workspace", "acme", "--json"]);

    let joiner = TestRepo::new();
    let value = joiner.ok_json(&["link", folder.to_str().unwrap(), "--json"]);

    assert_eq!(value["data"]["board"], canonical(&folder.join("board")));

    let id = joiner.add("Shared task", &[]);
    let listed = joiner.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}

#[test]
fn link_still_accepts_a_direct_board_root() {
    let creator = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("acme");
    creator.ok_json_in(parent.path(), &["workspace", "acme", "--json"]);

    let joiner = TestRepo::new();
    let board = folder.join("board");
    let value = joiner.ok_json(&["link", board.to_str().unwrap(), "--json"]);

    assert_eq!(value["data"]["board"], canonical(&board));
}
