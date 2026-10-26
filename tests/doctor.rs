mod common;

use common::TestRepo;

fn trace_sources(value: &serde_json::Value) -> Vec<String> {
    value["data"]["discovery"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["source"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn doctor_runs_without_a_board_and_reports_the_discovery_trace() {
    let repo = TestRepo::new();

    let value = repo.ok_json(&["doctor", "--json"]);

    assert_eq!(value["data"]["healthy"], true);
    assert_eq!(value["data"]["tasks"], 0);
    assert!(value["data"]["issues"].as_array().unwrap().is_empty());
    assert!(value["data"]["discovery"]["root"].is_null());
    assert_eq!(value["data"]["discovery"]["source"], "none");
    let sources = trace_sources(&value);
    assert!(sources.contains(&"link".to_string()), "{sources:?}");
    assert!(sources.contains(&"nearest".to_string()), "{sources:?}");
    assert!(
        sources.contains(&"main_checkout".to_string()),
        "{sources:?}"
    );
    assert!(sources.contains(&"legacy_local".to_string()), "{sources:?}");
    assert!(sources.contains(&"global".to_string()), "{sources:?}");
}
