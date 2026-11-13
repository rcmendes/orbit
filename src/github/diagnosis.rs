//! The shared Remote Tracker health diagnosis (ADR 0018).
//!
//! `orbit remote status` and `orbit doctor` used to run the same GitHub
//! health rules separately. Both now project one typed [`Diagnosis`]: each
//! rule exists exactly once here, and each command renders its own view —
//! the `remote status` JSON/human envelope or `doctor` findings. GitHub is
//! the only Remote Tracker adapter today, so the rules live next to the
//! `gh` client; a second tracker gets its own diagnosis behind the same
//! seam rather than a second port.

use serde_json::{Value, json};

use crate::board::Board;
use crate::error::{OrbitError, Result, code};
use crate::github::client::{GitHubClient, parse_auth_scopes, parse_auth_username};
use crate::github::config::GitHubConfig;
use crate::github::status::IN_PROGRESS_LABEL;
use crate::graph::Issue;

/// Which repositories the diagnosis probes and how strictly the connected
/// project set is resolved.
#[derive(Debug, Clone, Default)]
pub struct DiagnosisOptions {
    /// `remote status --repo` filter; `None` checks every repository.
    pub repository_scope: Option<String>,
    /// Whether a connected-projects resolution error aborts the diagnosis
    /// (`remote status`) or reads as an empty set (`doctor`, matching its
    /// historical `unwrap_or_default`).
    pub strict_connected_projects: bool,
}

/// The typed result both `remote status` and `doctor` project.
#[derive(Debug)]
pub struct Diagnosis {
    /// Whether the Board has any GitHub configuration at all.
    pub configured: bool,
    pub basic_mode: bool,
    pub auto_push: bool,
    pub auth: AuthDiagnosis,
    pub organization: Option<String>,
    /// The normalized `--repo` scope, echoed back by `remote status`.
    pub repository_scope: Option<String>,
    /// Every repository in the checked universe, in report order.
    pub repositories: Vec<RepositoryDiagnosis>,
    pub fields: FieldsDiagnosis,
    /// Health findings in `remote status` report order.
    pub findings: Vec<Finding>,
}

#[derive(Debug)]
pub struct AuthDiagnosis {
    pub authenticated: bool,
    pub message: String,
    pub username: Option<String>,
    pub scopes: Vec<String>,
    pub scopes_known: bool,
    pub required_scopes: Vec<&'static str>,
    pub missing_scopes: Vec<&'static str>,
}

#[derive(Debug)]
pub struct RepositoryDiagnosis {
    pub name: String,
    /// Whether `repository_info` ran for this repository (authenticated
    /// sessions only).
    pub access_checked: bool,
    /// `None` when the repository was not access-checked.
    pub accessible: Option<bool>,
    pub owner_type: Option<String>,
    pub access_error: Option<RemoteFailure>,
    /// The reserved-label probe; `None` when it never ran (unauthenticated).
    pub label: Option<LabelDiagnosis>,
}

#[derive(Debug)]
pub struct LabelDiagnosis {
    /// `None` when the label check itself failed.
    pub present: Option<bool>,
    pub error: Option<RemoteFailure>,
}

/// A tracker failure reduced to what projections need.
#[derive(Debug)]
pub struct RemoteFailure {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug)]
pub struct FieldsDiagnosis {
    pub area: FieldDiagnosis,
    pub verification: FieldDiagnosis,
    /// `validate_org_fields` itself failed (vs. ids merely mismatching).
    pub validation_failed: bool,
}

#[derive(Debug)]
pub struct FieldDiagnosis {
    pub configured: bool,
    pub resolved: Option<bool>,
    pub matches: Option<bool>,
}

/// One health finding before projection. `code` is the Orbit error code
/// both projections report.
#[derive(Debug)]
pub struct Finding {
    pub code: &'static str,
    pub kind: FindingKind,
    pub repository: Option<String>,
}

#[derive(Debug)]
pub enum FindingKind {
    NotConfigured,
    AuthRequired,
    ScopesMissing {
        missing: Vec<&'static str>,
    },
    RepositoryAccess {
        failure: RemoteFailure,
    },
    /// The reserved label is absent. `accessible` is the repository's
    /// access result so `remote status` can keep reporting it only for
    /// reachable repositories while `doctor` reports it regardless.
    ReservedLabelMissing {
        accessible: Option<bool>,
    },
    /// The label check failed; `doctor` reports it, `remote status` does
    /// not (its `reserved_label_present` simply reads `null`).
    LabelCheckFailed {
        failure: RemoteFailure,
    },
    NoRepositoriesLinked,
    OrganizationMissing,
    IssueFieldIdsStale,
    IssueFieldsInvalid {
        failure: RemoteFailure,
    },
    /// Auto-push is on but no repository exists at all; a `doctor`-only
    /// finding (`remote status` already reports `NoRepositoriesLinked`).
    AutoPushNoRepositories,
}

