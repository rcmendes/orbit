mod common;

use common::TestRepo;

#[test]
fn setup_check_fails_when_agents_md_is_stale() {
    let repo = TestRepo::new();
    std::fs::write(
        repo.path().join("AGENTS.md"),
        "# Project\n\n<!-- BEGIN ORBIT INTEGRATION (managed by orbit setup) -->\n## Orbit task tracking\n\nStale instructions.\n<!-- END ORBIT INTEGRATION -->\n",
    )
    .unwrap();

    let (status, value) = repo.json(&["setup", "opencode", "--no-mcp", "--check", "--json"]);
    assert_eq!(status, 1, "{value}");
    assert_eq!(value["data"]["status"], "stale");
    let files = value["data"]["files"].as_array().unwrap();
    assert!(files.iter().any(|file| file["path"] == "AGENTS.md"));
    assert!(files.iter().all(|file| file["path"] != "opencode.json"));

    repo.ok_json(&["setup", "opencode", "--no-mcp", "--json"]);

    let (status, value) = repo.json(&["setup", "opencode", "--no-mcp", "--check", "--json"]);
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["status"], "installed");
}
