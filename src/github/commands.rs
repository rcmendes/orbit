use chrono::Utc;
use serde_json::{Value, json};

use crate::atomic;
use crate::board::Board;
use crate::cli::{
    GitHubAdoptArgs, GitHubArgs, GitHubCommand, GitHubCreateArgs, GitHubRepoCommand,
    GitHubRepoLinkArgs, GitHubSetupArgs, GitHubSyncArgs, GitHubSyncDirection,
};
use crate::error::{OrbitError, Result, code};
use crate::git::CommitOutcome;
use crate::github::client::{CreateIssueInput, GitHubClient, UpdateIssueInput, client_from_env};
use crate::github::fields::parse_area;
use crate::github::link::{GitHubLink, GitHubSyncBaseline};
use crate::github::model::{IssueSnapshot, SyncDirection, SyncPlan};
use crate::github::relationships;
use crate::github::sync::{self, OrbitSyncView, validate_orbit_metadata};
use crate::graph::{Graph, LoadMode};
use crate::task::{self, Task};

pub fn run(board: &Board, args: GitHubArgs) -> Result<crate::output::CommandOutput> {
    match args.command {
        GitHubCommand::Status => cmd_status(board),
        GitHubCommand::Connect => cmd_connect(board),
        command => {
            require_github_section(board)?;
            match command {
                GitHubCommand::Setup(args) => cmd_setup(board, args),
                GitHubCommand::Repo(command) => cmd_repo(board, command),
                GitHubCommand::Adopt(args) => cmd_adopt(board, args),
                GitHubCommand::Create(args) => cmd_create(board, args),
                GitHubCommand::Sync(args) => cmd_sync(board, args),
                GitHubCommand::Connect => unreachable!(),
                GitHubCommand::Status => unreachable!(),
            }
        }
    }
}