/// Run every shared health rule once and return the typed result.
///
/// The diagnosis never fetches linked Issues or plans syncs — that stays
/// with `remote status`'s per-task preview.
pub fn diagnose(
    board: &Board,
    client: &dyn GitHubClient,
    config: &GitHubConfig,
    options: &DiagnosisOptions,
) -> Result<Diagnosis> {
    let auth_status = client.auth_status();
    let username = auth_status
        .username
        .clone()
        .or_else(|| parse_auth_username(&auth_status.message));
    let scopes = if auth_status.scopes.is_empty() {
        parse_auth_scopes(&auth_status.message)
    } else {
        auth_status.scopes.clone()
    };
    let basic_mode = config.mode.is_basic();
    let (required_scopes, missing_scopes) = scope_status(basic_mode, &scopes);
    let configured = config.is_enabled();
    let auto_push = crate::remote::auto_push::enabled(board);

    let mut findings = Vec::new();
    if !configured {
        findings.push(Finding {
            code: code::GITHUB_NOT_CONFIGURED,
            kind: FindingKind::NotConfigured,
            repository: None,
        });
    }
    if !auth_status.authenticated {
        findings.push(Finding {
            code: "GITHUB_AUTH_REQUIRED",
            kind: FindingKind::AuthRequired,
            repository: None,
        });
    }
    if !missing_scopes.is_empty() {
        findings.push(Finding {
            code: "GITHUB_SCOPES_MISSING",
            kind: FindingKind::ScopesMissing {
                missing: missing_scopes.clone(),
            },
            repository: None,
        });
    }

    // The checked repository universe is the Board's connected GitHub
    // projects — configured Remote Projects plus member Repository origins —
    // the same set `remote status` and `doctor` both report on.
    let connected = if options.strict_connected_projects {
        crate::remote::connected_github_projects(board)?
    } else {
        crate::remote::connected_github_projects(board).unwrap_or_default()
    };
    let scope = options.repository_scope.as_deref();
    let names: Vec<String> = connected
        .into_iter()
        .filter(|name| scope.is_none_or(|scope| name.eq_ignore_ascii_case(scope)))
        .collect();

    let mut repositories = Vec::with_capacity(names.len());
    for name in names {
        let mut repository = RepositoryDiagnosis {
            name: name.clone(),
            access_checked: auth_status.authenticated,
            accessible: None,
            owner_type: None,
            access_error: None,
            label: None,
        };
        if auth_status.authenticated {
            match client.repository_info(&name) {
                Ok(info) => {
                    repository.accessible = Some(true);
                    repository.owner_type = Some(info.owner_type);
                }
                Err(error) => {
                    repository.accessible = Some(false);
                    repository.access_error = Some(RemoteFailure {
                        code: error.code,
                        message: error.message.clone(),
                    });
                    findings.push(Finding {
                        code: error.code,
                        kind: FindingKind::RepositoryAccess {
                            failure: RemoteFailure {
                                code: error.code,
                                message: error.message,
                            },
                        },
                        repository: Some(name.clone()),
                    });
                }
            }
            let probe = client.label_exists(&name, IN_PROGRESS_LABEL);
            repository.label = Some(LabelDiagnosis {
                present: probe.as_ref().ok().copied(),
                error: probe.err().map(|error| RemoteFailure {
                    code: error.code,
                    message: error.message,
                }),
            });
            if auto_push && let Some(label) = &repository.label {
                if label.present == Some(false) {
                    findings.push(Finding {
                        code: code::GITHUB_SETUP_INCOMPLETE,
                        kind: FindingKind::ReservedLabelMissing {
                            accessible: repository.accessible,
                        },
                        repository: Some(name.clone()),
                    });
                } else if let Some(error) = &label.error {
                    findings.push(Finding {
                        code: error.code,
                        kind: FindingKind::LabelCheckFailed {
                            failure: RemoteFailure {
                                code: error.code,
                                message: error.message.clone(),
                            },
                        },
                        repository: Some(name.clone()),
                    });
                }
            }
        }
        repositories.push(repository);
    }

    if configured && repositories.is_empty() {
        findings.push(Finding {
            code: code::GITHUB_REPO_NOT_LINKED,
            kind: FindingKind::NoRepositoriesLinked,
            repository: None,
        });
    }
    if auto_push && repositories.is_empty() {
        findings.push(Finding {
            code: code::GITHUB_REPO_NOT_LINKED,
            kind: FindingKind::AutoPushNoRepositories,
            repository: None,
        });
    }

    let area_id_configured = !basic_mode && config.issue_fields.area_id.is_some();
    let verification_id_configured = !basic_mode && config.issue_fields.verification_id.is_some();
    let mut fields = FieldsDiagnosis {
        area: FieldDiagnosis {
            configured: area_id_configured,
            resolved: None,
            matches: None,
        },
        verification: FieldDiagnosis {
            configured: verification_id_configured,
            resolved: None,
            matches: None,
        },
        validation_failed: false,
    };
    if !basic_mode {
        if let Some(org) = config.organization.as_deref() {
            if auth_status.authenticated {
                match client.validate_org_fields(org, &config.issue_fields) {
                    Ok(setup) => {
                        fields.area.resolved = Some(true);
                        fields.verification.resolved = Some(true);
                        fields.area.matches = Some(
                            config.issue_fields.area_id.as_deref() == Some(setup.area_id.as_str()),
                        );
                        fields.verification.matches = Some(
                            config.issue_fields.verification_id.as_deref()
                                == Some(setup.verification_id.as_str()),
                        );
                        if fields.area.matches == Some(false)
                            || fields.verification.matches == Some(false)
                        {
                            findings.push(Finding {
                                code: code::GITHUB_SETUP_INCOMPLETE,
                                kind: FindingKind::IssueFieldIdsStale,
                                repository: None,
                            });
                        }
                    }
                    Err(error) => {
                        fields.validation_failed = true;
                        let missing = missing_field_names(&error);
                        if !missing.is_empty() {
                            fields.area.resolved =
                                Some(!missing.contains(&config.issue_fields.area_name.as_str()));
                            fields.verification.resolved = Some(
                                !missing.contains(&config.issue_fields.verification_name.as_str()),
                            );
                        }
                        findings.push(Finding {
                            code: error.code,
                            kind: FindingKind::IssueFieldsInvalid {
                                failure: RemoteFailure {
                                    code: error.code,
                                    message: error.message,
                                },
                            },
                            repository: None,
                        });
                    }
                }
            }
        } else if configured {
            findings.push(Finding {
                code: code::GITHUB_SETUP_INCOMPLETE,
                kind: FindingKind::OrganizationMissing,
                repository: None,
            });
        }
    }

    Ok(Diagnosis {
        configured,
        basic_mode,
        auto_push,
        auth: AuthDiagnosis {
            authenticated: auth_status.authenticated,
            message: auth_status.message,
            username,
            scopes_known: !scopes.is_empty(),
            scopes,
            required_scopes: required_scopes.to_vec(),
            missing_scopes,
        },
        organization: config.organization.clone(),
        repository_scope: options.repository_scope.clone(),
        repositories,
        fields,
        findings,
    })
}

