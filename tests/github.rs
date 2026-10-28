mod common;

use common::TestRepo;
use std::fs;
use tempfile::TempDir;

fn write_github_config(repo: &TestRepo) {
    write_github_config_for_repositories(repo, &["acme/widget"]);
}

fn write_github_config_for_repositories(repo: &TestRepo, repositories: &[&str]) {
    let path = repo.board_path().join("config.toml");
    let mut config = fs::read_to_string(&path).unwrap();
    let repositories = repositories
        .iter()
        .map(|repository| format!("\"{repository}\""))
        .collect::<Vec<_>>()
        .join(", ");
    config.push_str(&format!(
        "\n[github]\norganization = \"acme\"\nrepositories = [{repositories}]\n"
    ));
    fs::write(path, config).unwrap();
}

fn write_fixture(dir: &TempDir) -> std::path::PathBuf {
    let path = dir.path().join("fixture.json");
    fs::write(
        &path,
        r#"{
  "org_fields": {
    "acme/Area": "AREA_ID",
    "acme/Verification": "VERIFICATION_ID"
  },
  "issues": {
    "acme/widget#42": {
      "repo": "acme/widget",
      "number": 42,
      "title": "Imported issue",
      "body": "Body from GitHub",
      "state": "open",
      "labels": ["bug"],
      "assignees": ["human"],
      "updated_at": "2026-10-23T13:00:00Z",
      "area": "cli",
      "verification": "cargo test"
    }
  }
}"#,
    )
    .unwrap();
    path
}

