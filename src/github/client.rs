use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::{OrbitError, Result, code};
use crate::github::config::IssueFieldConfig;
use crate::github::model::{IssueSnapshot, IssueState};

pub struct FieldSetup {
    pub area_id: String,
    pub verification_id: String,
}

pub struct CreateIssueInput {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub area: Option<String>,
    pub verification: Option<String>,
}

pub struct UpdateIssueInput {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<IssueState>,
    pub labels: Option<Vec<String>>,
    pub area: Option<Option<String>>,
    pub verification: Option<Option<String>>,
}

pub trait GitHubClient: Send + Sync {
    fn validate_org_fields(
        &self,
        org: &str,
        fields: &IssueFieldConfig,
    ) -> Result<FieldSetup>;

    fn get_issue(&self, repo: &str, number: u64) -> Result<IssueSnapshot>;

    fn create_issue(&self, repo: &str, input: &CreateIssueInput) -> Result<IssueSnapshot>;

    fn update_issue(
        &self,
        repo: &str,
        number: u64,
        input: &UpdateIssueInput,
    ) -> Result<IssueSnapshot>;
}

pub fn client_from_env() -> Result<Box<dyn GitHubClient>> {
    if let Ok(path) = std::env::var("ORBIT_GITHUB_FIXTURE") {
        return Ok(Box::new(FixtureClient::load(Path::new(&path))?));
    }
    Ok(Box::new(GhCliClient))
}

struct GhCliClient;

impl GitHubClient for GhCliClient {
    fn validate_org_fields(
        &self,
        org: &str,
        fields: &IssueFieldConfig,
    ) -> Result<FieldSetup> {
        let query = format!(
            "organization(login:\"{org}\"){{ issueFields(first:100) {{ nodes {{ ... on IssueField {{ id name }} }} }} }}"
        );
        let output = gh_api(&["graphql", "-f", &format!("query={query}")])?;
        let response: GraphQlFieldsResponse = serde_json::from_slice(&output).map_err(|error| {
            OrbitError::new(
                code::GITHUB_SETUP_INCOMPLETE,
                format!("invalid GitHub response: {error}"),
            )
        })?;
        let nodes = response.data.organization.issue_fields.nodes;
        let area_id = find_field_id(&nodes, &fields.area_name).ok_or_else(|| {
            OrbitError::new(
                code::GITHUB_SETUP_INCOMPLETE,
                format!(
                    "organization `{org}` is missing Issue Field `{}`",
                    fields.area_name
                ),
            )
        })?;
        let verification_id = find_field_id(&nodes, &fields.verification_name).ok_or_else(|| {
            OrbitError::new(
                code::GITHUB_SETUP_INCOMPLETE,
                format!(
                    "organization `{org}` is missing Issue Field `{}`",
                    fields.verification_name
                ),
            )
        })?;
        Ok(FieldSetup {
            area_id,
            verification_id,
        })
    }

    fn get_issue(&self, repo: &str, number: u64) -> Result<IssueSnapshot> {
        let path = format!("/repos/{repo}/issues/{number}");
        let output = gh_api(&["api", &path])?;
        parse_rest_issue(repo, &output)
    }

    fn create_issue(&self, repo: &str, input: &CreateIssueInput) -> Result<IssueSnapshot> {
        if input.area.is_some() || input.verification.is_some() {
            return Err(OrbitError::new(
                code::GITHUB_SETUP_INCOMPLETE,
                "issue field values require `orbit github setup` before create/sync",
            ));
        }
        let payload = serde_json::json!({
            "title": input.title,
            "body": input.body,
            "labels": input.labels,
        })
        .to_string();
        let path = format!("/repos/{repo}/issues");
        let output = gh_api_with_input(
            &["api", &path, "-X", "POST", "--input", "-"],
            payload.as_bytes(),
        )?;
        parse_rest_issue(repo, &output)
    }

    fn update_issue(
        &self,
        repo: &str,
        number: u64,
        input: &UpdateIssueInput,
    ) -> Result<IssueSnapshot> {
        if input.area.is_some() || input.verification.is_some() {
            return Err(OrbitError::new(
                code::GITHUB_SETUP_INCOMPLETE,
                "issue field updates require configured field ids from `orbit github setup`",
            ));
        }
        let mut payload = serde_json::Map::new();
        if let Some(title) = &input.title {
            payload.insert("title".into(), serde_json::json!(title));
        }
        if let Some(body) = &input.body {
            payload.insert("body".into(), serde_json::json!(body));
        }
        if let Some(state) = input.state {
            payload.insert(
                "state".into(),
                serde_json::json!(match state {
                    IssueState::Open => "open",
                    IssueState::Closed => "closed",
                }),
            );
        }
        if let Some(labels) = &input.labels {
            payload.insert("labels".into(), serde_json::json!(labels));
        }
        let payload = serde_json::Value::Object(payload).to_string();
        let path = format!("/repos/{repo}/issues/{number}");
        let output = gh_api_with_input(
            &["api", &path, "-X", "PATCH", "--input", "-"],
            payload.as_bytes(),
        )?;
        parse_rest_issue(repo, &output)
    }
}

#[derive(Debug, Deserialize)]
struct GraphQlFieldsResponse {
    data: GraphQlOrganization,
}

#[derive(Debug, Deserialize)]
struct GraphQlOrganization {
    organization: GraphQlIssueFields,
}

#[derive(Debug, Deserialize)]
struct GraphQlIssueFields {
    #[serde(rename = "issueFields")]
    issue_fields: GraphQlNodes,
}

#[derive(Debug, Deserialize)]
struct GraphQlNodes {
    nodes: Vec<GraphQlIssueField>,
}