/// The Issue Field names reported missing by `validate_org_fields`, read from
/// the error's structured details rather than its message text.
pub(crate) fn missing_field_names(error: &OrbitError) -> Vec<&str> {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("fields"))
        .and_then(|fields| fields.as_array())
        .map(|fields| fields.iter().filter_map(|field| field.as_str()).collect())
        .unwrap_or_default()
}

fn scope_status(
    basic_mode: bool,
    scopes: &[String],
) -> (&'static [&'static str], Vec<&'static str>) {
    const BASIC_REQUIRED_SCOPES: &[&str] = &["repo"];
    const FULL_REQUIRED_SCOPES: &[&str] = &["read:org", "repo"];

    let required_scopes = if basic_mode {
        BASIC_REQUIRED_SCOPES
    } else {
        FULL_REQUIRED_SCOPES
    };
    let missing_scopes = if scopes.is_empty() {
        Vec::new()
    } else {
        required_scopes
            .iter()
            .filter(|required| !scopes.iter().any(|scope| scope_satisfies(scope, required)))
            .copied()
            .collect()
    };

    (required_scopes, missing_scopes)
}

fn scope_satisfies(granted: &str, required: &str) -> bool {
    granted == required || (required == "read:org" && matches!(granted, "admin:org" | "write:org"))
}

/// One `remote status` `issues[]` entry: the typed form of the JSON object
/// the command reports.
#[derive(Debug, Clone)]
pub struct StatusIssue {
    pub code: &'static str,
    pub message: String,
    pub repository: Option<String>,
    pub task: Option<String>,
}

impl StatusIssue {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            repository: None,
            task: None,
        }
    }

    pub fn with_repository(mut self, repository: impl Into<String>) -> Self {
        self.repository = Some(repository.into());
        self
    }

    pub fn with_task(mut self, task: impl Into<String>) -> Self {
        self.task = Some(task.into());
        self
    }

    /// The `issues[]` JSON object; absent keys stay absent, matching the
    /// historical envelope.
    pub fn to_json(&self) -> Value {
        let mut issue = json!({
            "code": self.code,
            "message": self.message,
        });
        if let Some(task) = &self.task {
            issue["task"] = json!(task);
        }
        if let Some(repository) = &self.repository {
            issue["repository"] = json!(repository);
        }
        issue
    }
}

