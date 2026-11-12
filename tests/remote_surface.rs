//! Remote Tracker material is hidden on Boards with no Remote Project and no
//! Task links (ADR 0018): the `ISSUE` column, the `show` remote line, and the
//! `list --help` flags all stay out of human output while `--json` is
//! unchanged.

mod common;

use common::TestRepo;

/// Append a github Remote Project to the Board's `[remote]` config.
fn configure_remote_project(repo: &TestRepo) {
    let path = repo.board_path().join("config.toml");
    let config = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        format!("{config}\n[[remote.projects]]\ntracker = \"github\"\nproject = \"acme/widget\"\n"),
    )
    .unwrap();
}

/// Link the Task to `acme/widget#<issue>` through a `remote:` frontmatter
/// entry, without configuring any Remote Project.
fn link_task_as(repo: &TestRepo, id: &str, issue: &str) {
    let content = repo.read_task(id);
    let linked = content.replacen(
        "---\n",
        &format!("---\nremote:\n  tracker: github\n  project: acme/widget\n  id: \"{issue}\"\n"),
        1,
    );
    assert_ne!(linked, content, "task {id} has no frontmatter block");
    repo.write_task(id, &linked);
}

fn link_task(repo: &TestRepo, id: &str) {
    link_task_as(repo, id, "42");
}

fn list_header(repo: &TestRepo) -> String {
    let output = repo.orbit(&["list"]);
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn list_hides_the_issue_column_without_remote_material() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Plain task", &[]);

    let stdout = list_header(&repo);
    let header = stdout.lines().next().unwrap();
    assert!(!header.contains("ISSUE"), "{stdout}");
    for expected in ["ID", "STATUS", "P", "TYPE", "TITLE", "LABELS", "CLAIMED BY"] {
        assert!(header.contains(expected), "{expected}: {stdout}");
    }
}

#[test]
fn list_shows_the_issue_column_when_a_remote_project_is_configured() {
    let repo = TestRepo::new();
    repo.init();
    configure_remote_project(&repo);
    repo.add("Plain task", &[]);

    let stdout = list_header(&repo);
    assert!(stdout.contains("ISSUE"), "{stdout}");
}

#[test]
fn list_shows_the_issue_column_when_a_task_is_linked() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Linked task", &[]);
    link_task(&repo, &id);
    repo.add("Plain task", &[]);

    let stdout = list_header(&repo);
    assert!(stdout.contains("ISSUE"), "{stdout}");
    let row = stdout
        .lines()
        .find(|line| line.contains(&id))
        .unwrap_or_else(|| panic!("no row for {id}: {stdout}"));
    assert!(row.contains("#42"), "{stdout}");
}

#[test]
fn list_shows_the_issue_column_for_legacy_github_repositories() {
    let repo = TestRepo::new();
    repo.init();
    let path = repo.board_path().join("config.toml");
    let config = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        format!("{config}\n[github]\nrepositories = [\"acme/widget\"]\n"),
    )
    .unwrap();
    repo.add("Plain task", &[]);

    let stdout = list_header(&repo);
    assert!(stdout.contains("ISSUE"), "{stdout}");
}

#[test]
fn show_hides_the_remote_line_without_remote_material() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Plain task", &[]);

    let output = repo.orbit(&["show", id.as_str()]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("github:"), "{stdout}");
    assert!(!stdout.contains("remote:"), "{stdout}");
}

#[test]
fn show_prints_the_remote_line_when_a_task_is_linked() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Linked task", &[]);
    link_task(&repo, &id);

    let output = repo.orbit(&["show", id.as_str()]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("github: acme/widget#42"), "{stdout}");
}

#[test]
fn show_prints_a_placeholder_for_unlinked_tasks_when_connected() {
    let repo = TestRepo::new();
    repo.init();
    configure_remote_project(&repo);
    let id = repo.add("Plain task", &[]);

    let output = repo.orbit(&["show", id.as_str()]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("github: -"), "{stdout}");
}

#[test]
fn json_output_is_unchanged_without_remote_material() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Plain task", &[]);

    let list = repo.ok_json(&["list", "--json"]);
    let row = &list["data"]["tasks"][0];
    assert!(row["github"].is_null(), "{row}");
    assert!(row["remote"].is_null(), "{row}");

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert!(show["data"]["github"].is_null(), "{show}");
    assert!(show["data"]["remote"].is_null(), "{show}");
}