#[derive(Debug, Deserialize)]
struct GraphQlIssueField {
    id: String,
    name: String,
}

fn find_field_id(nodes: &[GraphQlIssueField], name: &str) -> Option<String> {
    nodes
        .iter()
        .find(|node| node.name.eq_ignore_ascii_case(name))
        .map(|node| node.id.clone())
}

fn gh_api(args: &[&str]) -> Result<Vec<u8>> {
    gh_api_with_input(args, &[])
}

fn gh_api_with_input(args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut command = Command::new("gh");
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        OrbitError::new(
            code::GITHUB_NOT_CONFIGURED,
            format!("failed to run `gh`: {error}"),
        )
    })?;
    if !input.is_empty() {
        child
            .stdin
            .take()
            .ok_or_else(|| OrbitError::git("failed to open gh stdin"))?
            .write_all(input)
            .map_err(|error| OrbitError::io("write gh stdin", error))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| OrbitError::io("wait for gh", error))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(OrbitError::new(code::GITHUB_NOT_CONFIGURED, stderr.trim()));
    }
    Ok(output.stdout)
}

#[derive(Debug, Deserialize)]
struct RestIssue {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    labels: Vec<RestLabel>,
    assignees: Vec<RestUser>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct RestLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RestUser {
    login: String,
}

fn parse_rest_issue(repo: &str, bytes: &[u8]) -> Result<IssueSnapshot> {
    let issue: RestIssue = serde_json::from_slice(bytes).map_err(|error| {
        OrbitError::new(
            code::GITHUB_NOT_CONFIGURED,
            format!("invalid issue payload: {error}"),
        )
    })?;
    Ok(IssueSnapshot {
        repo: repo.to_string(),
        number: issue.number,
        title: issue.title,
        body: issue.body.unwrap_or_default(),
        state: if issue.state == "open" {
            IssueState::Open
        } else {
            IssueState::Closed
        },
        labels: issue.labels.into_iter().map(|label| label.name).collect(),
        assignees: issue.assignees.into_iter().map(|user| user.login).collect(),
        updated_at: issue.updated_at,
        area: None,
        verification: None,
        parent_issue: Vec::new(),
        blocked_by: Vec::new(),
        blocking: Vec::new(),
    })
}

#[derive(Debug, Clone, Deserialize)]
struct FixtureStore {
    org_fields: HashMap<String, String>,
    issues: HashMap<String, IssueSnapshot>,
}

pub struct FixtureClient {
    store: Arc<Mutex<FixtureStore>>,
}

impl FixtureClient {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|error| OrbitError::io("read fixture", error))?;
        let store: FixtureStore = serde_json::from_str(&content).map_err(|error| {
            OrbitError::config(format!("invalid github fixture: {error}"))
        })?;
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
        })
    }
}

impl GitHubClient for FixtureClient {
    fn validate_org_fields(
        &self,
        org: &str,
        fields: &IssueFieldConfig,
    ) -> Result<FieldSetup> {
        let store = self.store.lock().expect("fixture lock");
        let area_key = format!("{org}/{}", fields.area_name);
        let verification_key = format!("{org}/{}", fields.verification_name);
        Ok(FieldSetup {
            area_id: store.org_fields.get(&area_key).cloned().ok_or_else(|| {
                OrbitError::new(code::GITHUB_SETUP_INCOMPLETE, "missing area field")
            })?,
            verification_id: store
                .org_fields
                .get(&verification_key)
                .cloned()
                .ok_or_else(|| {
                    OrbitError::new(code::GITHUB_SETUP_INCOMPLETE, "missing verification field")
                })?,
        })
    }

    fn get_issue(&self, repo: &str, number: u64) -> Result<IssueSnapshot> {
        let store = self.store.lock().expect("fixture lock");
        let key = format!("{repo}#{number}");
        store
            .issues
            .get(&key)
            .cloned()
            .ok_or_else(|| OrbitError::not_found(&key))
    }

    fn create_issue(&self, repo: &str, input: &CreateIssueInput) -> Result<IssueSnapshot> {
        let mut store = self.store.lock().expect("fixture lock");
        let number = store.issues.len() as u64 + 1;
        let snapshot = IssueSnapshot {
            repo: repo.to_string(),
            number,
            title: input.title.clone(),
            body: input.body.clone(),
            state: IssueState::Open,
            labels: input.labels.clone(),
            assignees: Vec::new(),
            updated_at: Utc::now(),
            area: input.area.clone(),
            verification: input.verification.clone(),
            parent_issue: Vec::new(),
            blocked_by: Vec::new(),
            blocking: Vec::new(),
        };
        store
            .issues
            .insert(format!("{repo}#{number}"), snapshot.clone());
        Ok(snapshot)
    }

    fn update_issue(
        &self,
        repo: &str,
        number: u64,
        input: &UpdateIssueInput,
    ) -> Result<IssueSnapshot> {
        let mut store = self.store.lock().expect("fixture lock");
        let key = format!("{repo}#{number}");
        let mut issue = store
            .issues
            .get(&key)
            .cloned()
            .ok_or_else(|| OrbitError::not_found(&key))?;
        if let Some(title) = &input.title {
            issue.title = title.clone();
        }
        if let Some(body) = &input.body {
            issue.body = body.clone();
        }
        if let Some(state) = input.state {
            issue.state = state;
        }
        if let Some(labels) = &input.labels {
            issue.labels = labels.clone();
        }
        if let Some(area) = &input.area {
            issue.area = area.clone();
        }
        if let Some(verification) = &input.verification {
            issue.verification = verification.clone();
        }
        issue.updated_at = Utc::now();
        store.issues.insert(key, issue.clone());
        Ok(issue)
    }
}