fn write_status_fixture(dir: &TempDir) -> std::path::PathBuf {
    let path = write_fixture(dir);
    let mut fixture: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    fixture["repositories"] = serde_json::json!({
        "acme/widget": { "owner_type": "Organization", "accessible": true }
    });
    fixture["auth_message"] = serde_json::json!(
        "Logged in to github.com account agent (keyring)\nToken scopes: 'read:org', 'repo'"
    );
    fs::write(&path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
    path
}

fn set_configured_issue_field_ids(repo: &TestRepo) {
    let path = repo.board_path().join("config.toml");
    let mut config: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let github = config
        .as_table_mut()
        .unwrap()
        .entry("github")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let issue_fields = github
        .as_table_mut()
        .unwrap()
        .entry("issue_fields")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let issue_fields = issue_fields.as_table_mut().unwrap();
    issue_fields.insert("area_id".into(), toml::Value::String("AREA_ID".into()));
    issue_fields.insert(
        "verification_id".into(),
        toml::Value::String("VERIFICATION_ID".into()),
    );
    fs::write(path, toml::to_string(&config).unwrap()).unwrap();
}

fn adopt_fixture_task(repo: &TestRepo, fixture: &std::path::Path, title: &str) -> String {
    repo.init();
    write_github_config(repo);
    let id = repo.add(title, &[]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            id.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    id
}

fn set_fixture_issue_field(fixture: &std::path::Path, field: &str, value: serde_json::Value) {
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(fixture).unwrap()).unwrap();
    data["issues"]["acme/widget#42"][field] = value;
    fs::write(fixture, serde_json::to_vec_pretty(&data).unwrap()).unwrap();
}

fn set_task_frontmatter(repo: &TestRepo, id: &str, field: &str, value: &str) {
    let content = repo.read_task(id);
    let updated = content
        .lines()
        .map(|line| {
            if line.starts_with(&format!("{field}:")) {
                format!("{field}: {value}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    repo.write_task(id, &(updated + "\n"));
}

#[test]
fn github_repo_link_and_adopt_use_fixture_client() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);

    let link = repo.ok_json(&[
        "github",
        "repo",
        "link",
        "acme/widget",
        "--json",
    ]);
    assert_eq!(link["data"]["repository"], "acme/widget");

    let id = repo.add("Sync me", &[]);
    let adopt = repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            id.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(adopt["data"]["github"]["number"], 42);
    assert_eq!(adopt["data"]["issue"]["title"], "Imported issue");

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["github"]["repo"], "acme/widget");
    assert_eq!(show["data"]["title"], "Imported issue");
    assert_eq!(show["data"]["area"], "cli");
}

#[test]
fn github_setup_validate_only_reads_fixture_fields() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    let setup = repo.ok_json_with_env(
        &["github", "setup", "--validate-only", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(setup["data"]["area_id"], "AREA_ID");
    assert_eq!(setup["data"]["verification_id"], "VERIFICATION_ID");
}

#[test]
fn github_sync_dry_run_reports_assignee_warning() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    let id = repo.add("Local title", &[]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            id.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    let plan = repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(plan["data"]["count"], 1);
    let warnings = plan["data"]["tasks"][0]["plan"]["warnings"]
        .as_array()
        .unwrap();
    assert!(warnings
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("assignees")));
}

#[test]
fn github_connect_configures_organization_and_is_idempotent() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = fixture_dir.path().join("connect.json");
    fs::write(
        &fixture,
        r#"{
  "authenticated": true,
  "repositories": {
    "acme/widget": { "owner_type": "Organization", "accessible": true }
  },
  "org_fields": {
    "acme/Area": "AREA_ID",
    "acme/Verification": "VERIFICATION_ID"
  },
  "issues": {}
}"#,
    )
    .unwrap();
    let repo = TestRepo::new();
    repo.init();
    let config_path = repo.board_path().join("config.toml");
    let config = fs::read_to_string(&config_path).unwrap().replace(
        "[github.issue_fields]",
        "[github]\norganization = \"acme\"\nrepositories = [\"acme/other\"]\n\
[github.issue_fields]\narea_id = \"AREA_ID\"\nverification_id = \"VERIFICATION_ID\"",
    );
    fs::write(&config_path, config).unwrap();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/widget.git",
    ]);

    let first = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(first["data"]["owner_type"], "organization");
    assert_eq!(first["data"]["organization"], "acme");
    assert_eq!(first["data"]["repository"], "acme/widget");
    assert_eq!(first["data"]["field_status"]["area"], "ready");
    assert_eq!(first["data"]["field_status"]["verification"], "ready");
    assert_eq!(first["data"]["next_action"], "connected");
    assert_eq!(first["data"]["changed"], true);

    let config = fs::read_to_string(&config_path).unwrap();
    assert!(config.contains("organization = \"acme\""));
    assert!(config.contains("\"acme/other\""));
    assert!(config.contains("\"acme/widget\""));
    assert_eq!(config.matches("\"acme/widget\"").count(), 1);
    assert!(config.contains("area_id = \"AREA_ID\""));
    assert!(config.contains("verification_id = \"VERIFICATION_ID\""));

    let second = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(second["data"]["changed"], false);

    let human = repo.orbit_with_env(
        &["github", "connect"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stdout).contains("Configuration was already current"));
}

#[test]
fn github_connect_personal_owner_reports_choices_without_changing_config() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = fixture_dir.path().join("connect.json");
    fs::write(
        &fixture,
        r#"{
  "authenticated": true,
  "repositories": {
    "riley/widget": { "owner_type": "User", "accessible": true }
  },
  "org_fields": {},
  "issues": {}
}"#,
    )
    .unwrap();
    let repo = TestRepo::new();
    repo.init();
    repo.git(&["remote", "add", "origin", "git@github.com:riley/widget.git"]);

    let result = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(result["data"]["status"], "blocked");
    assert_eq!(result["data"]["owner_type"], "user");
    assert_eq!(result["data"]["repository"], "riley/widget");
    assert_eq!(
        result["data"]["next_action"],
        "choose_personal_account_path"
    );
    assert_eq!(result["data"]["next_actions"].as_array().unwrap().len(), 3);
    let config = fs::read_to_string(repo.board_path().join("config.toml")).unwrap();
    assert!(!config.contains("[github]"));
}