fn cmd_status(board: &Board) -> Result<crate::output::CommandOutput> {
    let client = client_from_env()?;
    let auth = client.auth_status();
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let github = &board.config.github;
    let configured = github.is_enabled();

    let username = auth
        .username
        .clone()
        .or_else(|| parse_auth_username(&auth.message));
    let scopes = if auth.scopes.is_empty() {
        parse_auth_scopes(&auth.message)
    } else {
        auth.scopes.clone()
    };
    let required_scopes = ["read:org", "repo"];
    let missing_scopes: Vec<_> = if scopes.is_empty() {
        Vec::new()
    } else {
        required_scopes
            .iter()
            .filter(|required| !scopes.iter().any(|scope| scope == **required))
            .copied()
            .collect()
    };

    let mut issues = Vec::<Value>::new();
    if !configured {
        issues.push(json!({
            "code": "GITHUB_NOT_CONFIGURED",
            "message": "GitHub is not configured for this Board",
        }));
    }
    if !auth.authenticated {
        issues.push(json!({
            "code": "GITHUB_AUTH_REQUIRED",
            "message": auth.message,
        }));
    }
    if !missing_scopes.is_empty() {
        issues.push(json!({
            "code": "GITHUB_SCOPES_MISSING",
            "message": format!("GitHub token is missing required scopes: {}", missing_scopes.join(", ")),
        }));
    }

    let mut repositories = Vec::new();
    if auth.authenticated {
        for name in &github.repositories {
            match client.repository_info(name) {
                Ok(info) => repositories.push(json!({
                    "name": name,
                    "accessible": true,
                    "owner_type": info.owner_type,
                })),
                Err(error) => {
                    issues.push(json!({
                        "code": error.code,
                        "message": error.message,
                        "repository": name,
                    }));
                    repositories.push(json!({
                        "name": name,
                        "accessible": false,
                        "owner_type": null,
                    }));
                }
            }
        }
    } else {
        repositories.extend(github.repositories.iter().map(|name| {
            json!({
                "name": name,
                "accessible": null,
                "owner_type": null,
            })
        }));
    }
    if configured && github.repositories.is_empty() {
        issues.push(json!({
            "code": "GITHUB_REPO_NOT_LINKED",
            "message": "No GitHub repositories are linked to this Board",
        }));
    }

    let area_id_configured = github.issue_fields.area_id.is_some();
    let verification_id_configured = github.issue_fields.verification_id.is_some();
    let mut area_resolved = None;
    let mut verification_resolved = None;
    let mut area_matches = None;
    let mut verification_matches = None;
    let mut field_validation_failed = false;
    if let Some(org) = github.organization.as_deref() {
        if auth.authenticated {
            match client.validate_org_fields(org, &github.issue_fields) {
                Ok(fields) => {
                    area_resolved = Some(true);
                    verification_resolved = Some(true);
                    area_matches = Some(
                        github.issue_fields.area_id.as_deref() == Some(fields.area_id.as_str()),
                    );
                    verification_matches = Some(
                        github.issue_fields.verification_id.as_deref()
                            == Some(fields.verification_id.as_str()),
                    );
                    if area_matches == Some(false) || verification_matches == Some(false) {
                        issues.push(json!({
                            "code": "GITHUB_SETUP_INCOMPLETE",
                            "message": "stored GitHub Issue Field ids do not match the organization fields; run `orbit github setup`",
                        }));
                    }
                }
                Err(error) => {
                    field_validation_failed = true;
                    let message = error.message.clone();
                    if message.contains(&format!(
                        "missing Issue Field `{}`",
                        github.issue_fields.area_name
                    )) {
                        area_resolved = Some(false);
                    } else if message.contains(&format!(
                        "missing Issue Field `{}`",
                        github.issue_fields.verification_name
                    )) {
                        area_resolved = Some(true);
                    }
                    if message.contains(&format!(
                        "missing Issue Field `{}`",
                        github.issue_fields.verification_name
                    )) {
                        verification_resolved = Some(false);
                    } else if message.contains(&format!(
                        "missing Issue Field `{}`",
                        github.issue_fields.area_name
                    )) {
                        verification_resolved = Some(true);
                    }
                    issues.push(json!({ "code": error.code, "message": message }));
                }
            }
        }
    } else if configured {
        issues.push(json!({
            "code": "GITHUB_SETUP_INCOMPLETE",
            "message": "github.organization is not set; run `orbit github connect`",
        }));
    }

    let mut pending_pull = Vec::new();
    let mut pending_push = Vec::new();
    let mut conflicts = Vec::new();
    let mut unsupported = Vec::new();
    for task in graph.iter().filter(|task| task.github_link().is_some()) {
        let link = task.github_link().expect("filtered linked task");
        let linked_repo = github.contains_repo(&link.repo);
        if !linked_repo {
            issues.push(json!({
                "code": "GITHUB_REPO_NOT_LINKED",
                "task": task.id,
                "repository": link.repo,
                "message": format!("Task {} links to {}, which is not linked to this Board", task.id, link.repo),
            }));
        }

        let children: Vec<_> = graph
            .iter()
            .filter(|candidate| candidate.front.parent.as_deref() == Some(task.id.as_str()))
            .filter_map(|candidate| {
                candidate
                    .github_link()
                    .map(|child_link| (candidate.clone(), child_link.clone()))
            })
            .collect();
        let dependencies: Vec<_> = task
            .front
            .depends_on
            .iter()
            .filter_map(|id| graph.tasks.get(id))
            .filter_map(|dependency| {
                dependency
                    .github_link()
                    .map(|dep_link| (dependency.clone(), dep_link.clone()))
            })
            .collect();
        let empty_issue = IssueSnapshot {
            repo: link.repo.clone(),
            number: link.number,
            title: String::new(),
            body: String::new(),
            state: crate::github::model::IssueState::Open,
            labels: Vec::new(),
            assignees: Vec::new(),
            updated_at: Utc::now(),
            area: None,
            verification: None,
            parent_issue: Vec::new(),
            blocked_by: Vec::new(),
            blocking: Vec::new(),
        };
        let relationships = relationships::plan_relationship_sync(
            task,
            link,
            &empty_issue,
            &children,
            &dependencies,
        );
        unsupported.extend(relationships.unsupported.into_iter().map(|message| {
            json!({
                "task": task.id,
                "message": message,
            })
        }));

        if !auth.authenticated || !linked_repo {
            continue;
        }
        match client.get_issue(&link.repo, link.number) {
            Ok(issue) => {
                let plan =
                    sync::plan_sync(OrbitSyncView { task, link }, &issue, SyncDirection::Both);
                let pull_count = count_pull_changes(&plan);
                let push_count = count_push_changes(&plan);
                if pull_count > 0 {
                    pending_pull.push(json!({ "task": task.id, "repository": link.repo, "number": link.number, "changes": pull_count }));
                }
                if push_count > 0 {
                    pending_push.push(json!({ "task": task.id, "repository": link.repo, "number": link.number, "changes": push_count }));
                }
                if !plan.conflicts.is_empty() {
                    conflicts.push(json!({ "task": task.id, "repository": link.repo, "number": link.number, "conflicts": plan.conflicts }));
                }
            }
            Err(error) => issues.push(json!({
                "code": error.code,
                "task": task.id,
                "message": error.message,
            })),
        }
    }

    let fields = json!({
        "area": {
            "name": github.issue_fields.area_name,
            "configured": area_id_configured,
            "resolved": area_resolved,
            "matches": area_matches,
        },
        "verification": {
            "name": github.issue_fields.verification_name,
            "configured": verification_id_configured,
            "resolved": verification_resolved,
            "matches": verification_matches,
        },
    });
    let next_action = if !auth.authenticated {
        "gh auth login".to_string()
    } else if !missing_scopes.is_empty() {
        "gh auth refresh --hostname github.com --scopes read:org,repo".to_string()
    } else if !configured || github.organization.is_none() {
        "orbit github connect".to_string()
    } else if github.repositories.is_empty() {
        "orbit github repo link OWNER/REPO".to_string()
    } else if field_validation_failed
        && area_resolved.is_none()
        && verification_resolved.is_none()
    {
        "gh auth refresh --hostname github.com --scopes read:org,repo".to_string()
    } else if area_resolved == Some(false)
        || verification_resolved == Some(false)
        || area_matches == Some(false)
        || verification_matches == Some(false)
        || !area_id_configured
        || !verification_id_configured
    {
        "orbit github setup".to_string()
    } else if !unsupported.is_empty() {
        "review unsupported GitHub relationships".to_string()
    } else if !conflicts.is_empty() {
        "resolve_github_sync_conflicts".to_string()
    } else if !pending_pull.is_empty() || !pending_push.is_empty() {
        "orbit github sync".to_string()
    } else if issues
        .iter()
        .any(|issue| issue["code"] == "GITHUB_REPO_NOT_LINKED")
    {
        "orbit github repo link OWNER/REPO".to_string()
    } else if issues
        .iter()
        .any(|issue| issue["repository"].is_string() && issue["accessible"] != true)
    {
        "check GitHub repository access".to_string()
    } else {
        "none".to_string()
    };
    let next_command = match next_action.as_str() {
        "gh auth login"
        | "orbit github connect"
        | "orbit github setup"
        | "gh auth refresh --hostname github.com --scopes read:org,repo"
        | "orbit github sync" => Some(next_action.clone()),
        "orbit github repo link OWNER/REPO" => {
            let repository = issues
                .iter()
                .find(|issue| issue["code"] == "GITHUB_REPO_NOT_LINKED")
                .and_then(|issue| issue["repository"].as_str())
                .unwrap_or("OWNER/REPO");
            Some(next_action.replace("OWNER/REPO", repository))
        }
        "resolve_github_sync_conflicts" => conflicts
            .first()
            .and_then(|conflict| conflict["task"].as_str())
            .map(|task| format!("orbit github sync {task}")),
        "check GitHub repository access" => issues
            .iter()
            .find(|issue| issue["repository"].is_string() && issue["accessible"] != true)
            .and_then(|issue| issue["repository"].as_str())
            .map(|repository| format!("gh repo view {repository}")),
        _ => None,
    };
    let healthy = issues.is_empty()
        && pending_pull.is_empty()
        && pending_push.is_empty()
        && conflicts.is_empty()
        && unsupported.is_empty();
    let data = json!({
        "configured": configured,
        "healthy": healthy,
        "auth": {
            "authenticated": auth.authenticated,
            "username": username,
            "scopes": scopes,
            "scopes_known": !scopes.is_empty(),
            "required_scopes": required_scopes,
            "missing_scopes": missing_scopes,
            "message": auth.message,
        },
        "organization": github.organization,
        "repositories": repositories,
        "fields": fields,
        "linked_tasks": graph.iter().filter(|task| task.github_link().is_some()).count(),
        "pending": {
            "pull": pending_pull.iter().map(|entry| entry["changes"].as_u64().unwrap_or(0)).sum::<u64>(),
            "push": pending_push.iter().map(|entry| entry["changes"].as_u64().unwrap_or(0)).sum::<u64>(),
            "pull_tasks": pending_pull,
            "push_tasks": pending_push,
        },
        "conflicts": conflicts.iter().map(|entry| entry["conflicts"].as_array().map(Vec::len).unwrap_or(0)).sum::<usize>(),
        "conflict_details": conflicts,
        "unsupported_relationships": unsupported.len(),
        "unsupported_details": unsupported,
        "issues": issues,
        "next_action": next_action,
        "next_command": next_command,
    });
    let human = render_status(&data);
    Ok(crate::output::CommandOutput::new(data, human))
}