impl Diagnosis {
    /// The `issues` entries of `orbit remote status --json`, in report
    /// order. Findings `remote status` never surfaced (label probe errors,
    /// the Auto-push empty-repository variant, missing-label reports on
    /// inaccessible repositories) are filtered out here.
    pub fn status_issues(&self) -> Vec<StatusIssue> {
        let mut issues = Vec::new();
        for finding in &self.findings {
            let issue = match &finding.kind {
                FindingKind::NotConfigured => {
                    StatusIssue::new(finding.code, "GitHub is not configured for this Board")
                }
                FindingKind::AuthRequired => {
                    StatusIssue::new(finding.code, self.auth.message.clone())
                }
                FindingKind::ScopesMissing { missing } => StatusIssue::new(
                    finding.code,
                    format!(
                        "GitHub token is missing required scopes: {}",
                        missing.join(", ")
                    ),
                ),
                FindingKind::RepositoryAccess { failure } => {
                    StatusIssue::new(failure.code, failure.message.clone())
                        .with_repository(finding.repository.as_deref().unwrap_or_default())
                }
                FindingKind::ReservedLabelMissing { accessible } => {
                    if *accessible != Some(true) {
                        continue;
                    }
                    StatusIssue::new(
                        finding.code,
                        format!(
                            "the `{IN_PROGRESS_LABEL}` label is missing on `{}`; run `orbit remote connect` or create it",
                            finding.repository.as_deref().unwrap_or_default()
                        ),
                    )
                    .with_repository(finding.repository.as_deref().unwrap_or_default())
                }
                FindingKind::LabelCheckFailed { .. } | FindingKind::AutoPushNoRepositories => {
                    continue;
                }
                FindingKind::NoRepositoriesLinked => StatusIssue::new(
                    finding.code,
                    match self.repository_scope.as_deref() {
                        Some(repo) => {
                            format!("GitHub repository `{repo}` is not linked to this Board")
                        }
                        None => "No GitHub repositories are linked to this Board".to_string(),
                    },
                ),
                FindingKind::OrganizationMissing => StatusIssue::new(
                    finding.code,
                    "github.organization is not set; run `orbit remote connect`",
                ),
                FindingKind::IssueFieldIdsStale => StatusIssue::new(
                    finding.code,
                    "stored GitHub Issue Field ids do not match the organization fields; run `orbit remote connect --fields`",
                ),
                FindingKind::IssueFieldsInvalid { failure } => {
                    StatusIssue::new(failure.code, failure.message.clone())
                }
            };
            issues.push(issue);
        }
        issues
    }

    /// The `issues` `orbit doctor` reports, in doctor order. `doctor`
    /// returns early on missing authentication, so an unauthenticated
    /// diagnosis projects to the single auth finding.
    pub fn doctor_issues(&self) -> Vec<Issue> {
        if !self.auth.authenticated {
            return self
                .findings
                .iter()
                .find(|finding| matches!(finding.kind, FindingKind::AuthRequired))
                .map(|finding| vec![doctor_issue(finding, self)])
                .unwrap_or_default();
        }
        let mut findings: Vec<&Finding> = self
            .findings
            .iter()
            .filter(|finding| !matches!(finding.kind, FindingKind::NotConfigured))
            .collect();
        // Stable sort: findings already arrive in repository order inside
        // each rank, preserving the historical per-repository sequence.
        findings.sort_by_key(|finding| doctor_rank(finding));
        findings
            .into_iter()
            .map(|finding| doctor_issue(finding, self))
            .collect()
    }
}

fn doctor_rank(finding: &Finding) -> u8 {
    match finding.kind {
        FindingKind::AuthRequired => 0,
        FindingKind::ScopesMissing { .. } => 1,
        FindingKind::OrganizationMissing => 2,
        FindingKind::NoRepositoriesLinked => 3,
        FindingKind::RepositoryAccess { .. } => 4,
        FindingKind::AutoPushNoRepositories => 5,
        FindingKind::ReservedLabelMissing { .. } | FindingKind::LabelCheckFailed { .. } => 6,
        FindingKind::IssueFieldIdsStale | FindingKind::IssueFieldsInvalid { .. } => 7,
        FindingKind::NotConfigured => 8,
    }
}

fn doctor_issue(finding: &Finding, diagnosis: &Diagnosis) -> Issue {
    let repository = finding.repository.as_deref().unwrap_or_default();
    let (code, message) = match &finding.kind {
        FindingKind::AuthRequired => (
            finding.code,
            format!(
                "GitHub authentication is unavailable; run `gh auth login` ({})",
                diagnosis.auth.message
            ),
        ),
        FindingKind::ScopesMissing { missing } => (
            finding.code,
            format!("GitHub token is missing scopes: {}", missing.join(", ")),
        ),
        FindingKind::OrganizationMissing => (
            finding.code,
            "github.organization is not set; run `orbit remote connect`".to_string(),
        ),
        FindingKind::NoRepositoriesLinked => (
            finding.code,
            "no GitHub repositories are linked; run `orbit remote connect OWNER/REPO`".to_string(),
        ),
        FindingKind::RepositoryAccess { failure } => (
            failure.code,
            format!("GitHub repository `{repository}` is unavailable: {}", failure.message),
        ),
        FindingKind::AutoPushNoRepositories => (
            finding.code,
            "auto-push is enabled but no GitHub repository is linked or included; new Tasks stay local"
                .to_string(),
        ),
        FindingKind::ReservedLabelMissing { .. } => (
            finding.code,
            format!(
                "the `{IN_PROGRESS_LABEL}` label is missing on `{repository}`; run `orbit remote connect` or create it"
            ),
        ),
        FindingKind::LabelCheckFailed { failure } => (
            failure.code,
            format!(
                "could not check the `{IN_PROGRESS_LABEL}` label on `{repository}`: {}",
                failure.message
            ),
        ),
        FindingKind::IssueFieldIdsStale => (
            finding.code,
            "GitHub Issue Field ids are missing or stale; run `orbit remote connect --fields`"
                .to_string(),
        ),
        FindingKind::IssueFieldsInvalid { failure } => (
            failure.code,
            format!(
                "GitHub Issue Fields are not ready: {}; run `orbit remote connect --fields`",
                failure.message
            ),
        ),
        FindingKind::NotConfigured => (
            finding.code,
            "GitHub is not configured for this Board".to_string(),
        ),
    };
    Issue::new(code, None, message)
}

