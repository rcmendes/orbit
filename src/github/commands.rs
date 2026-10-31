use super::execution::{
    fetch_issue, reject_basic_metadata, repository_available, require_repository_available,
    set_baseline,
};
use crate::github::execution::SyncOutcome;
use crate::mutation::persist_task;
use chrono::Utc;
use serde_json::{Value, json};

use crate::board::Board;
use crate::cli::{
    AutoPushState, GitHubAdoptArgs, GitHubArgs, GitHubAutoPushArgs, GitHubCommand,
    GitHubCreateArgs, GitHubRepoCommand, GitHubRepoLinkArgs, GitHubSetupArgs, GitHubSyncArgs,
    GitHubSyncDirection,
};
use crate::error::{OrbitError, Result, code};
use crate::github::client::{CreateIssueInput, UpdateIssueInput, client_from_env};
use crate::github::fields::parse_area;
use crate::github::link::GitHubLink;
use crate::github::model::{IssueSnapshot, SyncDirection, SyncPlan};
use crate::github::relationships;
use crate::github::sync::{self, OrbitSyncView, validate_orbit_metadata};
use crate::graph::{Graph, LoadMode};
use crate::task::Task;

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
                GitHubCommand::AutoPush(args) => cmd_auto_push(board, args),
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
    let basic_mode = github.mode.is_basic();

    let username = auth
        .username
        .clone()
        .or_else(|| crate::github::client::parse_auth_username(&auth.message));
    let scopes = if auth.scopes.is_empty() {
        crate::github::client::parse_auth_scopes(&auth.message)
    } else {
        auth.scopes.clone()
    };
    let (required_scopes, missing_scopes) = github_scope_status(basic_mode, &scopes);

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

    let mut linked_repositories = github.repositories.clone();
    if let Some(workspace_repositories) = crate::workspace::export_repositories(board)? {
        for repository in workspace_repositories {
            if !linked_repositories
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&repository))
            {
                linked_repositories.push(repository);
            }
        }
    }

    let mut repositories = Vec::new();
    if auth.authenticated {
        for name in &linked_repositories {
            match client.repository_info(name) {
                Ok(info) => {
                    let label_present = client
                        .label_exists(name, crate::github::status::IN_PROGRESS_LABEL)
                        .ok();
                    if github.auto_push && label_present == Some(false) {
                        issues.push(json!({
                            "code": "GITHUB_SETUP_INCOMPLETE",
                            "message": format!(
                                "the `{}` label is missing on `{name}`; run `orbit github connect` or create it",
                                crate::github::status::IN_PROGRESS_LABEL
                            ),
                            "repository": name,
                        }));
                    }
                    repositories.push(json!({
                        "name": name,
                        "accessible": true,
                        "owner_type": info.owner_type,
                        "reserved_label_present": label_present,
                    }));
                }
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
                        "reserved_label_present": null,
                    }));
                }
            }
        }
    } else {
        repositories.extend(linked_repositories.iter().map(|name| {
            json!({
                "name": name,
                "accessible": null,
                "owner_type": null,
                "reserved_label_present": null,
            })
        }));
    }
    if configured && linked_repositories.is_empty() {
        issues.push(json!({
            "code": "GITHUB_REPO_NOT_LINKED",
            "message": "No GitHub repositories are linked to this Board",
        }));
    }

    let area_id_configured = !basic_mode && github.issue_fields.area_id.is_some();
    let verification_id_configured = !basic_mode && github.issue_fields.verification_id.is_some();
    let mut area_resolved = None;
    let mut verification_resolved = None;
    let mut area_matches = None;
    let mut verification_matches = None;
    let mut field_validation_failed = false;
    if !basic_mode && let Some(org) = github.organization.as_deref() {
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
                    let missing = missing_field_names(&error);
                    if !missing.is_empty() {
                        area_resolved =
                            Some(!missing.contains(&github.issue_fields.area_name.as_str()));
                        verification_resolved = Some(
                            !missing.contains(&github.issue_fields.verification_name.as_str()),
                        );
                    }
                    issues.push(json!({ "code": error.code, "message": error.message }));
                }
            }
        }
    } else if configured && !basic_mode {
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
        let linked_repo = repository_available(board, &link.repo)?;
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
        let relationships = relationships::plan_relationship_sync(
            link,
            &relationships::RemoteRelations::EMPTY,
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
        match fetch_issue(board, client.as_ref(), &link.repo, link.number) {
            Ok(issue) => {
                let direction = if basic_mode {
                    SyncDirection::Pull
                } else {
                    SyncDirection::Both
                };
                let plan = sync::plan_sync(OrbitSyncView { task, link }, &issue, direction);
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
            "applicable": !basic_mode,
            "configured": if basic_mode { Value::Null } else { json!(area_id_configured) },
            "resolved": area_resolved,
            "matches": area_matches,
        },
        "verification": {
            "name": github.issue_fields.verification_name,
            "applicable": !basic_mode,
            "configured": if basic_mode { Value::Null } else { json!(verification_id_configured) },
            "resolved": verification_resolved,
            "matches": verification_matches,
        },
    });
    let scope_refresh = if basic_mode {
        "gh auth refresh --hostname github.com --scopes repo"
    } else {
        "gh auth refresh --hostname github.com --scopes read:org,repo"
    };
    let next_action = if !auth.authenticated {
        "gh auth login".to_string()
    } else if !missing_scopes.is_empty() {
        scope_refresh.to_string()
    } else if !configured || (!basic_mode && github.organization.is_none()) {
        "orbit github connect".to_string()
    } else if github.repositories.is_empty() {
        "orbit github repo link OWNER/REPO".to_string()
    } else if field_validation_failed && area_resolved.is_none() && verification_resolved.is_none()
    {
        scope_refresh.to_string()
    } else if area_resolved == Some(false)
        || verification_resolved == Some(false)
        || area_matches == Some(false)
        || verification_matches == Some(false)
        || (!basic_mode && (!area_id_configured || !verification_id_configured))
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
        "gh auth login" | "orbit github connect" | "orbit github setup" | "orbit github sync" => {
            Some(next_action.clone())
        }
        "gh auth refresh --hostname github.com --scopes read:org,repo"
        | "gh auth refresh --hostname github.com --scopes repo" => Some(next_action.clone()),
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
        "mode": if basic_mode { "basic" } else { "full" },
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
        "auto_push": {
            "enabled": github.auto_push,
            "reserved_label": crate::github::status::IN_PROGRESS_LABEL,
        },
        "linked_tasks": graph.iter().filter(|task| task.github_link().is_some()).count(),
        "local_tasks": graph.iter().filter(|task| task.github_link().is_none()).count(),
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

/// The Issue Field names reported missing by `validate_org_fields`, read from
/// the error's structured details rather than its message text.
fn missing_field_names(error: &OrbitError) -> Vec<&str> {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("fields"))
        .and_then(|fields| fields.as_array())
        .map(|fields| fields.iter().filter_map(|field| field.as_str()).collect())
        .unwrap_or_default()
}

