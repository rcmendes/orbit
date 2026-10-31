use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{OrbitError, Result, code};
use crate::github::config::IssueFieldConfig;
use crate::github::model::{IssueSnapshot, IssueState, IssueStateReason};
use crate::github::relationships::{RelationshipChange, SupportedRelationship};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueCatalogItem {
    #[serde(default)]
    pub identity: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub state: IssueState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<IssueStateReason>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub issue_fields: BTreeMap<String, serde_json::Value>,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
}

impl IssueCatalogItem {
    pub fn identity(&self) -> String {
        if self.identity.is_empty() {
            format!("{}#{}", self.repo, self.number)
        } else {
            self.identity.clone()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueComment {
    pub id: u64,
    pub body: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
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
    pub state_reason: Option<IssueStateReason>,
    pub labels: Option<Vec<String>>,
    pub area: Option<Option<String>>,
    pub verification: Option<Option<String>>,
    pub organization: Option<String>,
    pub issue_fields: Option<IssueFieldConfig>,
}

pub trait GitHubClient: Send + Sync {
    fn auth_status(&self) -> AuthStatus;

    fn repository_info(&self, repo: &str) -> Result<RepositoryInfo>;

    fn validate_org_fields(&self, org: &str, fields: &IssueFieldConfig) -> Result<FieldSetup>;

    fn create_org_fields(&self, org: &str, fields: &IssueFieldConfig) -> Result<FieldSetup>;

    fn get_issue(
        &self,
        repo: &str,
        number: u64,
        issue_fields: Option<&IssueFieldConfig>,
    ) -> Result<IssueSnapshot>;

    fn list_issues_page(
        &self,
        repo: &str,
        page: usize,
        issue_fields: Option<&IssueFieldConfig>,
    ) -> Result<Vec<IssueCatalogItem>>;

    /// Lists every issue in the repository, following pages until exhausted.
    /// Pagination stays inside the seam; callers see the whole catalog.
    fn list_all_issues(
        &self,
        repo: &str,
        issue_fields: Option<&IssueFieldConfig>,
    ) -> Result<Vec<IssueCatalogItem>> {
        let mut issues = Vec::new();
        let mut page = 1;
        loop {
            let results = self.list_issues_page(repo, page, issue_fields)?;
            if results.is_empty() {
                break;
            }
            issues.extend(results);
            page += 1;
        }
        Ok(issues)
    }

    fn issue_comments_page(
        &self,
        repo: &str,
        number: u64,
        page: usize,
    ) -> Result<Vec<IssueComment>>;

    /// Lists every comment on an issue, following pages until exhausted.
    fn issue_comments(&self, repo: &str, number: u64) -> Result<Vec<IssueComment>> {
        let mut comments = Vec::new();
        let mut page = 1;
        loop {
            let results = self.issue_comments_page(repo, number, page)?;
            if results.is_empty() {
                break;
            }
            comments.extend(results);
            page += 1;
        }
        Ok(comments)
    }

    fn create_issue(&self, repo: &str, input: &CreateIssueInput) -> Result<IssueSnapshot>;

    fn update_issue(
        &self,
        repo: &str,
        number: u64,
        input: &UpdateIssueInput,
    ) -> Result<IssueSnapshot>;

    /// Ensure an Orbit-managed label exists on the repository. An existing
    /// label is left untouched.
    fn ensure_label(&self, repo: &str, name: &str) -> Result<()>;

    fn label_exists(&self, repo: &str, name: &str) -> Result<bool>;

    /// Reconcile an issue with its desired relationships.
    ///
    /// Entries are filtered by kind (sub_issue affects the sub-issue
    /// list, blocked_by the blocker list) and matched by issue number
    /// only: the sync planner guarantees same-repo references. Desired
    /// relationships are ensured present (existing ones are left
    /// untouched); removals are dropped only when currently present.
    /// Relationships the caller does not mention are preserved, and a
    /// reference listed in both ends up removed.
    fn update_relationships(
        &self,
        repo: &str,
        number: u64,
        desired: &[SupportedRelationship],
        removals: &[RelationshipChange],
    ) -> Result<()>;
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
            Ok(output) => {
                let message = String::from_utf8_lossy(if output.status.success() {
                    &output.stdout
                } else {
                    &output.stderr
                })
                .trim()
                .to_string();
                AuthStatus {
                    authenticated: output.status.success(),
                    username: parse_auth_username(&message),
                    scopes: parse_auth_scopes(&message),
                    message,
                }
            }
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

    fn get_issue(
        &self,
        repo: &str,
        number: u64,
        issue_fields: Option<&IssueFieldConfig>,
    ) -> Result<IssueSnapshot> {
        let path = format!("/repos/{repo}/issues/{number}");
        let output = gh_api(&["api", &path])?;
        let mut issue = parse_rest_issue(repo, &output)?;
        if let Some(issue_fields) = issue_fields {
            let values = list_issue_field_values(repo, number)?;
            issue.area = string_field(&values, &issue_fields.area_name);
            issue.verification = string_field(&values, &issue_fields.verification_name);
        }
        issue.blocking = list_issue_relationships(repo, number, "sub_issues")?
            .iter()
            .filter_map(|item| {
                item.get("number")
                    .and_then(serde_json::Value::as_u64)
                    .map(|number| crate::github::model::IssueRef {
                        repo: repo.to_string(),
                        number,
                    })
            })
            .collect();
        issue.blocked_by = list_issue_relationships(repo, number, "dependencies/blocked_by")?
            .iter()
            .filter_map(|item| {
                item.get("number")
                    .and_then(serde_json::Value::as_u64)
                    .map(|number| crate::github::model::IssueRef {
                        repo: repo.to_string(),
                        number,
                    })
            })
            .collect();
        Ok(issue)
    }

    fn list_issues_page(
        &self,
        repo: &str,
        page: usize,
        issue_fields: Option<&IssueFieldConfig>,
    ) -> Result<Vec<IssueCatalogItem>> {
        let path = format!("/repos/{repo}/issues?state=all&per_page=100&page={page}");
        let output = gh_api(&["api", &path])?;
        let issues: Vec<RestCatalogIssue> = serde_json::from_slice(&output).map_err(|error| {
            OrbitError::new(
                code::GITHUB_NOT_CONFIGURED,
                format!("invalid GitHub issue catalog response: {error}"),
            )
        })?;
        let mut summaries = Vec::new();
        for issue in issues
            .into_iter()
            .filter(|issue| issue.pull_request.is_none())
        {
            let field_values = list_issue_field_values(repo, issue.number)?;
            summaries.push(IssueCatalogItem {
                identity: format!("{repo}#{}", issue.number),
                repo: repo.to_string(),
                number: issue.number,
                title: issue.title,
                state: if issue.state == "open" {
                    IssueState::Open
                } else {
                    IssueState::Closed
                },
                state_reason: issue.state_reason,
                labels: issue.labels.into_iter().map(|label| label.name).collect(),
                url: issue.html_url,
                created_at: issue.created_at,
                updated_at: issue.updated_at,
                area: issue.area.or_else(|| {
                    issue_fields.and_then(|fields| string_field(&field_values, &fields.area_name))
                }),
                verification: issue.verification.or_else(|| {
                    issue_fields
                        .and_then(|fields| string_field(&field_values, &fields.verification_name))
                }),
                issue_fields: field_values,
            });
        }
        Ok(summaries)
    }

    fn issue_comments_page(
        &self,
        repo: &str,
        number: u64,
        page: usize,
    ) -> Result<Vec<IssueComment>> {
        let path = format!("/repos/{repo}/issues/{number}/comments?per_page=100&page={page}");
        let output = gh_api(&["api", &path])?;
        let comments: Vec<RestIssueComment> = serde_json::from_slice(&output).map_err(|error| {
            OrbitError::new(
                code::GITHUB_NOT_CONFIGURED,
                format!("invalid GitHub comments response: {error}"),
            )
        })?;
        Ok(comments
            .into_iter()
            .map(|comment| IssueComment {
                id: comment.id,
                body: comment.body,
                author: comment.user.login,
                created_at: comment.created_at,
            })
            .collect())
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
        if let Some(reason) = input.state_reason {
            payload.insert("state_reason".into(), serde_json::json!(reason));
        }
        if let Some(labels) = &input.labels {
            payload.insert("labels".into(), serde_json::json!(labels));
        }
        if !payload.is_empty() {
            let payload = serde_json::Value::Object(payload).to_string();
            let path = format!("/repos/{repo}/issues/{number}");
            gh_api_with_input(
                &["api", &path, "-X", "PATCH", "--input", "-"],
                payload.as_bytes(),
            )?;
        }
        if input.area.is_some() || input.verification.is_some() {
            let issue_fields = input.issue_fields.as_ref().ok_or_else(|| {
                OrbitError::new(
                    code::GITHUB_SETUP_INCOMPLETE,
                    "issue field updates require configured field names from `orbit github setup`",
                )
            })?;
            let organization = input.organization.as_deref().ok_or_else(|| {
                OrbitError::new(
                    code::GITHUB_SETUP_INCOMPLETE,
                    "issue field updates require github.organization and `orbit github setup`",
                )
            })?;
            update_rest_issue_field_values(
                repo,
                number,
                organization,
                issue_fields,
                input.area.as_ref(),
                input.verification.as_ref(),
            )?;
        }
        self.get_issue(repo, number, input.issue_fields.as_ref())
    }

    fn ensure_label(&self, repo: &str, name: &str) -> Result<()> {
        if self.label_exists(repo, name)? {
            return Ok(());
        }
        let payload = serde_json::json!({
            "name": name,
            "color": crate::github::status::IN_PROGRESS_LABEL_COLOR,
            "description": crate::github::status::IN_PROGRESS_LABEL_DESCRIPTION,
        })
        .to_string();
        let create_path = format!("/repos/{repo}/labels");
        match gh_api_with_input(
            &["api", &create_path, "-X", "POST", "--input", "-"],
            payload.as_bytes(),
        ) {
            Ok(_) => Ok(()),
            Err(error)
                if error.message.contains("already_exists")
                    || error.message.contains("already exists") =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn label_exists(&self, repo: &str, name: &str) -> Result<bool> {
        let encoded = name.replace(':', "%3A");
        let path = format!("/repos/{repo}/labels/{encoded}");
        match gh_api(&["api", &path]) {
            Ok(_) => Ok(true),
            Err(error)
                if error.message.contains("Not Found") || error.message.contains("HTTP 404") =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn update_relationships(
        &self,
        repo: &str,
        number: u64,
        desired: &[SupportedRelationship],
        removals: &[RelationshipChange],
    ) -> Result<()> {
        let desired_children: std::collections::BTreeSet<_> = desired
            .iter()
            .filter(|relationship| relationship.kind == "sub_issue")
            .map(|relationship| relationship.github.number)
            .collect();
        let desired_blockers: std::collections::BTreeSet<_> = desired
            .iter()
            .filter(|relationship| relationship.kind == "blocked_by")
            .map(|relationship| relationship.github.number)
            .collect();
        let remove_children: std::collections::BTreeSet<_> = removals
            .iter()
            .filter(|relationship| relationship.kind == "sub_issue")
            .map(|relationship| relationship.github.number)
            .collect();
        let remove_blockers: std::collections::BTreeSet<_> = removals
            .iter()
            .filter(|relationship| relationship.kind == "blocked_by")
            .map(|relationship| relationship.github.number)
            .collect();

        let children = list_issue_relationships(repo, number, "sub_issues")?;
        let current_children: std::collections::BTreeSet<_> = children
            .iter()
            .filter_map(|child| child.get("number").and_then(serde_json::Value::as_u64))
            .collect();
        for child_number in desired_children.difference(&current_children) {
            let child_id = rest_issue_numeric_id(repo, *child_number)?;
            let path = format!("/repos/{repo}/issues/{number}/sub_issues");
            let payload = serde_json::json!({ "sub_issue_id": child_id }).to_string();
            gh_api_with_input(
                &["api", &path, "-X", "POST", "--input", "-"],
                payload.as_bytes(),
            )?;
        }
        for child_number in remove_children.intersection(&current_children) {
            let child_id = rest_issue_numeric_id(repo, *child_number)?;
            let path = format!("/repos/{repo}/issues/{number}/sub_issue");
            let payload = serde_json::json!({ "sub_issue_id": child_id }).to_string();
            gh_api_with_input(
                &["api", &path, "-X", "DELETE", "--input", "-"],
                payload.as_bytes(),
            )?;
        }

        let blockers = list_issue_relationships(repo, number, "dependencies/blocked_by")?;
        let current_blockers: std::collections::BTreeSet<_> = blockers
            .iter()
            .filter_map(|blocker| blocker.get("number").and_then(serde_json::Value::as_u64))
            .collect();
        for blocker_number in desired_blockers.difference(&current_blockers) {
            let blocker_id = rest_issue_numeric_id(repo, *blocker_number)?;
            let path = format!("/repos/{repo}/issues/{number}/dependencies/blocked_by");
            let payload = serde_json::json!({ "issue_id": blocker_id }).to_string();
            gh_api_with_input(
                &["api", &path, "-X", "POST", "--input", "-"],
                payload.as_bytes(),
            )?;
        }
        for blocker_number in remove_blockers.intersection(&current_blockers) {
            let blocker_id = rest_issue_numeric_id(repo, *blocker_number)?;
            let path =
                format!("/repos/{repo}/issues/{number}/dependencies/blocked_by/{blocker_id}");
            gh_api(&["api", &path, "-X", "DELETE"])?;
        }
        Ok(())
    }
}

fn list_issue_relationships(
    repo: &str,
    number: u64,
    relationship: &str,
) -> Result<Vec<serde_json::Value>> {
    let mut results = Vec::new();
    let mut page = 1;
    loop {
        let path = format!("/repos/{repo}/issues/{number}/{relationship}?per_page=100&page={page}");
        let output = gh_api(&["api", &path])?;
        let page_values: Vec<serde_json::Value> =
            serde_json::from_slice(&output).map_err(|error| {
                OrbitError::new(
                    code::GITHUB_NOT_CONFIGURED,
                    format!("invalid GitHub issue relationship response: {error}"),
                )
            })?;
        let count = page_values.len();
        results.extend(page_values);
        if count < 100 {
            break;
        }
        page += 1;
    }
    Ok(results)
}

fn rest_issue_numeric_id(repo: &str, number: u64) -> Result<u64> {
    let path = format!("/repos/{repo}/issues/{number}");
    let output = gh_api(&["api", &path])?;
    let issue: serde_json::Value = serde_json::from_slice(&output).map_err(|error| {
        OrbitError::new(
            code::GITHUB_NOT_CONFIGURED,
            format!("invalid GitHub issue response: {error}"),
        )
    })?;
    issue
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            OrbitError::new(
                code::GITHUB_NOT_CONFIGURED,
                "GitHub issue response has no id",
            )
        })
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

fn update_rest_issue_field_values(
    repo: &str,
    number: u64,
    organization: &str,
    config: &IssueFieldConfig,
    area: Option<&Option<String>>,
    verification: Option<&Option<String>>,
) -> Result<()> {
    let definitions = list_rest_issue_fields(organization)?;
    let area_id = if area.is_some() {
        Some(rest_issue_field_id(&definitions, &config.area_name).ok_or_else(|| {
            OrbitError::new(
                code::GITHUB_SETUP_INCOMPLETE,
                format!(
                    "organization `{organization}` is missing Issue Field `{}`; run `orbit github setup`",
                    config.area_name
                ),
            )
        })?)
    } else {
        None
    };
    let verification_id = if verification.is_some() {
        Some(rest_issue_field_id(&definitions, &config.verification_name).ok_or_else(|| {
            OrbitError::new(
                code::GITHUB_SETUP_INCOMPLETE,
                format!(
                    "organization `{organization}` is missing Issue Field `{}`; run `orbit github setup`",
                    config.verification_name
                ),
            )
        })?)
    } else {
        None
    };
    let target_ids: Vec<_> = area_id.into_iter().chain(verification_id).collect();
    let mut values = list_rest_issue_field_values(repo, number)?;
    values.retain(|value| {
        value
            .get("field_id")
            .and_then(serde_json::Value::as_u64)
            .is_none_or(|field_id| !target_ids.contains(&field_id))
    });
    for (field_id, value) in [(area_id, area), (verification_id, verification)] {
        if let (Some(field_id), Some(Some(value))) = (field_id, value) {
            values.push(serde_json::json!({ "field_id": field_id, "value": value }));
        }
    }
    let path = format!("/repos/{repo}/issues/{number}/issue-field-values");
    let payload = serde_json::json!({ "issue_field_values": values }).to_string();
    gh_api_with_input(
        &["api", &path, "-X", "PUT", "--input", "-"],
        payload.as_bytes(),
    )?;
    Ok(())
}

fn list_rest_issue_field_values(repo: &str, number: u64) -> Result<Vec<serde_json::Value>> {
    let mut values = Vec::new();
    let mut page = 1;
    loop {
        let path =
            format!("/repos/{repo}/issues/{number}/issue-field-values?per_page=100&page={page}");
        let output = gh_api(&["api", &path])?;
        let fields: Vec<RestIssueFieldValue> =
            serde_json::from_slice(&output).map_err(|error| {
                OrbitError::new(
                    code::GITHUB_SETUP_INCOMPLETE,
                    format!("invalid GitHub Issue Fields response: {error}"),
                )
            })?;
        let count = fields.len();
        for field in fields {
            let value = field
                .single_select_option
                .and_then(|option| option.get("name").cloned())
                .or(field.value);
            if field.issue_field_id > 0
                && let Some(value) = value.filter(|value| !value.is_null())
            {
                values.push(serde_json::json!({
                    "field_id": field.issue_field_id,
                    "value": value,
                }));
            }
        }
        if count < 100 {
            break;
        }
        page += 1;
    }
    Ok(values)
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
    #[serde(default)]
    state_reason: Option<IssueStateReason>,
    labels: Vec<RestLabel>,
    assignees: Vec<RestUser>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct RestLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RestCatalogIssue {
    number: u64,
    title: String,
    state: String,
    #[serde(default)]
    state_reason: Option<IssueStateReason>,
    labels: Vec<RestLabel>,
    html_url: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
    #[serde(default)]
    area: Option<String>,
    #[serde(default)]
    verification: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RestIssueFieldValue {
    #[serde(default)]
    issue_field_id: u64,
    issue_field_name: String,
    #[serde(default)]
    value: Option<serde_json::Value>,
    #[serde(default)]
    single_select_option: Option<serde_json::Value>,
    #[serde(default)]
    multi_select_options: Option<Vec<serde_json::Value>>,
}

fn list_issue_field_values(repo: &str, number: u64) -> Result<BTreeMap<String, serde_json::Value>> {
    let mut fields = BTreeMap::new();
    let mut page = 1;
    loop {
        let path =
            format!("/repos/{repo}/issues/{number}/issue-field-values?per_page=100&page={page}");
        let output = gh_api(&["api", &path])?;
        let values: Vec<RestIssueFieldValue> =
            serde_json::from_slice(&output).map_err(|error| {
                OrbitError::new(
                    code::GITHUB_NOT_CONFIGURED,
                    format!("invalid GitHub Issue Fields response: {error}"),
                )
            })?;
        if values.is_empty() {
            break;
        }
        let count = values.len();
        for field in values {
            let value = field
                .single_select_option
                .and_then(|option| option.get("name").cloned())
                .or_else(|| {
                    field.multi_select_options.map(|options| {
                        serde_json::Value::Array(
                            options
                                .into_iter()
                                .filter_map(|option| option.get("name").cloned())
                                .collect(),
                        )
                    })
                })
                .or(field.value)
                .unwrap_or(serde_json::Value::Null);
            fields.insert(field.issue_field_name, value);
        }
        if count < 100 {
            break;
        }
        page += 1;
    }
    Ok(fields)
}

fn string_field(fields: &BTreeMap<String, serde_json::Value>, name: &str) -> Option<String> {
    fields
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.as_str().map(str::to_string))
}

#[derive(Debug, Deserialize)]
struct RestIssueComment {
    id: u64,
    body: String,
    user: RestUser,
    created_at: DateTime<Utc>,
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
        state_reason: issue.state_reason,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    created_fields: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    labels: HashMap<String, Vec<String>>,
    #[serde(default)]
    issues: HashMap<String, IssueSnapshot>,
    #[serde(default)]
    create_failures: HashMap<String, String>,
    #[serde(default)]
    update_failures: HashMap<String, String>,
    #[serde(default)]
    catalog_issues: HashMap<String, Vec<FixtureCatalogIssue>>,
    #[serde(default = "fixture_catalog_page_size")]
    catalog_page_size: usize,
    #[serde(default)]
    catalog_errors: HashMap<String, String>,
    #[serde(default)]
    issue_details: HashMap<String, serde_json::Value>,
    #[serde(default)]
    issue_comments: HashMap<String, Vec<IssueComment>>,
    #[serde(skip)]
    fixture_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FixtureCatalogIssue {
    number: u64,
    title: String,
    state: String,
    #[serde(default)]
    state_reason: Option<IssueStateReason>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    pull_request: bool,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default = "fixture_timestamp")]
    created_at: DateTime<Utc>,
    #[serde(default = "fixture_timestamp")]
    updated_at: DateTime<Utc>,
    #[serde(default)]
    area: Option<String>,
    #[serde(default)]
    verification: Option<String>,
    #[serde(default)]
    issue_fields: BTreeMap<String, serde_json::Value>,
}

fn fixture_timestamp() -> DateTime<Utc> {
    Utc::now()
}
fn fixture_catalog_page_size() -> usize {
    100
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

    fn get_issue(
        &self,
        repo: &str,
        number: u64,
        _issue_fields: Option<&IssueFieldConfig>,
    ) -> Result<IssueSnapshot> {
        let store = self.store.lock().expect("fixture lock");
        let key = format!("{repo}#{number}");
        if let Some(issue) = store.issues.get(&key).cloned() {
            return Ok(issue);
        }
        let summary = store
            .catalog_issues
            .get(repo)
            .and_then(|issues| issues.iter().find(|issue| issue.number == number))
            .ok_or_else(|| OrbitError::not_found(&key))?;
        let body = store
            .issue_details
            .get(&key)
            .and_then(|detail| detail.get("body"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok(IssueSnapshot {
            repo: repo.to_string(),
            number,
            title: summary.title.clone(),
            body,
            state: if summary.state == "open" {
                IssueState::Open
            } else {
                IssueState::Closed
            },
            state_reason: None,
            labels: summary.labels.clone(),
            assignees: Vec::new(),
            updated_at: summary.updated_at,
            area: summary.area.clone(),
            verification: summary.verification.clone(),
            parent_issue: Vec::new(),
            blocked_by: Vec::new(),
            blocking: Vec::new(),
        })
    }

    fn list_issues_page(
        &self,
        repo: &str,
        page: usize,
        issue_fields: Option<&IssueFieldConfig>,
    ) -> Result<Vec<IssueCatalogItem>> {
        let store = self.store.lock().expect("fixture lock");
        if let Some(message) = store.catalog_errors.get(repo) {
            return Err(OrbitError::new(code::GITHUB_NOT_CONFIGURED, message));
        }
        let issues = store.catalog_issues.get(repo).cloned().unwrap_or_default();
        let issues = issues
            .into_iter()
            .filter(|issue| !issue.pull_request)
            .collect::<Vec<_>>();
        let page_size = store.catalog_page_size.max(1);
        let start = page.saturating_sub(1) * page_size;
        Ok(issues
            .into_iter()
            .skip(start)
            .take(page_size)
            .map(|issue| IssueCatalogItem {
                identity: format!("{repo}#{}", issue.number),
                repo: repo.to_string(),
                number: issue.number,
                title: issue.title,
                state: if issue.state == "open" {
                    IssueState::Open
                } else {
                    IssueState::Closed
                },
                state_reason: issue.state_reason,
                labels: issue.labels,
                url: issue.html_url.or(issue.url).unwrap_or_else(|| {
                    format!("https://github.com/{repo}/issues/{}", issue.number)
                }),
                created_at: issue.created_at,
                updated_at: issue.updated_at,
                area: issue.area.or_else(|| {
                    issue_fields.and_then(|fields| {
                        string_field(&issue.issue_fields, &fields.area_name)
                    })
                }),
                verification: issue.verification.or_else(|| {
                    issue_fields.and_then(|fields| {
                        string_field(&issue.issue_fields, &fields.verification_name)
                    })
                }),
                issue_fields: issue.issue_fields,
            })
            .collect())
    }

    fn issue_comments_page(
        &self,
        repo: &str,
        number: u64,
        page: usize,
    ) -> Result<Vec<IssueComment>> {
        let store = self.store.lock().expect("fixture lock");
        let key = format!("{repo}#{number}");
        let comments = store.issue_comments.get(&key).cloned().unwrap_or_default();
        let start = page.saturating_sub(1) * 100;
        Ok(comments.into_iter().skip(start).take(100).collect())
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
            state_reason: None,
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
        if let Some(message) = store
            .update_failures
            .get(&key)
            .or_else(|| store.update_failures.get(repo))
        {
            return Err(OrbitError::new(
                code::GITHUB_NOT_CONFIGURED,
                message.clone(),
            ));
        }
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
            if state == IssueState::Open {
                issue.state_reason = None;
            }
        }
        if let Some(reason) = input.state_reason {
            issue.state_reason = Some(reason);
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

    fn ensure_label(&self, repo: &str, name: &str) -> Result<()> {
        let mut store = self.store.lock().expect("fixture lock");
        let labels = store.labels.entry(repo.to_string()).or_default();
        if labels.iter().any(|label| label == name) {
            return Ok(());
        }
        labels.push(name.to_string());
        persist_fixture(&store)
    }

    fn label_exists(&self, repo: &str, name: &str) -> Result<bool> {
        let store = self.store.lock().expect("fixture lock");
        Ok(store
            .labels
            .get(repo)
            .is_some_and(|labels| labels.iter().any(|label| label == name)))
    }

    fn update_relationships(
        &self,
        repo: &str,
        number: u64,
        desired: &[SupportedRelationship],
        removals: &[RelationshipChange],
    ) -> Result<()> {
        let mut store = self.store.lock().expect("fixture lock");
        let key = format!("{repo}#{number}");
        let mut issue = store
            .issues
            .get(&key)
            .cloned()
            .ok_or_else(|| OrbitError::not_found(&key))?;
        // Mirrors GhCliClient semantics: relationships are filtered by
        // kind and matched by issue number only (the planner emits
        // same-repo references). Desired entries are ensured present;
        // removals then drop the number when it is currently stored, so
        // a reference listed in both ends up absent.
        for relationship in desired {
            let current = match relationship.kind {
                "sub_issue" => &mut issue.blocking,
                "blocked_by" => &mut issue.blocked_by,
                _ => continue,
            };
            if !current
                .iter()
                .any(|item| item.number == relationship.github.number)
            {
                current.push(relationship.github.clone());
            }
        }
        for removal in removals {
            match removal.kind {
                "sub_issue" => issue
                    .blocking
                    .retain(|item| item.number != removal.github.number),
                "blocked_by" => issue
                    .blocked_by
                    .retain(|item| item.number != removal.github.number),
                _ => {}
            }
        }
        store.issues.insert(key, issue);
        persist_fixture(&store)
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

/// Parses the account name from `gh auth status` output. The adapter fills
/// `AuthStatus.username` so consumers never re-parse the message.
pub(crate) fn parse_auth_username(message: &str) -> Option<String> {
    message
        .lines()
        .find_map(|line| line.split_once("account ").map(|(_, account)| account))
        .and_then(|account| account.split_whitespace().next())
        .map(str::to_string)
}

/// Parses the granted scopes from `gh auth status` output.
pub(crate) fn parse_auth_scopes(message: &str) -> Vec<String> {
    message
        .lines()
        .find_map(|line| line.split_once("Token scopes:").map(|(_, scopes)| scopes))
        .map(|scopes| {
            scopes
                .split(',')
                .map(|scope| scope.trim().trim_matches('\'').trim_matches('"'))
                .filter(|scope| !scope.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::model::IssueRef;

    fn issue_ref(repo: &str, number: u64) -> IssueRef {
        IssueRef {
            repo: repo.to_string(),
            number,
        }
    }

    fn snapshot(repo: &str, number: u64) -> IssueSnapshot {
        IssueSnapshot {
            repo: repo.to_string(),
            number,
            title: "Issue".into(),
            body: String::new(),
            state: IssueState::Open,
            state_reason: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            updated_at: Utc::now(),
            area: None,
            verification: None,
            parent_issue: Vec::new(),
            blocked_by: Vec::new(),
            blocking: Vec::new(),
        }
    }

    fn desired(kind: &'static str, repo: &str, number: u64) -> SupportedRelationship {
        SupportedRelationship {
            kind,
            orbit_task: "orb-aaaaaa".into(),
            github: issue_ref(repo, number),
        }
    }

    fn removal(kind: &'static str, repo: &str, number: u64) -> RelationshipChange {
        RelationshipChange {
            action: "remove",
            kind,
            github: issue_ref(repo, number),
        }
    }

    fn fixture_client(issue: &IssueSnapshot) -> (tempfile::TempDir, FixtureClient) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fixture.json");
        let store = serde_json::json!({
            "issues": { format!("{}#{}", issue.repo, issue.number): issue }
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&store).unwrap()).unwrap();
        (dir, FixtureClient::load(&path).expect("load fixture"))
    }

    #[test]
    fn desired_relationships_are_matched_by_issue_number() {
        let mut issue = snapshot("acme/widget", 42);
        // The stored ref repo spelling differs; GhCliClient matches by
        // issue number only, so this must not duplicate the relationship.
        issue.blocking.push(issue_ref("ACME/Widget", 43));
        let (_dir, client) = fixture_client(&issue);

        client
            .update_relationships(
                "acme/widget",
                42,
                &[desired("sub_issue", "acme/widget", 43)],
                &[],
            )
            .unwrap();

        let updated = client.get_issue("acme/widget", 42, None).unwrap();
        assert_eq!(updated.blocking, vec![issue_ref("ACME/Widget", 43)]);
    }

    #[test]
    fn removals_drop_relationships_by_issue_number() {
        let mut issue = snapshot("acme/widget", 42);
        issue.blocked_by.push(issue_ref("ACME/Widget", 43));
        let (_dir, client) = fixture_client(&issue);

        client
            .update_relationships(
                "acme/widget",
                42,
                &[],
                &[removal("blocked_by", "acme/widget", 43)],
            )
            .unwrap();

        let updated = client.get_issue("acme/widget", 42, None).unwrap();
        assert!(updated.blocked_by.is_empty());
    }

    #[test]
    fn removals_are_noops_when_the_relationship_is_absent() {
        let mut issue = snapshot("acme/widget", 42);
        issue.blocking.push(issue_ref("acme/widget", 43));
        let (_dir, client) = fixture_client(&issue);

        client
            .update_relationships(
                "acme/widget",
                42,
                &[],
                &[removal("sub_issue", "acme/widget", 44)],
            )
            .unwrap();

        let updated = client.get_issue("acme/widget", 42, None).unwrap();
        assert_eq!(updated.blocking, vec![issue_ref("acme/widget", 43)]);
    }

    #[test]
    fn a_relationship_listed_as_desired_and_removed_ends_up_absent() {
        let issue = snapshot("acme/widget", 42);
        let (_dir, client) = fixture_client(&issue);

        client
            .update_relationships(
                "acme/widget",
                42,
                &[desired("sub_issue", "acme/widget", 43)],
                &[removal("sub_issue", "acme/widget", 43)],
            )
            .unwrap();

        let updated = client.get_issue("acme/widget", 42, None).unwrap();
        assert!(updated.blocking.is_empty());
    }

    #[test]
    fn removals_only_apply_to_their_own_relationship_kind() {
        let mut issue = snapshot("acme/widget", 42);
        issue.blocked_by.push(issue_ref("acme/widget", 43));
        let (_dir, client) = fixture_client(&issue);

        client
            .update_relationships(
                "acme/widget",
                42,
                &[],
                &[removal("sub_issue", "acme/widget", 43)],
            )
            .unwrap();

        let updated = client.get_issue("acme/widget", 42, None).unwrap();
        assert_eq!(updated.blocked_by, vec![issue_ref("acme/widget", 43)]);
    }

    #[test]
    fn applying_the_same_update_twice_is_idempotent() {
        let issue = snapshot("acme/widget", 42);
        let (_dir, client) = fixture_client(&issue);
        let desired = [desired("blocked_by", "acme/widget", 43)];

        client
            .update_relationships("acme/widget", 42, &desired, &[])
            .unwrap();
        client
            .update_relationships("acme/widget", 42, &desired, &[])
            .unwrap();

        let updated = client.get_issue("acme/widget", 42, None).unwrap();
        assert_eq!(updated.blocked_by, vec![issue_ref("acme/widget", 43)]);
    }
}
