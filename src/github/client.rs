use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{OrbitError, Result, code};
use crate::github::config::IssueFieldConfig;
use crate::github::model::{IssueSnapshot, IssueState};

const AREA_FIELD_OPTIONS: [&str; 6] = ["board", "cli", "mcp", "setup", "docs", "release"];

pub struct FieldSetup {
    pub area_id: String,
    pub verification_id: String,
    pub created_fields: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AuthStatus {
    pub authenticated: bool,
    pub message: String,
    pub username: Option<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RepositoryInfo {
    pub owner_type: String,
}

pub struct CreateIssueInput {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub area: Option<String>,
    pub verification: Option<String>,
    pub organization: Option<String>,
    pub issue_fields: IssueFieldConfig,
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
    fn auth_status(&self) -> AuthStatus;

    fn repository_info(&self, repo: &str) -> Result<RepositoryInfo>;

    fn validate_org_fields(&self, org: &str, fields: &IssueFieldConfig) -> Result<FieldSetup>;

    fn create_org_fields(&self, org: &str, fields: &IssueFieldConfig) -> Result<FieldSetup>;

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
    fn auth_status(&self) -> AuthStatus {
        use std::process::Command;

        match Command::new("gh").args(["auth", "status"]).output() {
            Ok(output) => AuthStatus {
                authenticated: output.status.success(),
                message: String::from_utf8_lossy(if output.status.success() {
                    &output.stdout
                } else {
                    &output.stderr
                })
                .trim()
                .to_string(),
                username: None,
                scopes: Vec::new(),
            },
            Err(error) => AuthStatus {
                authenticated: false,
                message: format!("could not run `gh auth status`: {error}"),
                username: None,
                scopes: Vec::new(),
            },
        }
    }

    fn repository_info(&self, repo: &str) -> Result<RepositoryInfo> {
        #[derive(Deserialize)]
        struct RepositoryResponse {
            owner: RepositoryOwnerResponse,
        }
        #[derive(Deserialize)]
        struct RepositoryOwnerResponse {
            #[serde(rename = "type")]
            owner_type: String,
        }

        let path = format!("/repos/{repo}");
        let output = gh_api(&["api", &path])?;
        let response: RepositoryResponse = serde_json::from_slice(&output).map_err(|error| {
            OrbitError::new(
                code::GITHUB_NOT_CONFIGURED,
                format!("invalid GitHub repository response: {error}"),
            )
        })?;
        Ok(RepositoryInfo {
            owner_type: response.owner.owner_type,
        })
    }

    fn validate_org_fields(&self, org: &str, fields: &IssueFieldConfig) -> Result<FieldSetup> {
        let query = format!(
            "query {{ organization(login:\"{org}\"){{ issueFields(first:100) {{ nodes {{ ... on IssueFieldDate {{ id name }} ... on IssueFieldMultiSelect {{ id name }} ... on IssueFieldNumber {{ id name }} ... on IssueFieldSingleSelect {{ id name }} ... on IssueFieldText {{ id name }} }} }} }} }}"
        );
        let output = gh_api(&["api", "graphql", "-f", &format!("query={query}")])?;
        let response: GraphQlFieldsResponse = serde_json::from_slice(&output).map_err(|error| {
            OrbitError::new(
                code::GITHUB_SETUP_INCOMPLETE,
                format!("invalid GitHub response: {error}"),
            )
        })?;
        let nodes = response.data.organization.issue_fields.nodes;
        let area_id = find_field_id(&nodes, &fields.area_name);
        let verification_id = find_field_id(&nodes, &fields.verification_name);
        let mut missing = Vec::new();
        let mut missing_fields = Vec::new();
        if area_id.is_none() {
            missing.push(format!(
                "organization `{org}` is missing Issue Field `{}`",
                fields.area_name
            ));
            missing_fields.push(fields.area_name.as_str());
        }
        if verification_id.is_none() {
            missing.push(format!(
                "organization `{org}` is missing Issue Field `{}`",
                fields.verification_name
            ));
            missing_fields.push(fields.verification_name.as_str());
        }
        if !missing.is_empty() {
            return Err(
                OrbitError::new(code::GITHUB_SETUP_INCOMPLETE, missing.join("; ")).with_details(
                    serde_json::json!({
                        "organization": org,
                        "fields": missing_fields,
                    }),
                ),
            );
        }
        Ok(FieldSetup {
            area_id: area_id.expect("missing Area field was reported"),
            verification_id: verification_id.expect("missing Verification field was reported"),
            created_fields: Vec::new(),
        })
    }

    fn create_org_fields(&self, org: &str, fields: &IssueFieldConfig) -> Result<FieldSetup> {
        validate_org_admin(org)?;
        let query = "query($org: String!) { organization(login: $org) { id issueFields(first: 100) { nodes { ... on IssueFieldDate { id name } ... on IssueFieldMultiSelect { id name } ... on IssueFieldNumber { id name } ... on IssueFieldSingleSelect { id name } ... on IssueFieldText { id name } } } } }";
        let output = gh_api(&[
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &format!("org={org}"),
        ])?;
        let response: GraphQlSetupResponse = serde_json::from_slice(&output).map_err(|error| {
            OrbitError::new(
                code::GITHUB_SETUP_INCOMPLETE,
                format!("invalid GitHub organization response: {error}"),
            )
        })?;
        let organization = response.data.organization.ok_or_else(|| {
            OrbitError::new(
                code::GITHUB_SETUP_INCOMPLETE,
                format!("GitHub organization `{org}` was not found or is not accessible"),
            )
        })?;
        let mut area_id = find_field_id(&organization.issue_fields.nodes, &fields.area_name);
        let mut verification_id =
            find_field_id(&organization.issue_fields.nodes, &fields.verification_name);
        let mut created_fields = Vec::new();
        if area_id.is_none() {
            area_id = Some(create_issue_field(
                &organization.id,
                &fields.area_name,
                "SINGLE_SELECT",
                Some(&AREA_FIELD_OPTIONS),
            )?);
            created_fields.push(fields.area_name.clone());
        }
        if verification_id.is_none() {
            verification_id = Some(create_issue_field(
                &organization.id,
                &fields.verification_name,
                "TEXT",
                None,
            )?);
            created_fields.push(fields.verification_name.clone());
        }
        Ok(FieldSetup {
            area_id: area_id.expect("Area field id was resolved or created"),
            verification_id: verification_id
                .expect("Verification field id was resolved or created"),
            created_fields,
        })
    }

    fn get_issue(&self, repo: &str, number: u64) -> Result<IssueSnapshot> {
        let path = format!("/repos/{repo}/issues/{number}");
        let output = gh_api(&["api", &path])?;
        parse_rest_issue(repo, &output)
    }

    fn create_issue(&self, repo: &str, input: &CreateIssueInput) -> Result<IssueSnapshot> {
        let mut payload = serde_json::json!({
            "title": input.title,
            "body": input.body,
            "labels": input.labels,
        });
        let mut field_values = Vec::new();
        if input.area.is_some() || input.verification.is_some() {
            if input.area.is_some() && input.issue_fields.area_id.is_none()
                || input.verification.is_some() && input.issue_fields.verification_id.is_none()
            {
                return Err(OrbitError::new(
                    code::GITHUB_SETUP_INCOMPLETE,
                    "issue field values require resolved ids from `orbit github setup`",
                ));
            }
            let org = input
                .organization
                .as_deref()
                .filter(|org| !org.is_empty())
                .ok_or_else(|| {
                    OrbitError::new(
                        code::GITHUB_SETUP_INCOMPLETE,
                        "issue field values require `github.organization` and `orbit github setup`",
                    )
                })?;
            let fields = list_rest_issue_fields(org)?;
            if let Some(area) = input.area.as_deref() {
                let field_id = rest_issue_field_id(&fields, &input.issue_fields.area_name).ok_or_else(|| {
                    OrbitError::new(
                        code::GITHUB_SETUP_INCOMPLETE,
                        format!("organization `{org}` is missing Issue Field `{}`; run `orbit github setup`", input.issue_fields.area_name),
                    )
                })?;
                field_values.push(serde_json::json!({ "field_id": field_id, "value": area }));
            }
            if let Some(verification) = input.verification.as_deref() {
                let field_id = rest_issue_field_id(&fields, &input.issue_fields.verification_name).ok_or_else(|| {
                    OrbitError::new(
                        code::GITHUB_SETUP_INCOMPLETE,
                        format!("organization `{org}` is missing Issue Field `{}`; run `orbit github setup`", input.issue_fields.verification_name),
                    )
                })?;
                field_values
                    .push(serde_json::json!({ "field_id": field_id, "value": verification }));
            }
            payload["issue_field_values"] = serde_json::Value::Array(field_values);
        }
        let payload = payload.to_string();
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
struct RestIssueField {
    id: u64,
    name: String,
}

fn list_rest_issue_fields(org: &str) -> Result<Vec<RestIssueField>> {
    let path = format!("/orgs/{org}/issue-fields");
    let output = gh_api(&["api", &path])?;
    serde_json::from_slice(&output).map_err(|error| {
        OrbitError::new(
            code::GITHUB_SETUP_INCOMPLETE,
            format!("invalid GitHub organization Issue Fields response: {error}"),
        )
    })
}

fn rest_issue_field_id(fields: &[RestIssueField], name: &str) -> Option<u64> {
    fields
        .iter()
        .find(|field| field.name.eq_ignore_ascii_case(name))
        .map(|field| field.id)
}

fn validate_org_admin(org: &str) -> Result<()> {
    #[derive(Deserialize)]
    struct Membership {
        role: String,
        state: String,
    }

    if org.is_empty()
        || !org
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(OrbitError::invalid_argument(
            "github.organization must be a GitHub organization login containing only letters, numbers, and hyphens",
        ));
    }
    let endpoint = format!("/user/memberships/orgs/{org}");
    let output = gh_api(&["api", &endpoint]).map_err(|error| {
        OrbitError::new(
            code::GITHUB_SETUP_INCOMPLETE,
            format!(
                "could not verify organization administrator access for `{org}`: {}. Refresh `gh` with `gh auth refresh --hostname github.com --scopes read:org`, then retry; an organization administrator can create the fields in GitHub and run `orbit github setup`.",
                error.message
            ),
        )
    })?;
    let membership: Membership = serde_json::from_slice(&output).map_err(|error| {
        OrbitError::new(
            code::GITHUB_SETUP_INCOMPLETE,
            format!(
                "could not verify organization administrator access for `{org}`: invalid membership response ({error}). Refresh `gh` with `gh auth refresh --hostname github.com --scopes read:org`, then retry."
            ),
        )
    })?;
    if membership.role != "admin" || membership.state != "active" {
        return Err(missing_org_admin_permission(org));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct GraphQlFieldsResponse {
    data: GraphQlOrganization,
}

#[derive(Debug, Deserialize)]
struct GraphQlSetupResponse {
    data: GraphQlSetupData,
}

#[derive(Debug, Deserialize)]
struct GraphQlSetupData {
    organization: Option<GraphQlSetupOrganization>,
}

#[derive(Debug, Deserialize)]
struct GraphQlSetupOrganization {
    id: String,
    #[serde(rename = "issueFields")]
    issue_fields: GraphQlNodes,
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

fn create_issue_field(
    organization_id: &str,
    name: &str,
    data_type: &str,
    options: Option<&[&str]>,
) -> Result<String> {
    let organization_id = serde_json::to_string(organization_id).map_err(|error| {
        OrbitError::new(
            code::GITHUB_SETUP_INCOMPLETE,
            format!("could not encode GitHub organization id: {error}"),
        )
    })?;
    let name = serde_json::to_string(name).map_err(|error| {
        OrbitError::new(
            code::GITHUB_SETUP_INCOMPLETE,
            format!("could not encode GitHub Issue Field name: {error}"),
        )
    })?;
    let options = options.map_or_else(String::new, |options| {
        let options = options
            .iter()
            .enumerate()
            .map(|(priority, name)| {
                format!(
                    "{{ name: {}, color: GRAY, priority: {priority} }}",
                    serde_json::to_string(name).expect("static option name is valid JSON")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(", options: [{options}]")
    });
    let id_fragment = match data_type {
        "SINGLE_SELECT" => "... on IssueFieldSingleSelect { id }",
        "TEXT" => "... on IssueFieldText { id }",
        _ => "... on IssueFieldSingleSelect { id } ... on IssueFieldText { id }",
    };
    let query = format!(
        "mutation {{ createIssueField(input: {{ ownerId: {organization_id}, name: {name}, dataType: {data_type}{options} }}) {{ issueField {{ {id_fragment} }} }} }}"
    );
    let output = gh_api(&["api", "graphql", "-f", &format!("query={query}")]).map_err(|error| {
        OrbitError::new(
            code::GITHUB_SETUP_INCOMPLETE,
            format!(
                "GitHub could not create Issue Field `{}`: {}. Confirm `gh` is authenticated as an organization administrator and can create organization Issue Fields, then retry `orbit github setup --create-fields`; alternatively create the fields in GitHub and run `orbit github setup`.",
                name.trim_matches('"'),
                error.message
            ),
        )
    })?;
    let response: GraphQlCreateFieldResponse =
        serde_json::from_slice(&output).map_err(|error| {
            OrbitError::new(
                code::GITHUB_SETUP_INCOMPLETE,
                format!("invalid GitHub Issue Field creation response: {error}"),
            )
        })?;
    Ok(response.data.create_issue_field.issue_field.id)
}

fn missing_org_admin_permission(org: &str) -> OrbitError {
    OrbitError::new(
        code::GITHUB_SETUP_INCOMPLETE,
        format!(
            "creating GitHub Issue Fields for `{org}` requires organization administrator access. Ask an organization administrator to run `orbit github setup --create-fields`, or create the fields in GitHub and run `orbit github setup`."
        ),
    )
}

#[derive(Debug, Deserialize)]
struct GraphQlCreateFieldResponse {
    data: GraphQlCreateFieldData,
}

#[derive(Debug, Deserialize)]
struct GraphQlCreateFieldData {
    #[serde(rename = "createIssueField")]
    create_issue_field: GraphQlCreateFieldPayload,
}

#[derive(Debug, Deserialize)]
struct GraphQlCreateFieldPayload {
    #[serde(rename = "issueField")]
    issue_field: GraphQlCreatedField,
}

#[derive(Debug, Deserialize)]
struct GraphQlCreatedField {
    id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FixtureStore {
    #[serde(default = "fixture_authenticated")]
    authenticated: bool,
    #[serde(default)]
    auth_message: String,
    #[serde(default = "fixture_organization_admin")]
    organization_admin: bool,
    #[serde(default)]
    auth_username: Option<String>,
    #[serde(default)]
    auth_scopes: Vec<String>,
    #[serde(default)]
    repositories: HashMap<String, FixtureRepository>,
    #[serde(default)]
    org_fields: HashMap<String, String>,
    #[serde(default)]
    created_fields: Vec<serde_json::Value>,
    #[serde(default)]
    issues: HashMap<String, IssueSnapshot>,
    #[serde(default)]
    create_failures: HashMap<String, String>,
    #[serde(skip)]
    fixture_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FixtureRepository {
    owner_type: String,
    #[serde(default = "fixture_accessible")]
    accessible: bool,
}

fn fixture_authenticated() -> bool {
    true
}

fn fixture_accessible() -> bool {
    true
}

fn fixture_organization_admin() -> bool {
    true
}

pub struct FixtureClient {
    store: Arc<Mutex<FixtureStore>>,
}

impl FixtureClient {
    pub fn load(path: &Path) -> Result<Self> {
        let content =
            std::fs::read_to_string(path).map_err(|error| OrbitError::io("read fixture", error))?;
        let store: FixtureStore = serde_json::from_str(&content)
            .map_err(|error| OrbitError::config(format!("invalid github fixture: {error}")))?;
        Ok(Self {
            store: Arc::new(Mutex::new(FixtureStore {
                fixture_path: path.to_path_buf(),
                ..store
            })),
        })
    }
}

impl GitHubClient for FixtureClient {
    fn auth_status(&self) -> AuthStatus {
        let store = self.store.lock().expect("fixture lock");
        AuthStatus {
            authenticated: store.authenticated,
            message: store.auth_message.clone(),
            username: store.auth_username.clone(),
            scopes: store.auth_scopes.clone(),
        }
    }

    fn repository_info(&self, repo: &str) -> Result<RepositoryInfo> {
        let store = self.store.lock().expect("fixture lock");
        let repository = store.repositories.get(repo).ok_or_else(|| {
            OrbitError::new(
                code::GITHUB_NOT_CONFIGURED,
                format!("repository `{repo}` was not found or is not accessible"),
            )
        })?;
        if !repository.accessible {
            return Err(OrbitError::new(
                code::GITHUB_NOT_CONFIGURED,
                format!("repository `{repo}` was not found or is not accessible"),
            ));
        }
        Ok(RepositoryInfo {
            owner_type: repository.owner_type.clone(),
        })
    }

    fn validate_org_fields(&self, org: &str, fields: &IssueFieldConfig) -> Result<FieldSetup> {
        let store = self.store.lock().expect("fixture lock");
        let area_key = format!("{org}/{}", fields.area_name);
        let verification_key = format!("{org}/{}", fields.verification_name);
        let area_id = store.org_fields.get(&area_key).cloned();
        let verification_id = store.org_fields.get(&verification_key).cloned();
        let mut missing = Vec::new();
        let mut missing_fields = Vec::new();
        if area_id.is_none() {
            missing.push(format!(
                "organization `{org}` is missing Issue Field `{}`",
                fields.area_name
            ));
            missing_fields.push(fields.area_name.as_str());
        }
        if verification_id.is_none() {
            missing.push(format!(
                "organization `{org}` is missing Issue Field `{}`",
                fields.verification_name
            ));
            missing_fields.push(fields.verification_name.as_str());
        }
        if !missing.is_empty() {
            return Err(
                OrbitError::new(code::GITHUB_SETUP_INCOMPLETE, missing.join("; ")).with_details(
                    serde_json::json!({
                        "organization": org,
                        "fields": missing_fields,
                    }),
                ),
            );
        }
        Ok(FieldSetup {
            area_id: area_id.expect("missing Area field was reported"),
            verification_id: verification_id.expect("missing Verification field was reported"),
            created_fields: Vec::new(),
        })
    }

    fn create_org_fields(&self, org: &str, fields: &IssueFieldConfig) -> Result<FieldSetup> {
        let mut store = self.store.lock().expect("fixture lock");
        if !store.organization_admin {
            return Err(missing_org_admin_permission(org));
        }
        let mut created_fields = Vec::new();
        let area_key = format!("{org}/{}", fields.area_name);
        let area_id = if let Some(id) = store.org_fields.get(&area_key) {
            id.clone()
        } else {
            let id = fixture_created_field_id(&fields.area_name, true);
            store.org_fields.insert(area_key, id.clone());
            store.created_fields.push(serde_json::json!({
                "name": fields.area_name,
                "data_type": "SINGLE_SELECT",
                "options": AREA_FIELD_OPTIONS,
            }));
            created_fields.push(fields.area_name.clone());
            id
        };
        let verification_key = format!("{org}/{}", fields.verification_name);
        let verification_id = if let Some(id) = store.org_fields.get(&verification_key) {
            id.clone()
        } else {
            let id = fixture_created_field_id(&fields.verification_name, false);
            store.org_fields.insert(verification_key, id.clone());
            store.created_fields.push(serde_json::json!({
                "name": fields.verification_name,
                "data_type": "TEXT",
                "options": [],
            }));
            created_fields.push(fields.verification_name.clone());
            id
        };
        let setup = FieldSetup {
            area_id,
            verification_id,
            created_fields,
        };
        let serialized = serde_json::to_vec_pretty(&*store).map_err(|error| {
            OrbitError::config(format!("could not serialize GitHub fixture: {error}"))
        })?;
        std::fs::write(&store.fixture_path, serialized)
            .map_err(|error| OrbitError::io("write GitHub fixture", error))?;
        Ok(setup)
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
        if let Some(message) = store
            .create_failures
            .get(&format!("{repo}#{}", input.title))
            .or_else(|| store.create_failures.get(repo))
        {
            return Err(OrbitError::new(
                code::GITHUB_NOT_CONFIGURED,
                message.clone(),
            ));
        }
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
        persist_fixture(&store)?;
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
        persist_fixture(&store)?;
        Ok(issue)
    }
}

fn persist_fixture(store: &FixtureStore) -> Result<()> {
    let serialized = serde_json::to_vec_pretty(store).map_err(|error| {
        OrbitError::config(format!("could not serialize GitHub fixture: {error}"))
    })?;
    std::fs::write(&store.fixture_path, serialized)
        .map_err(|error| OrbitError::io("write GitHub fixture", error))
}

fn fixture_created_field_id(name: &str, area: bool) -> String {
    if area {
        return "CREATED_AREA_ID".to_string();
    }
    if name.eq_ignore_ascii_case("Verification") {
        return "CREATED_VERIFICATION_ID".to_string();
    }
    format!(
        "CREATED_{}_ID",
        name.chars()
            .map(|character| if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            })
            .collect::<String>()
    )
}