fn github_scope_status(
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

/// A repository is available when it is directly linked on a repository-local
/// Board or included by the Workspace catalog. Workspace members need no
/// per-repository `orbit github repo link`.
pub(crate) fn doctor_issues(board: &Board) -> Vec<crate::graph::Issue> {
    let github = &board.config.github;
    if !github.is_enabled() {
        return Vec::new();
    }
    let mut issues = Vec::new();
    let client = match client_from_env() {
        Ok(client) => client,
        Err(error) => {
            issues.push(crate::graph::Issue::new(error.code, None, error.message));
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
        crate::github::client::parse_auth_scopes(&auth.message)
    } else {
        auth.scopes
    };
    let (_, missing_scopes) = github_scope_status(github.mode.is_basic(), &scopes);
    if !missing_scopes.is_empty() {
        issues.push(crate::graph::Issue::new(
            "GITHUB_SCOPES_MISSING",
            None,
            format!(
                "GitHub token is missing scopes: {}",
                missing_scopes.join(", ")
            ),
        ));
    }
    if !github.mode.is_basic() && github.organization.is_none() {
        issues.push(crate::graph::Issue::new(
            code::GITHUB_SETUP_INCOMPLETE,
            None,
            "github.organization is not set; run `orbit github connect`",
        ));
    }
    if github.repositories.is_empty() && !board.is_workspace_board() {
        issues.push(crate::graph::Issue::new(
            code::GITHUB_REPO_NOT_LINKED,
            None,
            "no GitHub repositories are linked; run `orbit github repo link OWNER/REPO`",
        ));
    }
    for repository in &github.repositories {
        if let Err(error) = client.repository_info(repository) {
            issues.push(crate::graph::Issue::new(
                error.code,
                None,
                format!(
                    "GitHub repository `{repository}` is unavailable: {}",
                    error.message
                ),
            ));
        }
    }
    if github.auto_push {
        let mut repositories = github.repositories.clone();
        if let Ok(Some(workspace_repositories)) = crate::workspace::export_repositories(board) {
            for repository in workspace_repositories {
                if !repositories
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&repository))
                {
                    repositories.push(repository);
                }
            }
        }
        if repositories.is_empty() {
            issues.push(crate::graph::Issue::new(
                code::GITHUB_REPO_NOT_LINKED,
                None,
                "auto-push is enabled but no GitHub repository is linked or included; new Tasks stay local",
            ));
        }
        for repository in repositories {
            match client.label_exists(&repository, crate::github::status::IN_PROGRESS_LABEL) {
                Ok(true) => {}
                Ok(false) => issues.push(crate::graph::Issue::new(
                    code::GITHUB_SETUP_INCOMPLETE,
                    None,
                    format!(
                        "the `{}` label is missing on `{repository}`; run `orbit github connect` or create it",
                        crate::github::status::IN_PROGRESS_LABEL
                    ),
                )),
                Err(error) => issues.push(crate::graph::Issue::new(
                    error.code,
                    None,
                    format!(
                        "could not check the `{}` label on `{repository}`: {}",
                        crate::github::status::IN_PROGRESS_LABEL,
                        error.message
                    ),
                )),
            }
        }
    }
    if github.mode.is_basic() {
        return issues;
    }
    if let Some(org) = github.organization.as_deref() {
        match client.validate_org_fields(org, &github.issue_fields) {
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
                error.code,
                None,
                format!(
                    "GitHub Issue Fields are not ready: {}; run `orbit github setup`",
                    error.message
                ),
            )),
        }
    }
    issues
}

