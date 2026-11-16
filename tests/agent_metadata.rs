mod common;

use common::TestRepo;

#[test]
fn component_and_verification_round_trip_through_add_show_and_update() {
    let repo = TestRepo::new();
    repo.init();

    let id = repo.add(
        "Wire MCP filters",
        &["--component", "mcp", "--verification", "cargo test mcp"],
    );
    let content = repo.read_task(&id);
    assert!(content.contains("component: mcp"), "{content}");
    assert!(
        content.contains("verification: cargo test mcp"),
        "{content}"
    );

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["component"], "mcp");
    assert_eq!(show["data"]["verification"], "cargo test mcp");

    let updated = repo.ok_json(&[
        "update",
        id.as_str(),
        "--component",
        "cli",
        "--verification",
        "cargo test cli",
        "--json",
    ]);
    assert_eq!(updated["data"]["component"], "cli");
    assert_eq!(updated["data"]["verification"], "cargo test cli");

    repo.ok_json(&["update", id.as_str(), "--clear-component", "--json"]);
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert!(show["data"]["component"].is_null());
    assert_eq!(show["data"]["verification"], "cargo test cli");

    repo.ok_json(&["update", id.as_str(), "--clear-verification", "--json"]);
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert!(show["data"]["verification"].is_null());
}

#[test]
fn invalid_component_is_rejected_on_add_update_and_filters() {
    let repo = TestRepo::new();
    repo.init();

    // Component is a free-form slug: "Not An Component!" is not one.
    let (status, value) = repo.json(&[
        "add",
        "Bad component",
        "--component",
        "Not An Component!",
        "--json",
    ]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_COMPONENT");

    let id = repo.add("Task", &[]);
    let (status, value) = repo.json(&[
        "update",
        id.as_str(),
        "--component",
        "Bad Component",
        "--json",
    ]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_COMPONENT");

    let (status, value) = repo.json(&["list", "--component", "Bad Component", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_COMPONENT");
}

#[test]
fn component_accepts_any_lowercase_slug_when_the_board_defines_no_components() {
    let repo = TestRepo::new();
    repo.init();

    let id = repo.add("Mobile work", &["--component", "mobile-api"]);
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["component"], "mobile-api");

    let list = repo.ok_json(&["list", "--component", "mobile-api", "--json"]);
    assert_eq!(list["data"]["count"], 1);
    assert_eq!(list["data"]["tasks"][0]["id"], id.as_str());
}

/// Writes `components = [...]` into the Board's `[board]` config table.
fn configure_board_components(repo: &TestRepo, components: &str) {
    let path = repo.board_path().join("config.toml");
    let config = std::fs::read_to_string(&path).unwrap();
    let config = config.replacen(
        "[board]\n",
        &format!("[board]\ncomponents = {components}\n"),
        1,
    );
    std::fs::write(&path, config).unwrap();
}

#[test]
fn board_components_config_restricts_component_values() {
    let repo = TestRepo::new();
    repo.init();
    configure_board_components(&repo, r#"["cli", "docs"]"#);

    // A configured Component works; casing normalizes to the lowercase slug.
    let id = repo.add("Docs work", &["--component", "DOCS"]);
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["component"], "docs");

    // A valid slug outside the Board's list is rejected.
    let (status, value) = repo.json(&["add", "MCP work", "--component", "mcp", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_COMPONENT");

    let (status, value) = repo.json(&["update", id.as_str(), "--component", "mcp", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_COMPONENT");

    // Filters are validated against the same list.
    let (status, value) = repo.json(&["list", "--component", "mcp", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_COMPONENT");
    let listed = repo.ok_json(&["list", "--component", "docs", "--json"]);
    assert_eq!(listed["data"]["count"], 1);
}

#[test]
fn task_files_accept_free_form_component_slugs() {
    let repo = TestRepo::new();
    repo.init();

    let id = repo.add("Custom component", &["--component", "payments"]);
    let content = repo.read_task(&id);
    assert!(content.contains("component: payments"), "{content}");
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["component"], "payments");

    // A hand-edited value that is not a slug is corrupt, not silently kept.
    let content = content.replace("component: payments", "component: Not A Slug!");
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
fn component_filters_scope_list_and_ready() {
    let repo = TestRepo::new();
    repo.init();

    let cli = repo.add("CLI work", &["--component", "cli"]);
    let mcp = repo.add("MCP work", &["--component", "mcp"]);
    let unset = repo.add("Unclassified", &[]);

    let list = repo.ok_json(&["list", "--component", "cli", "--component", "mcp", "--json"]);
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

    let list = repo.ok_json(&["list", "--exclude-component", "cli", "--json"]);
    let ids: Vec<&str> = list["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert!(!ids.contains(&cli.as_str()));
    assert!(ids.contains(&mcp.as_str()));
    assert!(ids.contains(&unset.as_str()));

    let ready = repo.ok_json(&["ready", "--component", "mcp", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], mcp.as_str());
}

#[test]
fn export_includes_component_and_verification() {
    let repo = TestRepo::new();
    repo.init();

    let id = repo.add(
        "Export me",
        &["--component", "docs", "--verification", "cargo test export"],
    );
    let export = repo.ok_json(&["export", "--json"]);
    let task = export["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == id)
        .unwrap();
    assert_eq!(task["component"], "docs");
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
    assert!(text.contains("component"));
}