pub(crate) fn parse_auth_username(message: &str) -> Option<String> {
    message
        .lines()
        .find_map(|line| line.split_once("account ").map(|(_, account)| account))
        .and_then(|account| account.split_whitespace().next())
        .map(str::to_string)
}

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

pub(crate) fn doctor_issues(board: &Board) -> Vec<crate::graph::Issue> {
    let github = &board.config.github;
    if !github.is_enabled() {
        return Vec::new();
    }
    let mut issues = Vec::new();
    let client = match client_from_env() {
        Ok(client) => client,
        Err(error) => {
            issues.push(crate::graph::Issue::new(&error.code, None, error.message));
            return issues;
        }
    };
    let auth = client.auth_status();
    if !auth.authenticated {
        issues.push(crate::graph::Issue::new(
            "GITHUB_AUTH_REQUIRED",
            None,
            format!(
                "GitHub authentication is unavailable; run `gh auth login` ({})",
                auth.message
            ),
        ));
        return issues;
    }
    let scopes = if auth.scopes.is_empty() {
        parse_auth_scopes(&auth.message)
    } else {
        auth.scopes
    };
    if !scopes.is_empty() {
        let missing: Vec<_> = ["read:org", "repo"]
            .into_iter()
            .filter(|required| !scopes.iter().any(|scope| scope == required))
            .collect();
        if !missing.is_empty() {
            issues.push(crate::graph::Issue::new(
                "GITHUB_SCOPES_MISSING",
                None,
                format!("GitHub token is missing scopes: {}", missing.join(", ")),
            ));
        }
    }
    if github.organization.is_none() {
        issues.push(crate::graph::Issue::new(
            code::GITHUB_SETUP_INCOMPLETE,
            None,
            "github.organization is not set; run `orbit github connect`",
        ));
    }
    if github.repositories.is_empty() {
        issues.push(crate::graph::Issue::new(
            code::GITHUB_REPO_NOT_LINKED,
            None,
            "no GitHub repositories are linked; run `orbit github repo link OWNER/REPO`",
        ));
    }
    for repository in &github.repositories {
        if let Err(error) = client.repository_info(repository) {
            issues.push(crate::graph::Issue::new(
                &error.code,
                None,
                format!(
                    "GitHub repository `{repository}` is unavailable: {}",
                    error.message
                ),
            ));
        }
    }
    match github.organization.as_deref() {
        Some(org) => match client.validate_org_fields(org, &github.issue_fields) {
            Ok(fields) => {
                if github.issue_fields.area_id.as_deref() != Some(fields.area_id.as_str())
                    || github.issue_fields.verification_id.as_deref()
                        != Some(fields.verification_id.as_str())
                {
                    issues.push(crate::graph::Issue::new(
                        code::GITHUB_SETUP_INCOMPLETE,
                        None,
                        "GitHub Issue Field ids are missing or stale; run `orbit github setup`",
                    ));
                }
            }
            Err(error) => issues.push(crate::graph::Issue::new(
                &error.code,
                None,
                format!(
                    "GitHub Issue Fields are not ready: {}; run `orbit github setup`",
                    error.message
                ),
            )),
        },
        None => {}
    }
    issues
}