fn count_pull_changes(plan: &SyncPlan) -> usize {
    plan.pull_changes().len()
}

fn count_push_changes(plan: &SyncPlan) -> usize {
    plan.push_changes().len()
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
        data["organization"]
            .as_str()
            .unwrap_or(if data["mode"] == "basic" {
                "(not applicable in basic mode)"
            } else {
                "(not configured)"
            })
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
        "auto-push: {}{}\n",
        if data["auto_push"]["enabled"] == true {
            "enabled"
        } else {
            "disabled"
        },
        if data["auto_push"]["enabled"] == true {
            format!(
                " (reserved label {}; {} local Task(s) without an Issue)",
                data["auto_push"]["reserved_label"]
                    .as_str()
                    .unwrap_or("orbit:in-progress"),
                data["local_tasks"].as_u64().unwrap_or(0)
            )
        } else {
            String::new()
        }
    ));
    if data["auto_push"]["enabled"] == true {
        let missing = data["repositories"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|repository| repository["reserved_label_present"] == false)
            .filter_map(|repository| repository["name"].as_str())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            output.push_str(&format!(
                "reserved label missing on: {}\n",
                missing.join(", ")
            ));
        }
    }
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
                issue["message"]
                    .as_str()
                    .unwrap_or("GitHub status check failed")
            ));
        }
    }
    output
}