/// Everything the `remote status` `next_action`/`next_command` cascade
/// reads, assembled by the command from the diagnosis plus its own
/// per-task sync preview.
#[derive(Debug)]
pub struct NextActionInput<'a> {
    pub authenticated: bool,
    pub missing_scopes: &'a [&'static str],
    pub configured: bool,
    pub basic_mode: bool,
    pub organization_configured: bool,
    /// Whether `[github] repositories` is non-empty (members do not count).
    pub repositories_configured: bool,
    pub field_validation_failed: bool,
    pub fields: &'a FieldsDiagnosis,
    pub area_id_configured: bool,
    pub verification_id_configured: bool,
    pub unsupported_relationships: usize,
    pub has_conflicts: bool,
    pub first_conflict_task: Option<&'a str>,
    pub pending_changes: bool,
    /// Any reported issue carries `GITHUB_REPO_NOT_LINKED`.
    pub repo_not_linked: bool,
    /// The `repository` of the *first* `GITHUB_REPO_NOT_LINKED` issue;
    /// `None` falls back to `OWNER/REPO`, matching the historical cascade.
    pub repo_not_linked_repository: Option<&'a str>,
    /// The `repository` of the first issue carrying one; drives the
    /// repository-access hint.
    pub repository_issue: Option<&'a str>,
}

/// The typed `next_action` policy: one cascade, rendered as the historical
/// action/command strings by [`NextAction::action`] and
/// [`NextAction::command`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextAction {
    Authenticate,
    RefreshScopes { basic: bool },
    Connect,
    ConnectProject { repository: Option<String> },
    ConnectFields,
    ReviewUnsupportedRelationships,
    ResolveSyncConflicts { task: Option<String> },
    Sync,
    CheckRepositoryAccess { repository: Option<String> },
    None,
}

pub fn next_action(input: &NextActionInput) -> NextAction {
    if !input.authenticated {
        return NextAction::Authenticate;
    }
    if !input.missing_scopes.is_empty() {
        return NextAction::RefreshScopes {
            basic: input.basic_mode,
        };
    }
    if !input.configured || (!input.basic_mode && !input.organization_configured) {
        return NextAction::Connect;
    }
    if !input.repositories_configured {
        return NextAction::ConnectProject { repository: None };
    }
    if input.field_validation_failed
        && input.fields.area.resolved.is_none()
        && input.fields.verification.resolved.is_none()
    {
        return NextAction::RefreshScopes {
            basic: input.basic_mode,
        };
    }
    if input.fields.area.resolved == Some(false)
        || input.fields.verification.resolved == Some(false)
        || input.fields.area.matches == Some(false)
        || input.fields.verification.matches == Some(false)
        || (!input.basic_mode && (!input.area_id_configured || !input.verification_id_configured))
    {
        return NextAction::ConnectFields;
    }
    if input.unsupported_relationships > 0 {
        return NextAction::ReviewUnsupportedRelationships;
    }
    if input.has_conflicts {
        return NextAction::ResolveSyncConflicts {
            task: input.first_conflict_task.map(str::to_string),
        };
    }
    if input.pending_changes {
        return NextAction::Sync;
    }
    if input.repo_not_linked {
        return NextAction::ConnectProject {
            repository: input.repo_not_linked_repository.map(str::to_string),
        };
    }
    if let Some(repository) = input.repository_issue {
        return NextAction::CheckRepositoryAccess {
            repository: Some(repository.to_string()),
        };
    }
    NextAction::None
}

impl NextAction {
    /// The `next_action` string in the `remote status` JSON envelope.
    pub fn action(&self) -> String {
        match self {
            NextAction::Authenticate => "gh auth login".to_string(),
            NextAction::RefreshScopes { basic } => format!(
                "gh auth refresh --hostname github.com --scopes {}",
                if *basic { "repo" } else { "read:org,repo" }
            ),
            NextAction::Connect => "orbit remote connect".to_string(),
            NextAction::ConnectProject { .. } => "orbit remote connect OWNER/REPO".to_string(),
            NextAction::ConnectFields => "orbit remote connect --fields".to_string(),
            NextAction::ReviewUnsupportedRelationships => {
                "review unsupported GitHub relationships".to_string()
            }
            NextAction::ResolveSyncConflicts { .. } => "resolve_github_sync_conflicts".to_string(),
            NextAction::Sync => "orbit remote sync".to_string(),
            NextAction::CheckRepositoryAccess { .. } => {
                "check GitHub repository access".to_string()
            }
            NextAction::None => "none".to_string(),
        }
    }