#[test]
fn github_connect_reports_authentication_access_and_field_blockers() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = fixture_dir.path().join("connect.json");
    let repo = TestRepo::new();
    repo.init();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/widget.git",
    ]);

    fs::write(
        &fixture,
        r#"{
  "authenticated": false,
  "auth_message": "not logged in",
  "repositories": {},
  "org_fields": {},
  "issues": {}
}"#,
    )
    .unwrap();
    let auth = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(auth["data"]["next_action"], "authenticate_gh");
    assert!(
        auth["data"]["remediation"]
            .as_str()
            .unwrap()
            .contains("gh auth login")
    );

    fs::write(
        &fixture,
        r#"{
  "authenticated": true,
  "repositories": {
    "acme/widget": { "owner_type": "Organization", "accessible": false }
  },
  "org_fields": {},
  "issues": {}
}"#,
    )
    .unwrap();
    let access = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(access["data"]["next_action"], "grant_repository_access");
    assert!(
        access["data"]["remediation"]
            .as_str()
            .unwrap()
            .contains("access")
    );

    fs::write(
        &fixture,
        r#"{
  "authenticated": true,
  "repositories": {
    "acme/widget": { "owner_type": "Organization", "accessible": true }
  },
  "org_fields": { "acme/Area": "AREA_ID" },
  "issues": {}
}"#,
    )
    .unwrap();
    let fields = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(fields["data"]["field_status"]["area"], "ready");
    assert_eq!(fields["data"]["field_status"]["verification"], "missing");
    assert_eq!(fields["data"]["next_action"], "create_issue_fields");
    assert!(
        fields["data"]["remediation"]
            .as_str()
            .unwrap()
            .contains("Verification")
    );
}

#[test]
fn github_sync_human_dry_run_marks_clean_plan_as_all_clear() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");

    let output = repo.orbit_with_env(
        &["github", "sync", id.as_str()],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("nothing was written"), "{human}");
    assert!(human.contains(&format!("Task: {id}")), "{human}");
    assert!(human.contains("acme/widget#42"), "{human}");
    assert!(human.contains("No changes needed"), "{human}");
}

#[test]
fn github_sync_human_dry_run_explains_empty_plan() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);

    let output = repo.orbit_with_env(
        &["github", "sync"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("No linked Tasks to sync"), "{human}");
    assert!(human.contains("nothing was written"), "{human}");
}

#[test]
fn github_sync_human_dry_run_lists_pull_changes() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_fixture_issue_field(&fixture, "title", serde_json::json!("Updated remotely"));
    set_fixture_issue_field(
        &fixture,
        "updated_at",
        serde_json::json!("2026-10-24T13:00:00Z"),
    );

    let output = repo.orbit_with_env(
        &["github", "sync", id.as_str()],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("Pull into Orbit:"), "{human}");
    assert!(human.contains("title: \"Updated remotely\""), "{human}");
    assert!(human.contains("Push to GitHub:\n  (none)"), "{human}");
    assert!(human.contains("nothing was written"), "{human}");
}

#[test]
fn github_sync_human_dry_run_lists_push_changes() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_task_frontmatter(&repo, &id, "area", "setup");
    set_task_frontmatter(&repo, &id, "updated_at", "2026-10-24T14:00:00Z");

    let output = repo.orbit_with_env(
        &["github", "sync", id.as_str(), "--direction", "push"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("Pull into Orbit:\n  (none)"), "{human}");
    assert!(human.contains("Push to GitHub:"), "{human}");
    assert!(human.contains("area: \"setup\""), "{human}");
    assert!(human.contains("nothing was written"), "{human}");
}