fn field_human_status(field: &Value) -> &'static str {
    if field["applicable"] == false {
        return "not applicable (basic mode)";
    }
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
        let remediation = format!(
            "To use `{repo}` in basic mode, edit the Board's `config.toml` and add or merge these settings (preserve any other linked repositories):\n\n[github]\nmode = \"basic\"\nrepositories = [\"{repo}\"]\n\nThen run `orbit github status`. See `docs/github-sync.md#basic-mode-personal-repositories`."
        );
        let mut output = connect_blocked(
            json!({
                "repository": repo,
                "owner_type": "user",
                "organization": null,
                "field_status": {"area": "not_applicable", "verification": "not_applicable"},
                "next_action": "choose_personal_account_path",
                "next_actions": [
                    "Use an organization-owned GitHub repository to enable Issue Fields.",
                    remediation,
                    "Cancel and leave this Board unchanged."
                ],
                "remediation": remediation,
            }),
            format!(
                "`{repo}` is owned by a personal account. Organization Issue Fields are unavailable.\n{remediation}\nNo configuration was changed."
            ),
        );
        if let Err(error) = client.ensure_label(&repo, crate::github::status::IN_PROGRESS_LABEL) {
            output.warnings.push(format!(
                "could not ensure the `{}` label: {}; auto-push will keep pushing other fields",
                crate::github::status::IN_PROGRESS_LABEL,
                error.message
            ));
        }
        return Ok(output);
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
            let message = error.message.clone();
            let missing = missing_field_names(&error);
            let area_missing =
                missing.contains(&board.config.github.issue_fields.area_name.as_str());
            let verification_missing =
                missing.contains(&board.config.github.issue_fields.verification_name.as_str());
            let fields_missing = !missing.is_empty();
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
    let mut output = crate::output::CommandOutput::new(
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
    );
    if let Err(error) = client.ensure_label(&repo, crate::github::status::IN_PROGRESS_LABEL) {
        output.warnings.push(format!(
            "could not ensure the `{}` label: {}; auto-push will keep pushing other fields",
            crate::github::status::IN_PROGRESS_LABEL,
            error.message
        ));
    }
    Ok(output)
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
    let setup = if args.create_fields {
        client.create_org_fields(org, &board.config.github.issue_fields)?
    } else {
        client.validate_org_fields(org, &board.config.github.issue_fields)?
    };
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
    let created_fields_count = setup.created_fields.len();
    board.save_config("orbit: github setup", |config| {
        config.github.issue_fields.area_id = Some(setup.area_id);
        config.github.issue_fields.verification_id = Some(setup.verification_id);
        Ok(())
    })?;
    let mut data = json!({
        "organization": org,
        "area_id": area_id,
        "verification_id": verification_id,
    });
    if args.create_fields {
        data["created_fields"] = json!(setup.created_fields);
    }
    let mut output = crate::output::CommandOutput::new(
        data,
        if args.create_fields {
            if created_fields_count == 0 {
                format!("GitHub Issue Fields already exist for `{org}`; ids stored")
            } else {
                format!(
                    "Created {created_fields_count} missing GitHub Issue Field(s) for `{org}` and stored ids"
                )
            }
        } else {
            format!("Stored GitHub Issue Field ids for `{org}`")
        },
    );
    for repo in &board.config.github.repositories {
        if let Err(error) = client.ensure_label(repo, crate::github::status::IN_PROGRESS_LABEL) {
            output.warnings.push(format!(
                "could not ensure the `{}` label on `{repo}`: {}; auto-push will keep pushing other fields",
                crate::github::status::IN_PROGRESS_LABEL,
                error.message
            ));
        }
    }
    Ok(output)
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
    let mut output = crate::output::CommandOutput::new(
        json!({ "repository": repo, "linked": true }),
        format!("Linked `{repo}`"),
    );
    if let Ok(client) = client_from_env()
        && let Err(error) = client.ensure_label(&repo, crate::github::status::IN_PROGRESS_LABEL)
    {
        output.warnings.push(format!(
            "could not ensure the `{}` label on `{repo}`: {}; auto-push will keep pushing other fields",
            crate::github::status::IN_PROGRESS_LABEL,
            error.message
        ));
    }
    Ok(output)
}

