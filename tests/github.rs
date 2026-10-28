mod common;

use common::TestRepo;
use std::fs;
use tempfile::TempDir;

fn write_github_config(repo: &TestRepo) {
    let path = repo.board_path().join("config.toml");
    let mut config = fs::read_to_string(&path).unwrap();
    config.push_str(
        "\n[github]\norganization = \"acme\"\nrepositories = [\"acme/widget\"]\n",
    );
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
