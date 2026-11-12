mod common;

use common::TestRepo;

#[test]
fn area_and_verification_round_trip_through_add_show_and_update() {
    let repo = TestRepo::new();
    repo.init();

    let id = repo.add(
        "Wire MCP filters",
        &["--area", "mcp", "--verification", "cargo test mcp"],
    );
    let content = repo.read_task(&id);
    assert!(content.contains("area: mcp"), "{content}");
    assert!(
        content.contains("verification: cargo test mcp"),
        "{content}"
    );

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["area"], "mcp");
    assert_eq!(show["data"]["verification"], "cargo test mcp");

    let updated = repo.ok_json(&[
        "update",
        id.as_str(),
        "--area",
        "cli",
        "--verification",
        "cargo test cli",
        "--json",
    ]);
    assert_eq!(updated["data"]["area"], "cli");
    assert_eq!(updated["data"]["verification"], "cargo test cli");

    repo.ok_json(&["update", id.as_str(), "--clear-area", "--json"]);
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert!(show["data"]["area"].is_null());
    assert_eq!(show["data"]["verification"], "cargo test cli");

    repo.ok_json(&["update", id.as_str(), "--clear-verification", "--json"]);
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert!(show["data"]["verification"].is_null());
}

#[test]
fn invalid_area_is_rejected_on_add_update_and_filters() {
    let repo = TestRepo::new();
    repo.init();

    // Area is a free-form slug: "Not An Area!" is not one.
    let (status, value) = repo.json(&["add", "Bad area", "--area", "Not An Area!", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_AREA");

    let id = repo.add("Task", &[]);
    let (status, value) = repo.json(&["update", id.as_str(), "--area", "Bad Area", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_AREA");

    let (status, value) = repo.json(&["list", "--area", "Bad Area", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_AREA");
}

#[test]
fn area_accepts_any_lowercase_slug_when_the_board_defines_no_areas() {
    let repo = TestRepo::new();
    repo.init();

    let id = repo.add("Mobile work", &["--area", "mobile-api"]);
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["area"], "mobile-api");

    let list = repo.ok_json(&["list", "--area", "mobile-api", "--json"]);
    assert_eq!(list["data"]["count"], 1);
    assert_eq!(list["data"]["tasks"][0]["id"], id.as_str());
}

/// Writes `areas = [...]` into the Board's `[board]` config table.
fn configure_board_areas(repo: &TestRepo, areas: &str) {
    let path = repo.board_path().join("config.toml");
    let config = std::fs::read_to_string(&path).unwrap();
    let config = config.replacen("[board]\n", &format!("[board]\nareas = {areas}\n"), 1);
    std::fs::write(&path, config).unwrap();
}

#[test]
fn board_areas_config_restricts_area_values() {
    let repo = TestRepo::new();
    repo.init();
    configure_board_areas(&repo, r#"["cli", "docs"]"#);

    // A configured Area works; casing normalizes to the lowercase slug.
    let id = repo.add("Docs work", &["--area", "DOCS"]);
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["area"], "docs");

    // A valid slug outside the Board's list is rejected.
    let (status, value) = repo.json(&["add", "MCP work", "--area", "mcp", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_AREA");

    let (status, value) = repo.json(&["update", id.as_str(), "--area", "mcp", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_AREA");

    // Filters are validated against the same list.
    let (status, value) = repo.json(&["list", "--area", "mcp", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_AREA");
    let listed = repo.ok_json(&["list", "--area", "docs", "--json"]);
    assert_eq!(listed["data"]["count"], 1);
}

#[test]
fn task_files_accept_free_form_area_slugs() {
    let repo = TestRepo::new();
    repo.init();

    let id = repo.add("Custom area", &["--area", "payments"]);
    let content = repo.read_task(&id);
    assert!(content.contains("area: payments"), "{content}");
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["area"], "payments");

    // A hand-edited value that is not a slug is corrupt, not silently kept.
    let content = content.replace("area: payments", "area: Not A Slug!");
    repo.write_task(&id, &content);
    let (status, value) = repo.json(&["list", "--json"]);
    assert_eq!(status, 1, "{value}");
    assert_eq!(value["error"]["code"], "CORRUPT_TASK");
}

#[test]
fn empty_verification_is_rejected() {
    let repo = TestRepo::new();
    repo.init();

    let (status, value) = repo.json(&["add", "Task", "--verification", "   ", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn area_filters_scope_list_and_ready() {
    let repo = TestRepo::new();
    repo.init();

    let cli = repo.add("CLI work", &["--area", "cli"]);
    let mcp = repo.add("MCP work", &["--area", "mcp"]);
    let unset = repo.add("Unclassified", &[]);

    let list = repo.ok_json(&["list", "--area", "cli", "--area", "mcp", "--json"]);
    assert_eq!(list["data"]["count"], 2);
    let ids: Vec<&str> = list["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert!(ids.contains(&cli.as_str()));
    assert!(ids.contains(&mcp.as_str()));
    assert!(!ids.contains(&unset.as_str()));

    let list = repo.ok_json(&["list", "--exclude-area", "cli", "--json"]);
    let ids: Vec<&str> = list["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert!(!ids.contains(&cli.as_str()));
    assert!(ids.contains(&mcp.as_str()));
    assert!(ids.contains(&unset.as_str()));

    let ready = repo.ok_json(&["ready", "--area", "mcp", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], mcp.as_str());
}

#[test]
fn export_includes_area_and_verification() {
    let repo = TestRepo::new();
    repo.init();

    let id = repo.add(
        "Export me",
        &["--area", "docs", "--verification", "cargo test export"],
    );
    let export = repo.ok_json(&["export", "--json"]);
    let task = export["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == id)
        .unwrap();
    assert_eq!(task["area"], "docs");
    assert_eq!(task["verification"], "cargo test export");
}

#[test]
fn project_guidance_file_is_checked_in() {
    let guidance = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("docs/agents/project-guidance.md");
    assert!(
        guidance.is_file(),
        "expected shared project guidance at docs/agents/project-guidance.md"
    );
    let text = std::fs::read_to_string(guidance).unwrap();
    assert!(text.contains("verification"));
    assert!(text.contains("area"));
}