fn cmd_repo_unlink(
    board: &Board,
    args: GitHubRepoLinkArgs,
) -> Result<crate::output::CommandOutput> {
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
    require_repository_available(board, &repo)?;
    let client = client_from_env()?;
    let issue = fetch_issue(board, client.as_ref(), &repo, number)?;
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let task = graph.get(&args.task)?.clone();
    if task.front.github.as_ref().is_some_and(|link| !link.active) {
        return Err(OrbitError::invalid_argument(format!(
            "task {} retains inactive GitHub provenance; use `orbit workspace issues reattach {}#{}` after reviewing it",
            args.task,
            task.front
                .github
                .as_ref()
                .expect("inactive provenance")
                .repo,
            task.front
                .github
                .as_ref()
                .expect("inactive provenance")
                .number,
        )));
    }
    if task.github_link().is_some() {
        return Err(OrbitError::invalid_argument(format!(
            "task {} already links to a GitHub Issue",
            args.task
        )));
    }
    if let Some(existing) = graph.iter().find(|candidate| {
        candidate
            .github_link()
            .is_some_and(|link| link.repo.eq_ignore_ascii_case(&repo) && link.number == number)
    }) {
        return Err(OrbitError::invalid_argument(format!(
            "GitHub Issue {repo}#{number} is already linked to Task {}; one GitHub Issue can be paired with only one Orbit Task",
            existing.id
        )));
    }
    let mut task = task;
    task.front.github = Some(GitHubLink::new(repo.clone(), number));
    task.front.updated_at = Utc::now();
    apply_issue_to_task(&mut task, &issue, !board.config.github.mode.is_basic())?;
    validate_orbit_metadata(&task)?;
    let outcome = persist_task(board, &task, &format!("orbit: github adopt {}", task.id))?;
    let human = format!("Adopted {repo}/#{number} for {}", task.id);
    let mut output = crate::output::CommandOutput::new(
        json!({ "task": task.id, "github": task.front.github, "issue": issue }),
        human,
    );
    output
        .warnings
        .extend(crate::mutation::commit_warning(outcome));
    Ok(output)
}