#[test]
fn linked_and_local_flags_still_filter_without_remote_material() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("First", &[]);
    repo.add("Second", &[]);

    let linked = repo.ok_json(&["list", "--linked", "--json"]);
    assert_eq!(linked["data"]["count"], 0, "{linked}");

    let local = repo.ok_json(&["list", "--local", "--json"]);
    assert_eq!(local["data"]["count"], 2, "{local}");
}

#[test]
fn list_help_hides_remote_material_without_remote_details() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Plain task", &[]);

    for args in [
        &["list", "--help"][..],
        &["list", "-h"][..],
        &["help", "list"][..],
    ] {
        let output = repo.orbit(args);
        assert!(output.status.success(), "{args:?}");
        let help = String::from_utf8_lossy(&output.stdout);
        assert!(!help.contains("--linked"), "{args:?}: {help}");
        assert!(!help.contains("--local"), "{args:?}: {help}");
        assert!(!help.contains("ISSUE"), "{args:?}: {help}");
    }
}

#[test]
fn list_help_shows_remote_material_when_connected() {
    let repo = TestRepo::new();
    repo.init();
    configure_remote_project(&repo);
    repo.add("Plain task", &[]);

    let output = repo.orbit(&["list", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--linked"), "{help}");
    assert!(help.contains("--local"), "{help}");
    assert!(help.contains("ISSUE"), "{help}");
}

#[test]
fn list_help_shows_remote_material_when_a_task_is_linked() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Linked task", &[]);
    link_task(&repo, &id);

    let output = repo.orbit(&["list", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--linked"), "{help}");
    assert!(help.contains("--local"), "{help}");
}

#[test]
fn doctor_warns_once_for_links_without_remote_configuration() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("Linked one", &[]);
    let second = repo.add("Linked two", &[]);
    link_task_as(&repo, &first, "42");
    link_task_as(&repo, &second, "43");
    repo.add("Plain task", &[]);

    // A fixture that would fail if doctor tried to reach a tracker: the
    // unconfigured Board must not make remote calls at all.
    let output = repo.orbit_with_env(
        &["doctor", "--json"],
        &[(
            "ORBIT_GITHUB_FIXTURE",
            repo.path().join("missing-fixture.json").to_str().unwrap(),
        )],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["data"]["healthy"], true, "{json}");
    assert_eq!(
        json["data"]["issues"].as_array().unwrap().len(),
        0,
        "{json}"
    );

    let remote_warnings: Vec<&str> = json["data"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|warning| warning.as_str())
        .filter(|warning| warning.contains("Remote Tracker"))
        .collect();
    assert_eq!(remote_warnings.len(), 1, "{json}");
    assert!(remote_warnings[0].contains("2 task(s)"), "{json}");
    // The links stay untouched.
    assert!(repo.read_task(&first).contains("remote:"));
}

#[test]
fn doctor_stays_quiet_when_remote_is_configured() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Linked task", &[]);
    link_task(&repo, &id);
    configure_remote_project(&repo);

    let (_, json) = repo.json(&["doctor", "--json"]);
    let remote_warnings: Vec<&str> = json["data"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|warning| warning.as_str())
        .filter(|warning| warning.contains("Remote Tracker"))
        .collect();
    assert!(remote_warnings.is_empty(), "{json}");
}

#[test]
fn top_level_help_hides_no_sync() {
    let repo = TestRepo::new();
    repo.init();

    for args in [&["--help"][..], &["-h"][..], &["list", "--help"][..]] {
        let output = repo.orbit(args);
        assert!(output.status.success(), "{args:?}");
        let help = String::from_utf8_lossy(&output.stdout);
        assert!(!help.contains("--no-sync"), "{args:?}: {help}");
    }
}

#[test]
fn local_mutations_make_no_remote_calls() {
    let repo = TestRepo::new();
    repo.init();
    // A fixture path that cannot load: any attempt to build a Remote Tracker
    // client fails loudly instead of silently passing.
    let fixture = repo.path().join("missing-fixture.json");
    let fixture = fixture.to_str().unwrap();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture)];

    let value = repo.ok_json_with_env(&["add", "Local task", "--json"], &env);
    let id = value["data"]["id"].as_str().unwrap().to_string();
    let task = repo.read_task(&id);
    assert!(!task.contains("remote:"), "{task}");
    assert!(!task.contains("github:"), "{task}");

    repo.ok_json_with_env(&["done", &id, "--by", "test", "--json"], &env);
    repo.ok_json_with_env(&["prime", "--json"], &env);
    repo.ok_json_with_env(&["doctor", "--json"], &env);
}