    /// The `next_command` string, or `None` when the action has no
    /// copy-pasteable command (the human output falls back to `next_action`).
    pub fn command(&self) -> Option<String> {
        match self {
            NextAction::Authenticate => Some("gh auth login".to_string()),
            NextAction::RefreshScopes { .. } => Some(self.action()),
            NextAction::Connect => Some("orbit remote connect".to_string()),
            NextAction::ConnectProject { repository } => Some(format!(
                "orbit remote connect {}",
                repository.as_deref().unwrap_or("OWNER/REPO")
            )),
            NextAction::ConnectFields => Some("orbit remote connect --fields".to_string()),
            NextAction::Sync => Some("orbit remote sync".to_string()),
            NextAction::ResolveSyncConflicts { task } => task
                .as_ref()
                .map(|task| format!("orbit remote sync {task}")),
            NextAction::CheckRepositoryAccess { repository } => repository
                .as_ref()
                .map(|repo| format!("gh repo view {repo}")),
            NextAction::ReviewUnsupportedRelationships | NextAction::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Config;
    use crate::github::adapter::GitHubAdapter;
    use crate::github::client::FixtureClient;
    use crate::github::config::GitHubMode;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// A Board rooted at an empty tempdir: member resolution finds no
    /// repositories, which is all the diagnosis needs from it.
    fn board_with(config: Config) -> Board {
        let dir = Box::leak(Box::new(tempfile::tempdir().expect("tempdir")));
        Board {
            root: dir.path().to_path_buf(),
            lock_path: PathBuf::from("/nonexistent/.lock"),
            config,
            warnings: Vec::new(),
            orbit_dir: None,
            needs_migration: false,
            commit_failed: std::cell::Cell::new(false),
        }
    }

    fn github_config() -> GitHubConfig {
        GitHubConfig {
            organization: Some("acme".to_string()),
            repositories: vec!["acme/widget".to_string()],
            ..GitHubConfig::default()
        }
    }

    fn configured_board() -> Board {
        let mut config = github_config();
        config.issue_fields.area_id = Some("AREA_ID".to_string());
        config.issue_fields.verification_id = Some("VERIFICATION_ID".to_string());
        let mut board_config = Config::default();
        board_config.github = config;
        board_with(board_config)
    }

    fn auto_push_board() -> Board {
        let mut config = github_config();
        config.issue_fields.area_id = Some("AREA_ID".to_string());
        config.issue_fields.verification_id = Some("VERIFICATION_ID".to_string());
        let mut board_config = Config::default();
        board_config.github = config;
        board_config.remote.auto_push = true;
        board_with(board_config)
    }

    fn fixture_client(dir: &tempfile::TempDir, store: serde_json::Value) -> Arc<dyn GitHubClient> {
        let path = dir.path().join("fixture.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&store).unwrap()).unwrap();
        Arc::new(FixtureClient::load(&path).unwrap())
    }

    fn healthy_store() -> serde_json::Value {
        serde_json::json!({
            "authenticated": true,
            "auth_username": "agent",
            "auth_scopes": ["read:org", "repo"],
            "repositories": {
                "acme/widget": { "owner_type": "Organization", "accessible": true }
            },
            "org_fields": {
                "acme/Area": "AREA_ID",
                "acme/Verification": "VERIFICATION_ID"
            },
            "labels": { "acme/widget": ["orbit:in-progress"] }
        })
    }

    fn diagnose_fixture(
        board: &Board,
        client: &Arc<dyn GitHubClient>,
        options: &DiagnosisOptions,
    ) -> Diagnosis {
        diagnose(
            board,
            client.as_ref(),
            &GitHubAdapter::effective_config(board),
            options,
        )
        .unwrap()
    }

    #[test]
    fn diagnosis_reports_a_healthy_full_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let client = fixture_client(&dir, healthy_store());
        let board = configured_board();
        let diagnosis = diagnose_fixture(&board, &client, &DiagnosisOptions::default());

        assert!(diagnosis.configured);
        assert!(!diagnosis.basic_mode);
        assert!(
            diagnosis.findings.is_empty(),
            "{:?}",
            diagnosis
                .findings
                .iter()
                .map(|finding| &finding.kind)
                .collect::<Vec<_>>()
        );
        assert_eq!(diagnosis.repositories.len(), 1);
        assert_eq!(diagnosis.repositories[0].accessible, Some(true));
        assert_eq!(diagnosis.fields.area.resolved, Some(true));
        assert_eq!(diagnosis.fields.area.matches, Some(true));
        assert!(diagnosis.status_issues().is_empty());
        assert!(diagnosis.doctor_issues().is_empty());
    }

    #[test]
    fn diagnosis_requires_authentication_before_anything_else() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = healthy_store();
        store["authenticated"] = serde_json::json!(false);
        store["auth_message"] = serde_json::json!("not logged in");
        let client = fixture_client(&dir, store);
        let board = configured_board();
        let diagnosis = diagnose_fixture(&board, &client, &DiagnosisOptions::default());

        assert!(!diagnosis.auth.authenticated);
        // Status still reports the unauthenticated state; doctor collapses
        // to the single auth finding.
        assert!(
            diagnosis
                .status_issues()
                .iter()
                .any(|issue| issue.code == "GITHUB_AUTH_REQUIRED")
        );
        let doctor = diagnosis.doctor_issues();
        assert_eq!(doctor.len(), 1);
        assert_eq!(doctor[0].code, "GITHUB_AUTH_REQUIRED");
        assert!(doctor[0].message.contains("gh auth login"));
    }

    #[test]
    fn diagnosis_reports_missing_scopes_in_each_mode() {
        for (basic, scopes, expected) in [
            (false, serde_json::json!(["repo"]), "read:org"),
            (true, serde_json::json!(["read:org"]), "repo"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = healthy_store();
            store["auth_scopes"] = scopes;
            let client = fixture_client(&dir, store);
            let mut config = github_config();
            if basic {
                config.mode = GitHubMode::Basic;
            }
            let mut board_config = Config::default();
            board_config.github = config;
            let board = board_with(board_config);
            let diagnosis = diagnose_fixture(&board, &client, &DiagnosisOptions::default());

            assert_eq!(diagnosis.auth.missing_scopes, vec![expected]);
            assert!(
                diagnosis
                    .findings
                    .iter()
                    .any(|finding| matches!(finding.kind, FindingKind::ScopesMissing { .. }))
            );
        }
    }

    #[test]
    fn diagnosis_hides_label_findings_on_inaccessible_repositories_for_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = healthy_store();
        store["labels"] = serde_json::json!({});
        store["repositories"]["acme/widget"]["accessible"] = serde_json::json!(false);
        let client = fixture_client(&dir, store);
        let board = auto_push_board();
        let diagnosis = diagnose_fixture(
            &board,
            &client,
            &DiagnosisOptions {
                strict_connected_projects: true,
                ..DiagnosisOptions::default()
            },
        );

        let repository = &diagnosis.repositories[0];
        assert_eq!(repository.accessible, Some(false));
        // The status projection hides the missing-label finding on an
        // inaccessible repository; doctor still reports the access error.
        assert!(
            diagnosis
                .status_issues()
                .iter()
                .all(|issue| issue.code != code::GITHUB_SETUP_INCOMPLETE)
        );
        assert!(
            diagnosis
                .status_issues()
                .iter()
                .any(|issue| issue.code == code::GITHUB_NOT_CONFIGURED)
        );
        assert!(
            diagnosis
                .doctor_issues()
                .iter()
                .any(|issue| issue.code == code::GITHUB_NOT_CONFIGURED)
        );
        // Doctor also keeps the missing-label finding for the same repo.
        assert!(
            diagnosis
                .doctor_issues()
                .iter()
                .any(|issue| issue.message.contains("orbit:in-progress"))
        );
    }

    #[test]
    fn diagnosis_reports_auto_push_label_findings() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = healthy_store();
        store["labels"] = serde_json::json!({});
        let client = fixture_client(&dir, store);
        let board = auto_push_board();
        let diagnosis = diagnose_fixture(&board, &client, &DiagnosisOptions::default());

        assert!(diagnosis.auto_push);
        assert!(
            diagnosis
                .status_issues()
                .iter()
                .any(|issue| issue.message.contains("orbit:in-progress"))
        );
        assert!(
            diagnosis
                .doctor_issues()
                .iter()
                .any(|issue| issue.message.contains("orbit:in-progress"))
        );
    }