fn cmd_create(board: &Board, args: GitHubCreateArgs) -> Result<crate::output::CommandOutput> {
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let workspace_repositories = crate::workspace::export_repositories(board)?;
    let repositories = workspace_repositories
        .clone()
        .unwrap_or_else(|| board.config.github.repositories.clone());
    if workspace_repositories.as_ref().is_some_and(Vec::is_empty) && args.repo.is_none() {
        return Err(OrbitError::invalid_argument(
            "the Workspace repository catalog has no included repositories; run `orbit workspace refresh` or include a repository before exporting",
        ));
    }
    let repo = select_create_repo(&repositories, args.repo.as_deref())?;
    if workspace_repositories.is_some() {
        if !repositories
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&repo))
        {
            return Err(OrbitError::invalid_argument(format!(
                "repository `{repo}` is not included in the Workspace catalog; run `orbit workspace refresh` or choose an included repository"
            )));
        }
    } else {
        board.config.github.require_linked_repo(&repo)?;
    }

    let mut plans = Vec::new();
    let mut selected = std::collections::HashSet::new();
    for id in &args.tasks {
        let task = graph.get(id)?;
        let rejected = if !selected.insert(id) {
            Some(format!(
                "task {id} appears more than once in the export selection"
            ))
        } else if task.front.github.as_ref().is_some_and(|link| !link.active) {
            Some(format!(
                "task {id} retains inactive GitHub provenance; reattach the exact Issue before exporting"
            ))
        } else if task.github_link().is_some() {
            Some(format!(
                "task {id} already links to a GitHub Issue; use `orbit github sync {id}` instead"
            ))
        } else if task.front.status.is_terminal() && !args.include_closed {
            Some(format!(
                "task {id} is {}; pass --include-closed to export done and cancelled Tasks",
                task.front.status.as_str()
            ))
        } else if board.config.github.mode.is_basic()
            && (task.front.area.is_some() || task.front.verification.is_some())
        {
            Some(
                reject_basic_metadata(task)
                    .expect_err("metadata was checked")
                    .message,
            )
        } else {
            None
        };
        plans.push((task.clone(), rejected));
    }

    let client = if args.apply && plans.iter().any(|(_, error)| error.is_none()) {
        Some(client_from_env()?)
    } else {
        None
    };
    let mut results = Vec::new();
    let mut human = Vec::new();
    let mut warnings = Vec::new();
    for (mut task, rejected) in plans {
        let issue_state = if task.front.status.is_terminal() {
            "closed"
        } else {
            "open"
        };
        if let Some(error) = rejected {
            results.push(json!({
                "task": task.id,
                "repository": repo,
                "title": task.front.title,
                "body": task.body,
                "labels": task.front.labels,
                "area": task.front.area.map(|area| area.as_str()),
                "verification": task.front.verification,
                "status": task.front.status,
                "issue_state": issue_state,
                "success": false,
                "error": error,
            }));
            human.push(format!(
                "{}\n  Result: not exported — {error}",
                format_create_preview(&task, &repo, issue_state)
            ));
            continue;
        }

        let preview = json!({
            "task": task.id,
            "repository": repo,
            "title": task.front.title,
            "body": task.body,
            "labels": task.front.labels,
            "area": task.front.area.map(|area| area.as_str()),
            "verification": task.front.verification,
            "status": task.front.status,
            "issue_state": issue_state,
        });
        if !args.apply {
            results.push(preview);
            human.push(format!(
                "{}\n  Nothing written. Apply with --apply.",
                format_create_preview(&task, &repo, issue_state)
            ));
            continue;
        }

        let client = client.as_ref().expect("apply initialized GitHub client");
        let issue = match client.create_issue(
            &repo,
            &CreateIssueInput {
                title: task.front.title.clone(),
                body: task.body.clone(),
                labels: crate::github::status::labels_for_status(
                    &task.front.labels,
                    task.front.status,
                ),
                area: task.front.area.map(|area| area.as_str().to_string()),
                verification: task.front.verification.clone(),
                organization: board.config.github.organization.clone(),
                issue_fields: board.config.github.issue_fields.clone(),
            },
        ) {
            Ok(issue) => issue,
            Err(error) => {
                results.push(json!({
                    "task": task.id,
                    "repository": repo,
                    "title": task.front.title,
                    "body": task.body,
                    "labels": task.front.labels,
                    "area": task.front.area.map(|area| area.as_str()),
                    "verification": task.front.verification,
                    "status": task.front.status,
                    "issue_state": issue_state,
                    "success": false,
                    "error": error.message,
                    "error_code": error.code,
                }));
                human.push(format!(
                    "{}\n  Result: failed — {}",
                    format_create_preview(&task, &repo, issue_state),
                    error.message
                ));
                continue;
            }
        };

        task.front.github = Some(GitHubLink::new(repo.clone(), issue.number));
        task.front.updated_at = Utc::now();
        set_baseline(&mut task, &issue);
        warnings.extend(crate::mutation::commit_warning(persist_task(
            board,
            &task,
            &format!("orbit: github create {}", task.id),
        )?));

        let mut commit_warnings = Vec::new();
        let task_id = task.id.clone();
        let mut remember_issue = |issue: &IssueSnapshot| {
            if let Err(error) = crate::issue_catalog::remember_created_issue(board, &repo, issue) {
                let warning = format!(
                    "{task_id}: created {}/#{} but could not update the Remote Issue cache: {}; run `orbit workspace issues refresh`",
                    repo, issue.number, error.message
                );
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
        };
        remember_issue(&issue);

        let final_issue = if task.front.status.is_terminal() {
            match client.update_issue(
                &repo,
                issue.number,
                &UpdateIssueInput {
                    state: Some(crate::github::model::IssueState::Closed),
                    state_reason: crate::github::status::issue_state_reason_from_status(
                        task.front.status,
                    ),
                    title: None,
                    body: None,
                    labels: None,
                    area: None,
                    verification: None,
                    organization: None,
                    issue_fields: None,
                },
            ) {
                Ok(closed) => {
                    set_baseline(&mut task, &closed);
                    commit_warnings.extend(crate::mutation::commit_warning(persist_task(
                        board,
                        &task,
                        &format!("orbit: github create {}", task.id),
                    )?));
                    remember_issue(&closed);
                    closed
                }
                Err(error) => {
                    results.push(json!({
                        "task": task.id,
                        "repository": repo,
                        "github": task.front.github,
                        "issue": issue,
                        "success": false,
                        "created": true,
                        "linked": true,
                        "error": error.message,
                        "error_code": error.code,
                        "next_action": format!("run `orbit github sync {} --apply` to close the Issue", task.id),
                    }));
                    human.push(format!(
                        "{}\n  Result: created and linked, but could not close Issue — {}",
                        format_create_preview(&task, &repo, issue_state),
                        error.message
                    ));
                    continue;
                }
            }
        } else {
            issue
        };
        warnings.append(&mut commit_warnings);
        results.push(json!({
            "task": task.id,
            "repository": repo,
            "title": task.front.title,
            "body": task.body,
            "labels": task.front.labels,
            "area": task.front.area.map(|area| area.as_str()),
            "verification": task.front.verification,
            "status": task.front.status,
            "issue_state": issue_state,
            "github": task.front.github,
            "issue": final_issue,
            "success": true,
        }));
        human.push(format!(
            "{}\n  Result: created {}/#{}",
            format_create_preview(&task, &repo, issue_state),
            repo,
            final_issue.number
        ));
    }

    let failures = results
        .iter()
        .filter(|result| result["success"] == false)
        .count();
    let successes = results
        .iter()
        .filter(|result| result["success"] == true)
        .count();
    let count = results.len();
    let data = json!({
        "repository": repo,
        "apply_required": !args.apply,
        "count": count,
        "preview_count": if args.apply { Value::Null } else { json!(count) },
        "success_count": if args.apply { json!(successes) } else { Value::Null },
        "failure_count": if args.apply { json!(failures) } else { Value::Null },
        "tasks": results,
    });
    let summary = if args.apply {
        format!("Exported {successes} Task(s); {failures} failed")
    } else {
        format!("Previewed {count} Task(s); nothing was written. Apply with --apply.")
    };
    let mut output =
        crate::output::CommandOutput::new(data, format!("{summary}\n{}", human.join("\n")));
    for warning in warnings {
        output = output.with_warning(warning);
    }
    Ok(output)
}