#[test]
fn github_sync_human_dry_run_shows_conflicting_values_and_keeps_json_plan() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_task_frontmatter(&repo, &id, "title", "\"Local edit\"");
    set_task_frontmatter(&repo, &id, "updated_at", "2026-11-01T14:00:00Z");
    set_fixture_issue_field(&fixture, "title", serde_json::json!("Remote edit"));
    set_fixture_issue_field(
        &fixture,
        "updated_at",
        serde_json::json!("2026-10-24T15:00:00Z"),
    );
    let task_before = repo.read_task(&id);
    let fixture_before = fs::read(&fixture).unwrap();

    let human_output = repo.orbit_with_env(
        &["github", "sync", id.as_str()],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(human_output.status.success());
    let human = String::from_utf8_lossy(&human_output.stdout);
    assert!(human.contains("Conflicts:"), "{human}");
    assert!(
        human.contains("title: Orbit = \"Local edit\"; GitHub = \"Remote edit\""),
        "{human}"
    );
    assert!(human.contains("nothing was written"), "{human}");
    assert_eq!(repo.read_task(&id), task_before);
    assert_eq!(fs::read(&fixture).unwrap(), fixture_before);

    let json = repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let plan = &json["data"]["tasks"][0]["plan"];
    let conflict = &plan["conflicts"][0];
    assert_eq!(json["data"]["apply"], false);
    assert_eq!(conflict["field"], "title");
    assert_eq!(conflict["orbit_value"], "Local edit");
    assert_eq!(conflict["github_value"], "Remote edit");
}

#[test]
fn github_adopt_accepts_issue_url_and_legacy_arguments() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config_for_repositories(&repo, &["acme/widget"]);
    let by_url = repo.add("URL issue", &[]);
    let adopted = repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            by_url.as_str(),
            "https://github.com/acme/widget/issues/42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(adopted["data"]["github"]["repo"], "acme/widget");
    assert_eq!(adopted["data"]["github"]["number"], 42);

    let legacy = repo.add("Legacy issue", &[]);
    let adopted = repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            legacy.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(adopted["data"]["github"]["repo"], "acme/widget");
    assert_eq!(adopted["data"]["github"]["number"], 42);
}