fn count_pull_changes(plan: &SyncPlan) -> usize {
    [
        plan.apply_title.is_some(),
        plan.apply_body.is_some(),
        plan.apply_status.is_some(),
        plan.apply_labels.is_some(),
        plan.apply_area.is_some(),
        plan.apply_verification.is_some(),
    ]
    .into_iter()
    .filter(|pending| *pending)
    .count()
}

fn count_push_changes(plan: &SyncPlan) -> usize {
    [
        plan.push_labels.is_some(),
        plan.push_area.is_some(),
        plan.push_verification.is_some(),
        plan.push_state.is_some(),
    ]
    .into_iter()
    .filter(|pending| *pending)
    .count()
}

fn render_status(data: &Value) -> String {
    let mut output = format!(
        "GitHub integration: {}\n",
        if data["healthy"] == true {
            "healthy"
        } else {
            "needs attention"
        }
    );
    let auth = &data["auth"];
    output.push_str(&format!(
        "auth: {}{}\n",
        if auth["authenticated"] == true {
            "authenticated"
        } else {
            "not authenticated"
        },
        auth["username"]
            .as_str()
            .map(|name| format!(" as {name}"))
            .unwrap_or_default()
    ));
    let scopes = data["auth"]["scopes"]
        .as_array()
        .map(|scopes| {
            scopes
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    output.push_str(&format!(
        "scopes: {}{}\n",
        if scopes.is_empty() {
            "(unknown)"
        } else {
            &scopes
        },
        data["auth"]["missing_scopes"]
            .as_array()
            .filter(|missing| !missing.is_empty())
            .map(|missing| {
                let missing = missing
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(" (missing: {missing})")
            })
            .unwrap_or_default()
    ));
    output.push_str(&format!(
        "organization: {}\n",
        data["organization"].as_str().unwrap_or("(not configured)")
    ));
    if let Some(details) = data["conflict_details"].as_array() {
        for detail in details {
            let fields = detail["conflicts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|conflict| conflict["field"].as_str())
                .collect::<Vec<_>>()
                .join(", ");
            output.push_str(&format!(
                "conflict: {} ({fields})\n",
                detail["task"].as_str().unwrap_or("unknown task")
            ));
        }
    }
    if let Some(details) = data["unsupported_details"].as_array() {
        for detail in details {
            output.push_str(&format!(
                "unsupported: {}\n",
                detail["message"].as_str().unwrap_or("unknown relationship")
            ));
        }
    }
    output.push_str("repositories:\n");
    if let Some(repositories) = data["repositories"].as_array() {
        if repositories.is_empty() {
            output.push_str("  (none linked)\n");
        }
        for repository in repositories {
            output.push_str(&format!(
                "  - {} ({})\n",
                repository["name"].as_str().unwrap_or("unknown"),
                match repository["accessible"].as_bool() {
                    Some(true) => "accessible",
                    Some(false) => "inaccessible",
                    None => "not checked",
                }
            ));
        }
    }
    output.push_str(&format!(
        "fields: {} {} / {} {}\n",
        data["fields"]["area"]["name"].as_str().unwrap_or("Area"),
        field_human_status(&data["fields"]["area"]),
        data["fields"]["verification"]["name"]
            .as_str()
            .unwrap_or("Verification"),
        field_human_status(&data["fields"]["verification"]),
    ));
    output.push_str(&format!(
        "linked Tasks: {}; pending pull: {}; pending push: {}; conflicts: {}; unsupported relationships: {}\n",
        data["linked_tasks"].as_u64().unwrap_or(0),
        data["pending"]["pull"].as_u64().unwrap_or(0),
        data["pending"]["push"].as_u64().unwrap_or(0),
        data["conflicts"].as_u64().unwrap_or(0),
        data["unsupported_relationships"].as_u64().unwrap_or(0),
    ));
    output.push_str(&format!(
        "next: {}\n",
        data["next_command"]
            .as_str()
            .or_else(|| data["next_action"].as_str())
            .unwrap_or("none")
    ));
    if let Some(issues) = data["issues"].as_array() {
        for issue in issues {
            output.push_str(&format!(
                "issue: [{}] {}\n",
                issue["code"].as_str().unwrap_or("GITHUB_STATUS"),
                issue["message"].as_str().unwrap_or("GitHub status check failed")
            ));
        }
    }
    output
}

fn field_human_status(field: &Value) -> &'static str {
    match (field["configured"].as_bool(), field["resolved"].as_bool()) {
        (Some(true), Some(true)) if field["matches"] != false => "configured and resolved",
        (Some(true), Some(true)) => "resolved but stored id is stale",
        (Some(false), _) => "not configured",
        (_, Some(false)) => "missing from organization",
        _ => "not checked",
    }
}

fn cmd_connect(board: &Board) -> Result<crate::output::CommandOutput> {
    let repo = parse_github_remote(&crate::git::origin_url()?)?;
    let client = client_from_env()?;
    let auth = client.auth_status();
    if !auth.authenticated {
        return Ok(connect_blocked(
            json!({
                "repository": repo,
                "owner_type": null,
                "organization": null,
                "field_status": {"area": "unknown", "verification": "unknown"},
                "next_action": "authenticate_gh",
                "remediation": "Install GitHub CLI if needed, then run `gh auth login` and rerun `orbit github connect`.",
                "auth_message": auth.message,
            }),
            "GitHub CLI authentication is missing or insufficient. Install `gh` if needed, run `gh auth login`, then rerun `orbit github connect`.".to_string(),
        ));
    }

    let repository = match client.repository_info(&repo) {
        Ok(repository) => repository,
        Err(error) => {
            return Ok(connect_blocked(
                json!({
                    "repository": repo,
                    "owner_type": null,
                    "organization": null,
                    "field_status": {"area": "unknown", "verification": "unknown"},
                    "next_action": "grant_repository_access",
                    "remediation": format!("Confirm `{repo}` exists and that the account from `gh auth status` can access it, then rerun `orbit github connect`."),
                    "access_error": error.message,
                }),
                format!(
                    "Cannot access `{repo}` with the current GitHub account. Confirm the repository exists and grant that account access, then rerun `orbit github connect`."
                ),
            ));
        }
    };

    if repository.owner_type.eq_ignore_ascii_case("user") {
        return Ok(connect_blocked(
            json!({
                "repository": repo,
                "owner_type": "user",
                "organization": null,
                "field_status": {"area": "not_applicable", "verification": "not_applicable"},
                "next_action": "choose_personal_account_path",
                "next_actions": [
                    "Use an organization-owned GitHub repository to enable Issue Fields.",
                    "Use `orbit github basic` when personal-account basic mode is available.",
                    "Cancel and leave this Board unchanged."
                ],
            }),
            format!(
                "`{repo}` is owned by a personal account. Organization Issue Fields are unavailable; use an organization-owned repository, use basic mode when available, or cancel. No configuration was changed."
            ),
        ));
    }

    if !repository.owner_type.eq_ignore_ascii_case("organization") {
        return Ok(connect_blocked(
            json!({
                "repository": repo,
                "owner_type": repository.owner_type,
                "organization": null,
                "field_status": {"area": "unknown", "verification": "unknown"},
                "next_action": "check_repository_owner",
                "remediation": "Check that the origin is a GitHub repository owned by a User or Organization.",
            }),
            format!(
                "Could not determine whether `{repo}` is owned by a GitHub user or organization. Check the origin repository."
            ),
        ));
    }

    let organization = repo.split('/').next().unwrap_or_default();
    let setup = match client.validate_org_fields(organization, &board.config.github.issue_fields) {
        Ok(setup) => setup,
        Err(error) => {
            let message = error.message;
            let area_missing = message.contains(&format!(
                "missing Issue Field `{}`",
                board.config.github.issue_fields.area_name
            ));
            let verification_missing = message.contains(&format!(
                "missing Issue Field `{}`",
                board.config.github.issue_fields.verification_name
            ));
            let fields_missing = area_missing || verification_missing;
            let area_status = if area_missing {
                "missing"
            } else if fields_missing {
                "ready"
            } else {
                "unknown"
            };
            let verification_status = if verification_missing {
                "missing"
            } else if fields_missing {
                "ready"
            } else {
                "unknown"
            };
            return Ok(connect_blocked(
                json!({
                    "repository": repo,
                    "owner_type": "organization",
                    "organization": organization,
                    "field_status": {"area": area_status, "verification": verification_status},
                    "next_action": if fields_missing {
                        "create_issue_fields"
                    } else {
                        "grant_issue_field_access"
                    },
                    "remediation": format!("Create the missing organization Issue Field(s) `{}` and `{}` for `{organization}`, then rerun `orbit github connect`. If they exist, run `gh auth refresh --hostname github.com --scopes read:org,repo` and retry.", board.config.github.issue_fields.area_name, board.config.github.issue_fields.verification_name),
                    "field_error": message,
                }),
                format!(
                    "Organization `{organization}` is missing a required Issue Field or the current account cannot read it. Create `{}` and `{}`, or refresh `gh` scopes with `gh auth refresh --hostname github.com --scopes read:org,repo`, then rerun `orbit github connect`.",
                    board.config.github.issue_fields.area_name,
                    board.config.github.issue_fields.verification_name
                ),
            ));
        }
    };

    let changed = board.config.github.organization.as_deref() != Some(organization)
        || !board.config.github.contains_repo(&repo)
        || board.config.github.issue_fields.area_id.as_deref() != Some(setup.area_id.as_str())
        || board.config.github.issue_fields.verification_id.as_deref()
            != Some(setup.verification_id.as_str());
    if changed {
        board.save_config("orbit: github connect", |config| {
            config.github.organization = Some(organization.to_string());
            config.github.link_repo(&repo)?;
            config.github.issue_fields.area_id = Some(setup.area_id);
            config.github.issue_fields.verification_id = Some(setup.verification_id);
            Ok(())
        })?;
    }
    Ok(crate::output::CommandOutput::new(
        json!({
            "status": "connected",
            "owner_type": "organization",
            "organization": organization,
            "repository": repo,
            "field_status": {"area": "ready", "verification": "ready"},
            "next_action": "connected",
            "changed": changed,
        }),
        format!(
            "Connected `{repo}` to the Board (organization `{organization}`; Area and Verification fields ready).{}",
            if changed {
                " Configuration updated."
            } else {
                " Configuration was already current."
            }
        ),
    ))
}

fn connect_blocked(data: serde_json::Value, human: String) -> crate::output::CommandOutput {
    let mut data = data;
    if let Some(object) = data.as_object_mut() {
        object.insert("status".to_string(), json!("blocked"));
        object.insert("changed".to_string(), json!(false));
    }
    crate::output::CommandOutput::new(data, human)
}

fn parse_github_remote(remote: &str) -> Result<String> {
    let path = if let Some(path) = remote.strip_prefix("https://github.com/") {
        path
    } else if let Some(path) = remote.strip_prefix("http://github.com/") {
        path
    } else if let Some(path) = remote.strip_prefix("ssh://git@github.com/") {
        path
    } else if let Some(path) = remote.strip_prefix("git@github.com:") {
        path
    } else {
        return Err(OrbitError::new(
            code::GITHUB_NOT_CONFIGURED,
            format!(
                "origin `{remote}` is not a supported github.com remote; use HTTPS or SSH and rerun `orbit github connect`"
            ),
        ));
    };
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    crate::github::GitHubConfig::normalize_repo(path)
}

fn require_github_section(board: &Board) -> Result<()> {
    if board.config.github.is_enabled() {
        Ok(())
    } else {
        Err(OrbitError::new(
            code::GITHUB_NOT_CONFIGURED,
            "github provider is not configured; add a [github] section to config.toml",
        ))
    }
}

fn cmd_setup(board: &Board, args: GitHubSetupArgs) -> Result<crate::output::CommandOutput> {
    let org = board.config.github.require_organization()?;
    let client = client_from_env()?;
    let setup = client.validate_org_fields(org, &board.config.github.issue_fields)?;
    if args.validate_only {
        return Ok(crate::output::CommandOutput::new(
            json!({
                "organization": org,
                "area_id": setup.area_id,
                "verification_id": setup.verification_id,
                "validated": true,
            }),
            format!("GitHub Issue Fields are configured for `{org}`"),
        ));
    }
    let area_id = setup.area_id.clone();
    let verification_id = setup.verification_id.clone();
    board.save_config("orbit: github setup", |config| {
        config.github.issue_fields.area_id = Some(setup.area_id);
        config.github.issue_fields.verification_id = Some(setup.verification_id);
        Ok(())
    })?;
    Ok(crate::output::CommandOutput::new(
        json!({
            "organization": org,
            "area_id": area_id,
            "verification_id": verification_id,
        }),
        format!("Stored GitHub Issue Field ids for `{org}`"),
    ))
}

fn cmd_repo(board: &Board, command: GitHubRepoCommand) -> Result<crate::output::CommandOutput> {
    match command {
        GitHubRepoCommand::Link(args) => cmd_repo_link(board, args),
        GitHubRepoCommand::Unlink(args) => cmd_repo_unlink(board, args),
        GitHubRepoCommand::List => {
            let repos = board.config.github.repositories.clone();
            Ok(crate::output::CommandOutput::new(
                json!({ "repositories": repos, "count": repos.len() }),
                if repos.is_empty() {
                    "No GitHub repositories are linked".to_string()
                } else {
                    format!("Linked repositories: {}", repos.join(", "))
                },
            ))
        }
    }
}

fn cmd_repo_link(board: &Board, args: GitHubRepoLinkArgs) -> Result<crate::output::CommandOutput> {
    let repo = crate::github::GitHubConfig::normalize_repo(&args.repository)?;
    board.save_config("orbit: github repo link", |config| {
        config.github.link_repo(&repo)?;
        Ok(())
    })?;
    Ok(crate::output::CommandOutput::new(
        json!({ "repository": repo, "linked": true }),
        format!("Linked `{repo}`"),
    ))
}

fn cmd_repo_unlink(board: &Board, args: GitHubRepoLinkArgs) -> Result<crate::output::CommandOutput> {
    let repo = crate::github::GitHubConfig::normalize_repo(&args.repository)?;
    board.save_config("orbit: github repo unlink", |config| {
        if config.github.unlink_repo(&repo)? {
            Ok(())
        } else {
            Err(OrbitError::not_found(&repo))
        }
    })?;
    Ok(crate::output::CommandOutput::new(
        json!({ "repository": repo, "linked": false }),
        format!("Unlinked `{repo}`"),
    ))
}

fn cmd_adopt(board: &Board, args: GitHubAdoptArgs) -> Result<crate::output::CommandOutput> {
    let (repo, number) = resolve_adopt_identifier(&args)?;
    board.config.github.require_linked_repo(&repo)?;
    let client = client_from_env()?;
    let issue = client.get_issue(&repo, number)?;
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let task = graph.get(&args.task)?.clone();
    if task.github_link().is_some() {
        return Err(OrbitError::invalid_argument(format!(
            "task {} already links to a GitHub Issue",
            args.task
        )));
    }
    let mut task = task;
    task.front.github = Some(GitHubLink::new(repo.clone(), number));
    task.front.updated_at = Utc::now();
    apply_issue_to_task(&mut task, &issue)?;
    validate_orbit_metadata(&task)?;
    persist_task(board, &task, &format!("orbit: github adopt {}", task.id))?;
    let human = format!("Adopted {repo}/#{number} for {}", task.id);
    Ok(crate::output::CommandOutput::new(
        json!({ "task": task.id, "github": task.front.github, "issue": issue }),
        human,
    ))
}

fn cmd_create(board: &Board, args: GitHubCreateArgs) -> Result<crate::output::CommandOutput> {
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let task = graph.get(&args.task)?.clone();
    if task.github_link().is_some() {
        return Err(OrbitError::invalid_argument(format!(
            "task {} already links to a GitHub Issue",
            args.task
        )));
    }
    let repo = select_create_repo(&board.config.github, args.repo.as_deref())?;
    board.config.github.require_linked_repo(&repo)?;
    let client = client_from_env()?;
    let issue = client.create_issue(
        &repo,
        &CreateIssueInput {
            title: task.front.title.clone(),
            body: task.body.clone(),
            labels: task.front.labels.clone(),
            area: task.front.area.map(|area| area.as_str().to_string()),
            verification: task.front.verification.clone(),
        },
    )?;
    let mut task = task;
    task.front.github = Some(GitHubLink::new(repo.clone(), issue.number));
    task.front.updated_at = Utc::now();
    set_baseline(&mut task, &issue);
    persist_task(board, &task, &format!("orbit: github create {}", task.id))?;
    Ok(crate::output::CommandOutput::new(
        json!({ "task": task.id, "github": task.front.github, "issue": issue }),
        format!("Created {}/#{} for {}", repo, issue.number, task.id),
    ))
}

fn resolve_adopt_identifier(args: &GitHubAdoptArgs) -> Result<(String, u64)> {
    if let Some(url) = args.issue_url.as_deref() {
        let (repo, number) = parse_issue_url(url)?;
        if let Some(input_repo) = args.repo.as_deref() {
            let input_repo = crate::github::GitHubConfig::normalize_repo(input_repo)?;
            if !input_repo.eq_ignore_ascii_case(&repo) {
                return Err(OrbitError::invalid_argument(format!(
                    "repository `{input_repo}` does not match the repository `{repo}` in the GitHub Issue URL"
                )));
            }
        }
        if let Some(input_number) = args.number
            && input_number != number
        {
            return Err(OrbitError::invalid_argument(format!(
                "issue number {input_number} does not match issue number {number} in the GitHub Issue URL"
            )));
        }
        return Ok((repo, number));
    }

    match (args.repo.as_deref(), args.number) {
        (Some(repo), Some(number)) => {
            Ok((crate::github::GitHubConfig::normalize_repo(repo)?, number))
        }
        _ => Err(OrbitError::invalid_argument(
            "provide a GitHub Issue URL, or both --repo OWNER/REPO and --number NUMBER (example: `orbit github adopt TASK https://github.com/acme/widget/issues/42`)",
        )),
    }
}

fn parse_issue_url(url: &str) -> Result<(String, u64)> {
    let invalid = || {
        OrbitError::invalid_argument(format!(
            "invalid GitHub Issue URL `{url}`; expected `https://github.com/OWNER/REPO/issues/NUMBER`"
        ))
    };
    let (scheme, address) = url.split_once("://").ok_or_else(invalid)?;
    if !scheme.eq_ignore_ascii_case("https") {
        return Err(invalid());
    }
    let (host, path) = address.split_once('/').ok_or_else(invalid)?;
    if !host.eq_ignore_ascii_case("github.com")
        || path
            .chars()
            .any(|character| character.is_whitespace() || matches!(character, '?' | '#' | '\\'))
    {
        return Err(invalid());
    }
    let parts: Vec<_> = path.split('/').collect();
    let parts = if parts.last() == Some(&"") {
        &parts[..parts.len() - 1]
    } else {
        &parts[..]
    };
    if parts.len() != 4 || parts[2] != "issues" {
        return Err(invalid());
    }
    let repo = crate::github::GitHubConfig::normalize_repo(&format!("{}/{}", parts[0], parts[1]))
        .map_err(|_| invalid())?;
    let number = parts[3].parse::<u64>().map_err(|_| invalid())?;
    if number == 0 {
        return Err(invalid());
    }
    Ok((repo, number))
}

fn select_create_repo(
    config: &crate::github::GitHubConfig,
    requested: Option<&str>,
) -> Result<String> {
    if let Some(repo) = requested {
        return crate::github::GitHubConfig::normalize_repo(repo);
    }
    match config.repositories.as_slice() {
        [repo] => crate::github::GitHubConfig::normalize_repo(repo),
        [] => Err(OrbitError::invalid_argument(
            "no GitHub repository is linked; link one with `orbit github repo link OWNER/REPO`, then create without --repo or pass --repo OWNER/REPO",
        )),
        _ => Err(OrbitError::invalid_argument(
            "multiple GitHub repositories are linked; choose one with --repo OWNER/REPO",
        )),
    }
}

fn cmd_sync(board: &Board, args: GitHubSyncArgs) -> Result<crate::output::CommandOutput> {
    let direction = match args.direction {
        GitHubSyncDirection::Both => SyncDirection::Both,
        GitHubSyncDirection::Pull => SyncDirection::Pull,
        GitHubSyncDirection::Push => SyncDirection::Push,
    };
    let client = client_from_env()?;
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let targets: Vec<String> = if let Some(task) = args.task {
        vec![task]
    } else {
        graph
            .iter()
            .filter(|task| task.github_link().is_some())
            .map(|task| task.id.clone())
            .collect()
    };
    if targets.is_empty() {
        return Ok(crate::output::CommandOutput::new(
            json!({ "tasks": [], "count": 0 }),
            "No linked Tasks to sync (nothing was written).".to_string(),
        ));
    }

    let mut plans = Vec::new();
    let mut human_plans = Vec::new();
    for id in &targets {
        let task = graph.get(id)?;
        let link = task
            .github_link()
            .ok_or_else(|| OrbitError::invalid_argument(format!("task {id} has no GitHub link")))?
            .clone();
        board.config.github.require_linked_repo(&link.repo)?;
        let issue = client.get_issue(&link.repo, link.number)?;
        let plan = sync::plan_sync(
            OrbitSyncView { task, link: &link },
            &issue,
            direction,
        );
        human_plans.push(format_sync_preview(task, &link, &plan));
        let relationship = relationships::plan_relationship_sync(task, &link, &issue, &[], &[]);
        plans.push(json!({
            "task": id,
            "github": link,
            "plan": plan,
            "relationships": relationship,
        }));
        if args.apply && !plan.has_conflicts() {
            apply_plan(board, task, &link, &plan, &client)?;
        } else if args.apply && plan.has_conflicts() {
            return Err(OrbitError::new(
                code::GITHUB_ISSUE_CONFLICT,
                format!("sync for {id} has unresolved conflicts"),
            )
            .with_details(json!({ "task": id, "conflicts": plan.conflicts })));
        }
    }

    let human = if args.apply {
        format!("Applied GitHub sync for {} task(s)", targets.len())
    } else {
        format!(
            "GitHub sync preview (dry run; nothing was written)\n{}",
            human_plans.join("\n")
        )
    };
    Ok(crate::output::CommandOutput::new(
        json!({ "tasks": plans, "count": plans.len(), "apply": args.apply }),
        human,
    ))
}

fn format_sync_preview(task: &Task, link: &GitHubLink, plan: &SyncPlan) -> String {
    let mut output = format!(
        "\nTask: {} — {}\nIssue: {}#{}\n",
        task.id, task.front.title, link.repo, link.number
    );

    let mut pulls = Vec::new();
    push_preview_field(&mut pulls, "title", &plan.apply_title);
    push_preview_field(&mut pulls, "body", &plan.apply_body);
    push_preview_field(&mut pulls, "status", &plan.apply_status);
    push_preview_field(&mut pulls, "labels", &plan.apply_labels);
    push_preview_field(&mut pulls, "area", &plan.apply_area);
    push_preview_field(&mut pulls, "verification", &plan.apply_verification);
    output.push_str("Pull into Orbit:\n");
    append_preview_fields(&mut output, &pulls);

    let mut pushes = Vec::new();
    push_preview_field(&mut pushes, "labels", &plan.push_labels);
    push_preview_field(&mut pushes, "area", &plan.push_area);
    push_preview_field(&mut pushes, "verification", &plan.push_verification);
    push_preview_field(&mut pushes, "state", &plan.push_state);
    output.push_str("Push to GitHub:\n");
    append_preview_fields(&mut output, &pushes);

    output.push_str("Conflicts:\n");
    if plan.conflicts.is_empty() {
        output.push_str("  (none)\n");
    } else {
        for conflict in &plan.conflicts {
            output.push_str(&format!(
                "  - {}: Orbit = {}; GitHub = {}\n",
                conflict.field,
                format_preview_value(&conflict.orbit_value),
                format_preview_value(&conflict.github_value),
            ));
        }
    }

    if plan.is_noop() && !plan.has_conflicts() {
        output.push_str("No changes needed.\n");
    }
    for warning in &plan.warnings {
        output.push_str(&format!("Warning: {warning}\n"));
    }
    output
}

fn push_preview_field<T: serde::Serialize>(
    fields: &mut Vec<String>,
    name: &str,
    value: &Option<T>,
) {
    if let Some(value) = value {
        fields.push(format!(
            "  - {name}: {}\n",
            format_preview_value(&json!(value))
        ));
    }
}

fn append_preview_fields(output: &mut String, fields: &[String]) {
    if fields.is_empty() {
        output.push_str("  (none)\n");
    } else {
        for field in fields {
            output.push_str(field);
        }
    }
}

fn format_preview_value(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unprintable>".to_string())
}

fn apply_plan(
    board: &Board,
    task: &Task,
    link: &GitHubLink,
    plan: &SyncPlan,
    client: &Box<dyn GitHubClient>,
) -> Result<()> {
    let mut task = task.clone();
    if let Some(title) = &plan.apply_title {
        task.front.title = title.clone();
    }
    if let Some(body) = &plan.apply_body {
        task.body = body.clone();
    }
    if let Some(status) = plan.apply_status {
        task.front.status = status;
    }
    if let Some(labels) = &plan.apply_labels {
        task.front.labels = labels.clone();
    }
    if let Some(area) = &plan.apply_area {
        task.front.area = area
            .as_ref()
            .map(|value| parse_area(value))
            .transpose()?;
    }
    if let Some(verification) = &plan.apply_verification {
        task.front.verification = verification.clone();
    }
    task.front.updated_at = Utc::now();

    if plan.push_labels.is_some() || plan.push_state.is_some() {
        client.update_issue(
            &link.repo,
            link.number,
            &UpdateIssueInput {
                title: None,
                body: None,
                state: plan.push_state,
                labels: plan.push_labels.clone(),
                area: plan.push_area.clone(),
                verification: plan.push_verification.clone(),
            },
        )?;
    }

    let refreshed = client.get_issue(&link.repo, link.number)?;
    set_baseline(&mut task, &refreshed);
    persist_task(board, &task, &format!("orbit: github sync {}", task.id))?;
    Ok(())
}

fn apply_issue_to_task(task: &mut Task, issue: &IssueSnapshot) -> Result<()> {
    task.front.title = issue.title.clone();
    task.body = issue.body.clone();
    task.front.labels = issue.labels.clone();
    task.front.area = issue
        .area
        .as_deref()
        .map(parse_area)
        .transpose()?;
    task.front.verification = issue.verification.clone();
    task.front.status = crate::github::status::status_from_issue(issue.state, &issue.labels);
    set_baseline(task, issue);
    Ok(())
}

fn set_baseline(task: &mut Task, issue: &IssueSnapshot) {
    if let Some(link) = task.front.github.as_mut() {
        link.last_sync = Some(GitHubSyncBaseline {
            at: Utc::now(),
            issue_updated_at: issue.updated_at,
            orbit_updated_at: task.front.updated_at,
            title: issue.title.clone(),
            body: issue.body.clone(),
            labels: issue.labels.clone(),
            area: task.front.area.map(|area| area.as_str().to_string()),
            verification: task.front.verification.clone(),
        });
    }
}

fn persist_task(board: &Board, task: &Task, message: &str) -> Result<CommitOutcome> {
    let rendered = task::render_task(task)?;
    atomic::write_atomic(&board.task_path(&task.id), rendered.as_bytes(), true)?;
    board.commit(message, false)
}