fn format_create_preview(task: &Task, repository: &str, issue_state: &str) -> String {
    let body = if task.body.is_empty() {
        "  (empty)".to_string()
    } else {
        task.body
            .lines()
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "Task: {}\n  Repository: {}\n  Title: {}\n  Body:\n{}\n  Labels: {}\n  Area: {}\n  Verification: {}\n  Task status: {}\n  GitHub Issue state: {}",
        task.id,
        repository,
        task.front.title,
        body,
        if task.front.labels.is_empty() {
            "(none)".to_string()
        } else {
            task.front.labels.join(", ")
        },
        task.front
            .area
            .map(|area| area.as_str())
            .unwrap_or("(none)"),
        task.front.verification.as_deref().unwrap_or("(none)"),
        task.front.status.as_str(),
        issue_state,
    )
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

fn select_create_repo(repositories: &[String], requested: Option<&str>) -> Result<String> {
    if let Some(repo) = requested {
        let repo = crate::github::GitHubConfig::normalize_repo(repo)?;
        return repositories
            .iter()
            .find(|candidate| candidate.eq_ignore_ascii_case(&repo))
            .map(|candidate| crate::github::GitHubConfig::normalize_repo(candidate))
            .unwrap_or(Ok(repo));
    }
    match repositories {
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
    let report = super::execution::synchronize(
        board,
        client.as_ref(),
        super::execution::SyncRequest {
            task: args.task.as_deref(),
            direction,
            resolutions: &args.resolutions,
            apply: args.apply,
        },
    )?;
    if report.tasks.is_empty() {
        return Ok(crate::output::CommandOutput::new(
            json!({ "tasks": [], "count": 0 }),
            "No linked Tasks to sync (nothing was written).",
        ));
    }
    let human = if args.apply {
        format!(
            "Applied GitHub sync for {}/{} task(s); {} failed and can be retried",
            report.success_count,
            report.tasks.len(),
            report.failures.len()
        )
    } else {
        format!(
            "GitHub sync preview (dry run; nothing was written)\n{}",
            report
                .tasks
                .iter()
                .map(|item| format_sync_preview(
                    &item.task,
                    &item.github,
                    &item.plan,
                    &item.relationships
                ))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let plans: Vec<_> = report.tasks.iter().map(|item| {
        let mut value = json!({ "task": item.task.id, "github": item.github, "plan": item.plan, "relationships": item.relationships });
        match &item.outcome {
            SyncOutcome::Preview => {}
            SyncOutcome::Applied => value["success"] = json!(true),
            SyncOutcome::Failed(failure) => {
                value["success"] = json!(false);
                value["failure"] = json!(failure);
            }
        }
        value
    }).collect();
    let mut output = crate::output::CommandOutput::new(
        json!({
            "tasks": plans, "count": plans.len(), "apply": args.apply,
            "success_count": report.success_count, "failure_count": report.failures.len(), "failures": report.failures,
        }),
        human,
    );
    output.warnings = report.warnings;
    Ok(output)
}

fn cmd_auto_push(board: &Board, args: GitHubAutoPushArgs) -> Result<crate::output::CommandOutput> {
    let enabled = matches!(args.state, AutoPushState::On);
    let previous = board.config.github.auto_push;
    let changed = previous != enabled;
    let mut warnings = Vec::new();
    if changed {
        let outcome = board.save_config(
            if enabled {
                "orbit: github auto-push on"
            } else {
                "orbit: github auto-push off"
            },
            |config| {
                config.github.auto_push = enabled;
                Ok(())
            },
        )?;
        warnings.extend(crate::mutation::commit_warning(outcome));
    }
    if changed && enabled {
        let repositories = crate::workspace::export_repositories(board)?
            .unwrap_or_else(|| board.config.github.repositories.clone());
        match repositories.len() {
            1 => {}
            0 => warnings.push(
                "new Tasks will stay local: auto-push needs one available GitHub repository to create an Issue"
                    .to_string(),
            ),
            count => warnings.push(format!(
                "new Tasks will stay local: {count} repositories are available, so Issue creation has no single destination; keep one included repository or pass `--repo OWNER/REPO` to `orbit github create`"
            )),
        }
    }
    let state = if enabled { "on" } else { "off" };
    let human = if changed {
        format!("Auto-push turned {state}.")
    } else {
        format!("Auto-push is already {state}.")
    };
    let mut output = crate::output::CommandOutput::new(
        json!({
            "auto_push": {
                "enabled": enabled,
                "previous": previous,
                "changed": changed,
                "reserved_label": crate::github::status::IN_PROGRESS_LABEL,
            }
        }),
        human,
    );
    output.warnings = warnings;
    Ok(output)
}

fn format_sync_preview(
    task: &Task,
    link: &GitHubLink,
    plan: &SyncPlan,
    relationships: &relationships::RelationshipReport,
) -> String {
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
    push_preview_field(&mut pushes, "title", &plan.push_title);
    push_preview_field(&mut pushes, "body", &plan.push_body);
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
                "  - {}: Orbit = {}; GitHub = {}; choose `--resolve {}=orbit` or `--resolve {}=github`\n",
                conflict.field,
                format_preview_value(&conflict.orbit_value),
                format_preview_value(&conflict.github_value),
                conflict.field,
                conflict.field,
            ));
        }
    }

    if plan.is_noop() && !plan.has_conflicts() && relationships.changes.is_empty() {
        output.push_str("No changes needed.\n");
    }
    for warning in &plan.warnings {
        output.push_str(&format!("Warning: {warning}\n"));
    }
    output.push_str("Relationships:\n");
    if relationships.changes.is_empty() {
        output.push_str("  (none)\n");
    } else {
        for change in &relationships.changes {
            output.push_str(&format!(
                "  - {} {} {}/#{}\n",
                change.action, change.kind, change.github.repo, change.github.number
            ));
        }
    }
    for unsupported in &relationships.unsupported {
        output.push_str(&format!("Unsupported relationship: {unsupported}\n"));
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

fn apply_issue_to_task(task: &mut Task, issue: &IssueSnapshot, include_fields: bool) -> Result<()> {
    task.front.title = issue.title.clone();
    task.body = issue.body.clone();
    task.front.labels = crate::github::status::user_labels(&issue.labels);
    if include_fields {
        task.front.area = issue.area.as_deref().map(parse_area).transpose()?;
        task.front.verification = issue.verification.clone();
    }
    task.front.status =
        crate::github::status::status_from_issue(issue.state, issue.state_reason, &issue.labels);
    set_baseline(task, issue);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::github_scope_status;

    #[test]
    fn organization_admin_scope_satisfies_read_org() {
        let scopes = vec!["admin:org".to_string(), "repo".to_string()];
        let (_, missing) = github_scope_status(false, &scopes);
        assert!(missing.is_empty(), "missing: {missing:?}");
    }

    #[test]
    fn organization_write_scope_satisfies_read_org() {
        let scopes = vec!["write:org".to_string(), "repo".to_string()];
        let (_, missing) = github_scope_status(false, &scopes);
        assert!(missing.is_empty(), "missing: {missing:?}");
    }
}