    #[test]
    fn diagnosis_reports_stale_issue_field_ids() {
        let dir = tempfile::tempdir().unwrap();
        let client = fixture_client(&dir, healthy_store());
        let mut config = github_config();
        config.issue_fields.area_id = Some("STALE_AREA".to_string());
        config.issue_fields.verification_id = Some("VERIFICATION_ID".to_string());
        let mut board_config = Config::default();
        board_config.github = config;
        let board = board_with(board_config);
        let diagnosis = diagnose_fixture(&board, &client, &DiagnosisOptions::default());

        assert_eq!(diagnosis.fields.area.matches, Some(false));
        assert!(
            diagnosis
                .findings
                .iter()
                .any(|finding| matches!(finding.kind, FindingKind::IssueFieldIdsStale))
        );
    }

    #[test]
    fn diagnosis_reports_missing_issue_fields_with_resolved_flags() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = healthy_store();
        store["org_fields"] = serde_json::json!({ "acme/Area": "AREA_ID" });
        let client = fixture_client(&dir, store);
        let board = configured_board();
        let diagnosis = diagnose_fixture(&board, &client, &DiagnosisOptions::default());

        assert!(diagnosis.fields.validation_failed);
        assert_eq!(diagnosis.fields.area.resolved, Some(true));
        assert_eq!(diagnosis.fields.verification.resolved, Some(false));
        assert!(
            diagnosis
                .findings
                .iter()
                .any(|finding| matches!(finding.kind, FindingKind::IssueFieldsInvalid { .. }))
        );
    }

    #[test]
    fn diagnosis_repository_scope_filters_the_checked_universe() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = healthy_store();
        store["repositories"]["acme/other"] =
            serde_json::json!({ "owner_type": "Organization", "accessible": true });
        let client = fixture_client(&dir, store);
        let mut config = github_config();
        config.repositories.push("acme/other".to_string());
        let mut board_config = Config::default();
        board_config.github = config;
        let board = board_with(board_config);

        let scoped = diagnose_fixture(
            &board,
            &client,
            &DiagnosisOptions {
                repository_scope: Some("acme/other".to_string()),
                ..DiagnosisOptions::default()
            },
        );
        assert_eq!(scoped.repositories.len(), 1);
        assert_eq!(scoped.repositories[0].name, "acme/other");
        assert_eq!(scoped.repository_scope.as_deref(), Some("acme/other"));

        let unscoped = diagnose_fixture(&board, &client, &DiagnosisOptions::default());
        assert_eq!(unscoped.repositories.len(), 2);
    }

    fn resolved_fields() -> FieldsDiagnosis {
        FieldsDiagnosis {
            area: FieldDiagnosis {
                configured: true,
                resolved: Some(true),
                matches: Some(true),
            },
            verification: FieldDiagnosis {
                configured: true,
                resolved: Some(true),
                matches: Some(true),
            },
            validation_failed: false,
        }
    }

    fn next_input<'a>(fields: &'a FieldsDiagnosis) -> NextActionInput<'a> {
        NextActionInput {
            authenticated: true,
            missing_scopes: &[],
            configured: true,
            basic_mode: false,
            organization_configured: true,
            repositories_configured: true,
            field_validation_failed: false,
            fields,
            area_id_configured: true,
            verification_id_configured: true,
            unsupported_relationships: 0,
            has_conflicts: false,
            first_conflict_task: None,
            pending_changes: false,
            repo_not_linked: false,
            repo_not_linked_repository: None,
            repository_issue: None,
        }
    }

    #[test]
    fn next_action_cascade_prefers_auth_then_scopes_then_setup() {
        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.authenticated = false;
        assert_eq!(next_action(&input), NextAction::Authenticate);
        assert_eq!(next_action(&input).action(), "gh auth login");
        assert_eq!(
            next_action(&input).command().as_deref(),
            Some("gh auth login")
        );

        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.missing_scopes = &["read:org"];
        assert_eq!(
            next_action(&input),
            NextAction::RefreshScopes { basic: false }
        );
        assert_eq!(
            next_action(&input).command().as_deref(),
            Some("gh auth refresh --hostname github.com --scopes read:org,repo")
        );

        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.basic_mode = true;
        input.missing_scopes = &["repo"];
        assert_eq!(
            next_action(&input).action(),
            "gh auth refresh --hostname github.com --scopes repo"
        );

        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.configured = false;
        assert_eq!(next_action(&input), NextAction::Connect);

        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.organization_configured = false;
        assert_eq!(next_action(&input), NextAction::Connect);

        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.repositories_configured = false;
        assert_eq!(
            next_action(&input),
            NextAction::ConnectProject { repository: None }
        );
        assert_eq!(
            next_action(&input).command().as_deref(),
            Some("orbit remote connect OWNER/REPO")
        );
    }

    #[test]
    fn next_action_prefers_fields_then_relationships_then_sync() {
        let stale_fields = FieldsDiagnosis {
            area: FieldDiagnosis {
                configured: true,
                resolved: Some(true),
                matches: Some(false),
            },
            verification: FieldDiagnosis {
                configured: true,
                resolved: Some(true),
                matches: Some(true),
            },
            validation_failed: false,
        };
        let input = next_input(&stale_fields);
        assert_eq!(next_action(&input), NextAction::ConnectFields);
        assert_eq!(
            next_action(&input).command().as_deref(),
            Some("orbit remote connect --fields")
        );

        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.unsupported_relationships = 2;
        input.has_conflicts = true;
        input.first_conflict_task = Some("orbit-1a2b3c");
        input.pending_changes = true;
        assert_eq!(
            next_action(&input),
            NextAction::ReviewUnsupportedRelationships
        );
        assert_eq!(next_action(&input).command(), None);

        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.has_conflicts = true;
        input.first_conflict_task = Some("orbit-1a2b3c");
        input.pending_changes = true;
        assert_eq!(
            next_action(&input),
            NextAction::ResolveSyncConflicts {
                task: Some("orbit-1a2b3c".to_string())
            }
        );
        assert_eq!(
            next_action(&input).command().as_deref(),
            Some("orbit remote sync orbit-1a2b3c")
        );

        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.pending_changes = true;
        assert_eq!(next_action(&input), NextAction::Sync);
        assert_eq!(
            next_action(&input).command().as_deref(),
            Some("orbit remote sync")
        );
    }

    #[test]
    fn next_action_falls_back_to_issue_hints_then_none() {
        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.repo_not_linked = true;
        input.repo_not_linked_repository = Some("acme/other");
        input.repository_issue = Some("acme/widget");
        assert_eq!(
            next_action(&input),
            NextAction::ConnectProject {
                repository: Some("acme/other".to_string())
            }
        );
        assert_eq!(
            next_action(&input).command().as_deref(),
            Some("orbit remote connect acme/other")
        );

        // A repository-carrying issue without a usable repository name
        // still resolves the historical `OWNER/REPO` placeholder.
        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.repo_not_linked = true;
        assert_eq!(
            next_action(&input).command().as_deref(),
            Some("orbit remote connect OWNER/REPO")
        );

        let fields = resolved_fields();
        let mut input = next_input(&fields);
        input.repository_issue = Some("acme/widget");
        assert_eq!(
            next_action(&input),
            NextAction::CheckRepositoryAccess {
                repository: Some("acme/widget".to_string())
            }
        );
        assert_eq!(
            next_action(&input).command().as_deref(),
            Some("gh repo view acme/widget")
        );

        let fields = resolved_fields();
        let input = next_input(&fields);
        assert_eq!(next_action(&input), NextAction::None);
        assert_eq!(next_action(&input).action(), "none");
        assert_eq!(next_action(&input).command(), None);
    }
}