#[test]
fn github_adopt_rejects_malformed_urls_and_mismatched_identifiers() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config_for_repositories(&repo, &["acme/widget", "acme/other"]);

    let malformed_task = repo.add("Malformed URL", &[]);
    let (_, malformed) = repo.json_with_env(
        &[
            "github",
            "adopt",
            malformed_task.as_str(),
            "https://github.com/acme/widget/pull/42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(malformed["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        malformed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("GitHub Issue URL")
    );

    let mismatched_task = repo.add("Mismatched URL", &[]);
    let (_, mismatched) = repo.json_with_env(
        &[
            "github",
            "adopt",
            mismatched_task.as_str(),
            "https://github.com/acme/widget/issues/42",
            "--repo",
            "acme/other",
            "--number",
            "43",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(mismatched["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        mismatched["error"]["message"]
            .as_str()
            .unwrap()
            .contains("does not match")
    );

    let mismatched_number_task = repo.add("Mismatched issue number", &[]);
    let (_, mismatched_number) = repo.json_with_env(
        &[
            "github",
            "adopt",
            mismatched_number_task.as_str(),
            "https://github.com/acme/widget/issues/42",
            "--number",
            "43",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(mismatched_number["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        mismatched_number["error"]["message"]
            .as_str()
            .unwrap()
            .contains("issue number 43 does not match")
    );
}

#[test]
fn github_create_defaults_to_only_linked_repository() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config_for_repositories(&repo, &["acme/widget"]);
    let id = repo.add("Create issue", &[]);

    let created = repo.ok_json_with_env(
        &["github", "create", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(created["data"]["github"]["repo"], "acme/widget");
}

#[test]
fn github_create_requires_repo_when_selection_is_ambiguous_or_missing() {
    for repositories in [&["acme/widget", "acme/other"][..], &[][..]] {
        let repo = TestRepo::new();
        repo.init();
        write_github_config_for_repositories(&repo, repositories);
        let id = repo.add("Create issue", &[]);

        let (_, error) = repo.json(&["github", "create", id.as_str(), "--json"]);
        assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("--repo")
        );
    }

    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config_for_repositories(&repo, &["acme/widget", "acme/other"]);
    let id = repo.add("Explicit repository", &[]);
    let created = repo.ok_json_with_env(
        &[
            "github",
            "create",
            id.as_str(),
            "--repo",
            "acme/other",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(created["data"]["github"]["repo"], "acme/other");
}

#[test]
fn github_status_reports_healthy_configuration_and_is_read_only() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_status_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Imported issue");
    set_configured_issue_field_ids(&repo);
    let config_before = fs::read(repo.board_path().join("config.toml")).unwrap();
    let task_before = repo.read_task(&id);
    let fixture_before = fs::read(&fixture).unwrap();
    let board_status_before = repo.git_in(&repo.board_path(), &["status", "--porcelain"]);

    let status = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    assert_eq!(status["data"]["configured"], true);
    assert_eq!(status["data"]["auth"]["authenticated"], true);
    assert_eq!(status["data"]["auth"]["username"], "agent");
    assert_eq!(status["data"]["auth"]["scopes_known"], true);
    assert!(
        status["data"]["auth"]["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|scope| scope == "repo")
    );
    assert_eq!(status["data"]["organization"], "acme");
    assert_eq!(status["data"]["repositories"][0]["name"], "acme/widget");
    assert_eq!(status["data"]["fields"]["area"]["configured"], true);
    assert_eq!(status["data"]["fields"]["area"]["resolved"], true);
    assert_eq!(status["data"]["fields"]["verification"]["resolved"], true);
    assert_eq!(status["data"]["linked_tasks"], 1);
    assert_eq!(status["data"]["pending"]["pull"], 0);
    assert_eq!(status["data"]["pending"]["push"], 0);
    assert_eq!(status["data"]["conflicts"], 0);
    assert_eq!(status["data"]["unsupported_relationships"], 0);
    assert_eq!(status["data"]["next_action"], "none");
    assert_eq!(status["data"]["next_command"], serde_json::Value::Null);
    assert_eq!(
        fs::read(repo.board_path().join("config.toml")).unwrap(),
        config_before
    );
    assert_eq!(repo.read_task(&id), task_before);
    assert_eq!(fs::read(&fixture).unwrap(), fixture_before);
    let board_status_after = repo.git_in(&repo.board_path(), &["status", "--porcelain"]);
    assert_eq!(board_status_after.stdout, board_status_before.stdout);

    let human = repo.orbit_with_env(
        &["github", "status"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(human.status.success());
    let output = String::from_utf8_lossy(&human.stdout);
    assert!(output.contains("GitHub integration: healthy"), "{output}");
    assert!(output.contains("acme/widget"), "{output}");
    assert!(output.contains("scopes: read:org, repo"), "{output}");
}

#[test]
fn github_status_reports_unconfigured_and_missing_fields() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_status_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();

    let unconfigured = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(unconfigured["data"]["configured"], false);
    assert_eq!(unconfigured["data"]["next_action"], "orbit github connect");

    let mut fixture_data: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    fixture_data["auth_message"] = serde_json::json!(
        "Logged in to github.com account agent (keyring)\nToken scopes: 'repo'"
    );
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();
    let missing_scope = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(missing_scope["data"]["auth"]["missing_scopes"][0], "read:org");
    assert_eq!(
        missing_scope["data"]["next_command"],
        "gh auth refresh --hostname github.com --scopes read:org,repo"
    );

    fixture_data["auth_message"] = serde_json::json!(
        "Logged in to github.com account agent (keyring)\nToken scopes: 'read:org', 'repo'"
    );
    fixture_data["authenticated"] = serde_json::json!(false);
    fixture_data["auth_message"] = serde_json::json!("not logged in");
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();
    let unauthenticated = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(unauthenticated["data"]["auth"]["authenticated"], false);
    assert_eq!(unauthenticated["data"]["next_action"], "gh auth login");

    write_github_config(&repo);
    fixture_data["authenticated"] = serde_json::json!(true);
    fixture_data["org_fields"] = serde_json::json!({ "acme/Area": "AREA_ID" });
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();
    let missing = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(missing["data"]["fields"]["area"]["resolved"], true);
    assert_eq!(missing["data"]["fields"]["verification"]["resolved"], false);
    assert_eq!(missing["data"]["next_action"], "orbit github setup");
    assert_eq!(missing["data"]["next_command"], "orbit github setup");

    fixture_data["org_fields"] = serde_json::json!({});
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();
    let missing_both = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(missing_both["data"]["fields"]["area"]["resolved"], false);
    assert_eq!(
        missing_both["data"]["fields"]["verification"]["resolved"],
        false
    );
}

#[test]
fn github_status_reports_conflicts_and_doctor_reports_configuration_problems() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_status_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Imported issue");
    set_configured_issue_field_ids(&repo);
    set_task_frontmatter(&repo, &id, "title", "\"Orbit edit\"");
    set_task_frontmatter(&repo, &id, "area", "setup");
    set_task_frontmatter(&repo, &id, "updated_at", "2026-11-28T15:00:00Z");
    set_fixture_issue_field(&fixture, "title", serde_json::json!("GitHub edit"));
    set_fixture_issue_field(
        &fixture,
        "updated_at",
        serde_json::json!("2026-10-27T15:00:00Z"),
    );

    let status = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status["data"]["conflicts"], 1, "{status}");
    assert!(status["data"]["pending"]["pull"].as_u64().unwrap() > 0);
    assert!(status["data"]["pending"]["push"].as_u64().unwrap() > 0);
    assert_eq!(status["data"]["conflict_details"][0]["task"], id);
    assert_eq!(
        status["data"]["next_action"],
        "resolve_github_sync_conflicts"
    );
    assert_eq!(
        status["data"]["next_command"],
        format!("orbit github sync {id}")
    );

    let mut fixture_data: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    fixture_data["org_fields"] = serde_json::json!({});
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();
    let (exit, doctor) = repo.json_with_env(
        &["doctor", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(exit, 1);
    assert_eq!(doctor["data"]["tasks"], 1);
    assert!(
        doctor["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "GITHUB_SETUP_INCOMPLETE")
    );
}

#[test]
fn github_status_reports_unlinked_repositories_and_cross_repo_relationships() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_status_fixture(&fixture_dir);
    let mut fixture_data: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    fixture_data["repositories"]["acme/other"] =
        serde_json::json!({ "owner_type": "Organization", "accessible": true });
    let mut child_issue = fixture_data["issues"]["acme/widget#42"].clone();
    child_issue["repo"] = serde_json::json!("acme/other");
    child_issue["number"] = serde_json::json!(43);
    fixture_data["issues"]["acme/other#43"] = child_issue;
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();

    let repo = TestRepo::new();
    repo.init();
    write_github_config_for_repositories(&repo, &["acme/widget", "acme/other"]);
    let parent = repo.add("Parent", &[]);
    let child = repo.add("Child", &["--parent", parent.as_str()]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            parent.as_str(),
            "https://github.com/acme/widget/issues/42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            child.as_str(),
            "https://github.com/acme/other/issues/43",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    repo.ok_json(&["github", "repo", "unlink", "acme/other", "--json"]);

    let status = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    assert_eq!(status["data"]["unsupported_relationships"], 1);
    assert!(
        status["data"]["unsupported_details"][0]["message"]
            .as_str()
            .unwrap()
            .contains("cross-repository")
    );
    assert!(
        status["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "GITHUB_REPO_NOT_LINKED" && issue["task"] == child)
    );
}

#[test]
fn github_status_does_not_repair_an_orphan_board_during_discovery() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Orphaned task", &[]);
    fs::remove_dir_all(repo.board_path()).unwrap();

    let (status, result) = repo.json(&["github", "status", "--json"]);

    assert_ne!(status, 0);
    assert_eq!(result["ok"], false);
    assert!(!repo.board_path().exists());
}
