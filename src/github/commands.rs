use crate::github::adapter::GitHubAdapter;
use crate::github::client::client_from_env;
use crate::github::config::GitHubConfig;
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
use crate::graph::{Graph, LoadMode};
use crate::remote::execution::{self, CreateFollowUp, SyncOutcome};
use crate::remote::link::{active_link, github_compat_json, link_json, link_view};
use crate::remote::model::{RemoteIssue, RemoteIssueRef, SyncDirection, SyncPlan};
use crate::remote::port::{RemoteAdapter, RemoteResolver, board_resolver};
use crate::remote::{RemoteProject, relationships, sync};
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
    let github = github_config(board);
    let configured = github.is_enabled();
    let basic_mode = github.mode.is_basic();
    let auto_push = auto_push_enabled(board);

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
                    if auto_push && label_present == Some(false) {
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
    let github_link = |task: &Task| active_link(task).filter(|link| link.tracker == "github");
    let adapter = GitHubAdapter::from_board(board)?;
    let resolver = board_resolver(board);
    for task in graph.iter().filter(|task| github_link(task).is_some()) {
        let link = github_link(task).expect("filtered linked task");
        let project = link.remote_project();
        let linked_repo = crate::remote::execution::project_available(board, &resolver, &project)?;
        if !linked_repo {
            issues.push(json!({
                "code": "GITHUB_REPO_NOT_LINKED",
                "task": task.id,
                "repository": link.project,
                "message": format!("Task {} links to {}, which is not linked to this Board", task.id, link.project),
            }));
        }

        let children: Vec<_> = graph
            .iter()
            .filter(|candidate| candidate.front.parent.as_deref() == Some(task.id.as_str()))
            .filter_map(|candidate| {
                github_link(candidate).map(|child_link| (candidate.clone(), child_link))
            })
            .collect();
        let dependencies: Vec<_> = task
            .front
            .depends_on
            .iter()
            .filter_map(|id| graph.tasks.get(id))
            .filter_map(|dependency| {
                github_link(dependency).map(|dep_link| (dependency.clone(), dep_link))
            })
            .collect();
        let relationships = relationships::plan_relationship_sync(
            &link,
            &relationships::RemoteRelationsView::EMPTY,
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
        match adapter.fetch_issue(&project, &link.id) {
            Ok(issue) => {
                let direction = if basic_mode {
                    SyncDirection::Pull
                } else {
                    SyncDirection::Both
                };
                let plan = sync::plan_sync(
                    &adapter,
                    sync::OrbitSyncView { task, link: &link },
                    &issue,
                    direction,
                );
                let pull_count = count_pull_changes(&plan);
                let push_count = count_push_changes(&plan);
                if pull_count > 0 {
                    pending_pull.push(json!({ "task": task.id, "repository": link.project, "number": link.id, "changes": pull_count }));
                }
                if push_count > 0 {
                    pending_push.push(json!({ "task": task.id, "repository": link.project, "number": link.id, "changes": push_count }));
                }
                if !plan.conflicts.is_empty() {
                    conflicts.push(json!({ "task": task.id, "repository": link.project, "number": link.id, "conflicts": plan.conflicts }));
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
            "enabled": auto_push,
            "reserved_label": crate::github::status::IN_PROGRESS_LABEL,
        },
        "linked_tasks": graph.iter().filter(|task| github_link(task).is_some()).count(),
        "local_tasks": graph.iter().filter(|task| github_link(task).is_none()).count(),
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
    let github = github_config(board);
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
    if auto_push_enabled(board) {
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
    let effective = github_config(board);
    let setup = match client.validate_org_fields(organization, &effective.issue_fields) {
        Ok(setup) => setup,
        Err(error) => {
            let message = error.message.clone();
            let missing = missing_field_names(&error);
            let area_missing = missing.contains(&effective.issue_fields.area_name.as_str());
            let verification_missing =
                missing.contains(&effective.issue_fields.verification_name.as_str());
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
                    "remediation": format!("Create the missing organization Issue Field(s) `{}` and `{}` for `{organization}`, then rerun `orbit github connect`. If they exist, run `gh auth refresh --hostname github.com --scopes read:org,repo` and retry.", effective.issue_fields.area_name, effective.issue_fields.verification_name),
                    "field_error": message,
                }),
                format!(
                    "Organization `{organization}` is missing a required Issue Field or the current account cannot read it. Create `{}` and `{}`, or refresh `gh` scopes with `gh auth refresh --hostname github.com --scopes read:org,repo`, then rerun `orbit github connect`.",
                    effective.issue_fields.area_name, effective.issue_fields.verification_name
                ),
            ));
        }
    };

    let changed = effective.organization.as_deref() != Some(organization)
        || !effective.contains_repo(&repo)
        || effective.issue_fields.area_id.as_deref() != Some(setup.area_id.as_str())
        || effective.issue_fields.verification_id.as_deref()
            != Some(setup.verification_id.as_str());
    if changed {
        save_github_settings(board, "orbit: github connect", |config| {
            config.organization = Some(organization.to_string());
            config.issue_fields.area_id = Some(setup.area_id.clone());
            config.issue_fields.verification_id = Some(setup.verification_id.clone());
            Ok(())
        })?;
        link_github_repository(board, &repo)?;
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
    if GitHubAdapter::effective_config(board).is_enabled() {
        Ok(())
    } else {
        Err(OrbitError::new(
            code::GITHUB_NOT_CONFIGURED,
            "github provider is not configured; add a [github] section to config.toml",
        ))
    }
}

/// Whether this Board keeps GitHub settings under `[remote]` (the shape
/// `orbit migrate` writes) rather than the legacy top-level `[github]`
/// section. Config writes must land where the adapter actually reads.
/// `[remote] auto_push` does not count: it is the provider-neutral flag and
/// can be set on a legacy board without migrating `[github]` settings.
fn github_uses_remote_config(board: &Board) -> bool {
    board.config.remote.github.is_some()
        || board
            .config
            .remote
            .projects
            .iter()
            .any(|project| project.tracker == "github")
}

/// Apply a mutation to the GitHub adapter settings in whichever section the
/// Board uses: `[remote.github]` post-migration, `[github]` otherwise.
fn save_github_settings(
    board: &Board,
    message: &str,
    mutate: impl FnOnce(&mut GitHubConfig) -> Result<()>,
) -> Result<()> {
    let remote = github_uses_remote_config(board);
    board.save_config(message, move |config| {
        if remote {
            mutate(
                config
                    .remote
                    .github
                    .get_or_insert_with(GitHubConfig::default),
            )?;
        } else {
            mutate(&mut config.github)?;
        }
        Ok(())
    })?;
    Ok(())
}

/// The effective `[github]` settings: `[remote.github]` wins over the
/// legacy section, and `[remote] projects` on the github tracker count as
/// linked repositories.
fn github_config(board: &Board) -> GitHubConfig {
    GitHubAdapter::effective_config(board)
}

/// Whether Auto-push is enabled for this Board, whichever section carries
/// the flag.
fn auto_push_enabled(board: &Board) -> bool {
    crate::remote::auto_push::enabled(board)
}

/// Link a GitHub Remote Project (`owner/repo`) to the Board in whichever
/// config shape the Board uses: `[remote] projects` post-migration,
/// `[github] repositories` otherwise.
fn link_github_repository(board: &Board, repository: &str) -> Result<()> {
    let repository = GitHubConfig::normalize_repo(repository)?;
    let remote = github_uses_remote_config(board);
    board.save_config("orbit: github repo link", move |config| {
        if remote {
            let project = RemoteProject {
                tracker: "github".to_string(),
                project: repository.clone(),
            };
            if !config.remote.projects.contains(&project) {
                config.remote.projects.push(project);
            }
            if let Some(github) = config.remote.github.as_mut() {
                github
                    .repositories
                    .retain(|repo| !repo.eq_ignore_ascii_case(&repository));
            }
            config
                .github
                .repositories
                .retain(|repo| !repo.eq_ignore_ascii_case(&repository));
        } else {
            config.github.link_repo(&repository)?;
        }
        Ok(())
    })?;
    Ok(())
}

/// Remove a GitHub Remote Project link from every config shape that could
/// carry it. Returns whether anything was removed.
fn unlink_github_repository(board: &Board, repository: &str) -> Result<bool> {
    let repository = GitHubConfig::normalize_repo(repository)?;
    let remote = github_uses_remote_config(board);
    // Decide up-front so a missing repo reports not-found without a commit.
    let was_linked = if remote {
        board.config.remote.projects.iter().any(|project| {
            project.tracker == "github" && project.project.eq_ignore_ascii_case(&repository)
        }) || board
            .config
            .remote
            .github
            .as_ref()
            .is_some_and(|github| github.contains_repo(&repository))
            || board.config.github.contains_repo(&repository)
    } else {
        board.config.github.contains_repo(&repository)
    };
    if !was_linked {
        return Ok(false);
    }
    board.save_config("orbit: github repo unlink", move |config| {
        if remote {
            config.remote.projects.retain(|project| {
                !(project.tracker == "github" && project.project.eq_ignore_ascii_case(&repository))
            });
            if let Some(github) = config.remote.github.as_mut() {
                github.unlink_repo(&repository)?;
            }
            config.github.unlink_repo(&repository)?;
        } else {
            config.github.unlink_repo(&repository)?;
        }
        Ok(())
    })?;
    Ok(true)
}

/// The GitHub-shaped JSON view of a Remote Issue for `orbit github`
/// command output: `state`/`state_reason`/`number` keep the historical
/// IssueSnapshot shape while the engine works in Orbit terms.
fn github_issue_json(issue: &RemoteIssue) -> Value {
    let (state, state_reason) = match issue.status {
        task::Status::Done => ("closed", "completed"),
        task::Status::Cancelled => ("closed", "not_planned"),
        _ => ("open", ""),
    };
    let refs = |refs: &[RemoteIssueRef]| {
        refs.iter()
            .map(|issue| {
                json!({
                    "repo": issue.project,
                    "number": issue.id.parse::<u64>().unwrap_or(0),
                })
            })
            .collect::<Vec<_>>()
    };
    json!({
        "repo": issue.project,
        "number": issue.id.parse::<u64>().unwrap_or(0),
        "title": issue.title,
        "body": issue.body,
        "state": state,
        "state_reason": if state_reason.is_empty() { Value::Null } else { json!(state_reason) },
        "labels": issue.labels.iter().chain(issue.markers.iter()).cloned().collect::<Vec<_>>(),
        "assignees": issue.actors,
        "updated_at": issue.updated_at,
        "area": issue.area,
        "verification": issue.verification,
        "url": issue.url,
        "parent_issue": refs(&issue.relations.parents),
        "blocked_by": refs(&issue.relations.blocked_by),
        "blocking": refs(&issue.relations.blocking),
    })
}

fn cmd_setup(board: &Board, args: GitHubSetupArgs) -> Result<crate::output::CommandOutput> {
    let effective = github_config(board);
    let org = effective.require_organization()?;
    let client = client_from_env()?;
    let setup = if args.create_fields {
        let area_options = super::client::area_field_options(&board.config.board.areas);
        client.create_org_fields(org, &effective.issue_fields, &area_options)?
    } else {
        client.validate_org_fields(org, &effective.issue_fields)?
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
    save_github_settings(board, "orbit: github setup", |config| {
        config.issue_fields.area_id = Some(setup.area_id);
        config.issue_fields.verification_id = Some(setup.verification_id);
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
    for repo in &github_config(board).repositories {
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
            let repos = github_config(board).repositories;
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
    link_github_repository(board, &repo)?;
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
    if !unlink_github_repository(board, &repo)? {
        return Err(OrbitError::not_found(&repo));
    }
    Ok(crate::output::CommandOutput::new(
        json!({ "repository": repo, "linked": false }),
        format!("Unlinked `{repo}`"),
    ))
}

fn cmd_adopt(board: &Board, args: GitHubAdoptArgs) -> Result<crate::output::CommandOutput> {
    let (repo, number) = resolve_adopt_identifier(&args)?;
    let project = RemoteProject {
        tracker: "github".to_string(),
        project: repo.clone(),
    };
    let resolver = board_resolver(board);
    crate::remote::execution::require_project_available(board, &resolver, &project)?;
    let adapter = resolver.adapter("github")?;
    let issue = adapter.fetch_issue(&project, &number.to_string())?;
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let task = graph.get(&args.task)?.clone();
    if let Some(link) = link_view(&task).filter(|link| link.tracker == "github" && !link.active) {
        return Err(OrbitError::invalid_argument(format!(
            "task {} retains inactive GitHub provenance from {}#{}; review it before adopting another Issue",
            args.task, link.project, link.id,
        )));
    }
    if let Some(link) = active_link(&task) {
        return Err(OrbitError::invalid_argument(format!(
            "task {} already links to {}#{}",
            args.task, link.project, link.id
        )));
    }
    if let Some(existing) = graph.iter().find(|candidate| {
        active_link(candidate).is_some_and(|link| {
            link.tracker == "github"
                && link.project.eq_ignore_ascii_case(&repo)
                && link.id == number.to_string()
        })
    }) {
        return Err(OrbitError::invalid_argument(format!(
            "GitHub Issue {repo}#{number} is already linked to Task {}; one GitHub Issue can be paired with only one Orbit Task",
            existing.id
        )));
    }
    let mut task = task;
    task.front.remote = Some(crate::remote::RemoteLink {
        tracker: "github".to_string(),
        project: repo.clone(),
        id: number.to_string(),
        url: issue.url.clone(),
        active: true,
        baseline: None,
    });
    task.front.updated_at = Utc::now();
    crate::remote::execution::set_baseline(&mut task, &issue);
    apply_issue_to_task(&mut task, &issue, !github_config(board).mode.is_basic())?;
    sync::validate_orbit_metadata(&task)?;
    let outcome = persist_task(board, &task, &format!("orbit: github adopt {}", task.id))?;
    let human = format!("Adopted {repo}/#{number} for {}", task.id);
    let mut output = crate::output::CommandOutput::new(
        json!({
            "task": task.id,
            "github": github_compat_json(&task),
            "remote": link_json(&task),
            "issue": github_issue_json(&issue),
        }),
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
    let mut repositories = workspace_repositories.clone().unwrap_or_default();
    for repository in &github_config(board).repositories {
        if !repositories
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(repository))
        {
            repositories.push(repository.clone());
        }
    }
    if repositories.is_empty() && args.repo.is_none() {
        return Err(OrbitError::invalid_argument(
            "no GitHub repositories are linked or included; run `orbit github repo link OWNER/REPO` or pass `--repo OWNER/REPO` before exporting",
        ));
    }
    let repo = select_create_repo(&repositories, args.repo.as_deref())?;
    let create_project = RemoteProject {
        tracker: "github".to_string(),
        project: repo.clone(),
    };
    crate::remote::execution::require_project_available(
        board,
        &board_resolver(board),
        &create_project,
    )?;

    let mut plans = Vec::new();
    let mut selected = std::collections::HashSet::new();
    for id in &args.tasks {
        let task = graph.get(id)?;
        let rejected = if !selected.insert(id) {
            Some(format!(
                "task {id} appears more than once in the export selection"
            ))
        } else if link_view(task).is_some_and(|link| !link.active) {
            Some(format!(
                "task {id} retains inactive Remote Issue provenance; reattach the exact Issue before exporting"
            ))
        } else if active_link(task).is_some() {
            Some(format!(
                "task {id} already links to a Remote Issue; use `orbit github sync {id}` instead"
            ))
        } else if task.front.status.is_terminal() && !args.include_closed {
            Some(format!(
                "task {id} is {}; pass --include-closed to export done and cancelled Tasks",
                task.front.status.as_str()
            ))
        } else if github_config(board).mode.is_basic()
            && (task.front.area.is_some() || task.front.verification.is_some())
        {
            Some(basic_mode_metadata_error(task))
        } else {
            None
        };
        plans.push((task.clone(), rejected));
    }

    if args.apply && plans.iter().any(|(_, error)| error.is_none()) {
        // Fail fast on a missing `gh` before per-Task work begins; the
        // adapter resolves its own client inside `create_and_link`.
        client_from_env()?;
    }
    let resolver = board_resolver(board);
    let project = create_project;
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
                "area": task.front.area.as_deref(),
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
            "area": task.front.area.as_deref(),
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

        let created = match crate::remote::execution::create_and_link(
            board,
            &resolver,
            &project,
            &task,
            &format!("orbit: github create {}", task.id),
        ) {
            Ok(created) => created,
            Err(error) => {
                results.push(json!({
                    "task": task.id,
                    "repository": repo,
                    "title": task.front.title,
                    "body": task.body,
                    "labels": task.front.labels,
                    "area": task.front.area.as_deref(),
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
        task = created.task;
        let final_issue = created.issue;
        let mut commit_warnings = created.warnings;
        let mut failure = None;
        for follow_up in created.failures {
            match follow_up {
                CreateFollowUp::CatalogUpdateFailed(error) => {
                    let warning = format!(
                        "{}: created {}/#{} but could not update the Remote Issue cache: {}; run `orbit workspace issues refresh`",
                        task.id, repo, final_issue.id, error.message
                    );
                    if !warnings.contains(&warning) {
                        warnings.push(warning);
                    }
                }
                // A link failure outranks a close failure: it decides the recovery step.
                other if !matches!(failure, Some(CreateFollowUp::LinkNotPersisted(_))) => {
                    failure = Some(other)
                }
                _ => {}
            }
        }
        if let Some(follow_up) = failure {
            warnings.append(&mut commit_warnings);
            let (linked, error, next_action, summary) = match follow_up {
                CreateFollowUp::LinkNotPersisted(error) => (
                    false,
                    error,
                    format!(
                        "run `orbit github adopt {} https://github.com/{repo}/issues/{}` to link the created Issue",
                        task.id, final_issue.id
                    ),
                    "created, but could not link the Task",
                ),
                CreateFollowUp::CloseFailed(error) => (
                    true,
                    error,
                    format!(
                        "run `orbit github sync {} --apply` to close the Issue",
                        task.id
                    ),
                    "created and linked, but could not close Issue",
                ),
                CreateFollowUp::CatalogUpdateFailed(_) => unreachable!("handled above"),
            };
            results.push(json!({
                "task": task.id,
                "repository": repo,
                "github": if linked { github_compat_json(&task) } else { Value::Null },
                "remote": if linked { link_json(&task) } else { Value::Null },
                "issue": github_issue_json(&final_issue),
                "success": false,
                "created": true,
                "linked": linked,
                "error": error.message,
                "error_code": error.code,
                "next_action": next_action,
            }));
            human.push(format!(
                "{}\n  Result: {summary} — {}",
                format_create_preview(&task, &repo, issue_state),
                error.message
            ));
            continue;
        }
        warnings.append(&mut commit_warnings);
        results.push(json!({
            "task": task.id,
            "repository": repo,
            "title": task.front.title,
            "body": task.body,
            "labels": task.front.labels,
            "area": task.front.area.as_deref(),
            "verification": task.front.verification,
            "status": task.front.status,
            "issue_state": issue_state,
            "github": github_compat_json(&task),
            "remote": link_json(&task),
            "issue": github_issue_json(&final_issue),
            "success": true,
        }));
        human.push(format!(
            "{}\n  Result: created {}/#{}",
            format_create_preview(&task, &repo, issue_state),
            repo,
            final_issue.id
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

/// The rejection message for Orbit metadata a basic-mode GitHub adapter
/// cannot publish (no organization Issue Fields).
fn basic_mode_metadata_error(task: &Task) -> String {
    let present: Vec<&str> = [
        ("area", task.front.area.is_some()),
        ("verification", task.front.verification.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, present)| present.then_some(name))
    .collect();
    format!(
        "GitHub basic mode cannot sync task {} field(s): {}; remove them or use mode = `full` with organization Issue Fields",
        task.id,
        present.join(", ")
    )
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
        task.front.area.as_deref().unwrap_or("(none)"),
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
    // `--resolve FIELD=github` keeps working as the GitHub spelling of the
    // provider-neutral `FIELD=remote` choice.
    let resolutions: Vec<String> = args
        .resolutions
        .iter()
        .map(|resolution| match resolution.split_once('=') {
            Some((field, "github")) => format!("{field}=remote"),
            _ => resolution.clone(),
        })
        .collect();
    client_from_env()?;
    let resolver = board_resolver(board);
    let report = execution::synchronize(
        board,
        &resolver,
        execution::SyncRequest {
            task: args.task.as_deref(),
            direction,
            resolutions: &resolutions,
            apply: args.apply,
            tracker: Some("github"),
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
                .map(|item| {
                    format_sync_preview(&item.task, &item.link, &item.plan, &item.relationships)
                })
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let plans: Vec<_> = report
        .tasks
        .iter()
        .map(|item| {
            let mut value = json!({
                "task": item.task.id,
                "github": github_compat_json(&item.task),
                "remote": link_json(&item.task),
                "plan": item.plan,
                "relationships": item.relationships,
            });
            match &item.outcome {
                SyncOutcome::Preview => {}
                SyncOutcome::Applied => value["success"] = json!(true),
                SyncOutcome::Failed(failure) => {
                    value["success"] = json!(false);
                    value["failure"] = json!(failure);
                }
            }
            value
        })
        .collect();
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
    let previous = auto_push_enabled(board);
    let changed = previous != enabled;
    let mut warnings = Vec::new();
    if changed {
        // Auto-push is provider-neutral: the flag lives in `[remote]` and
        // covers every configured Remote Tracker.
        let outcome = board.save_config(
            if enabled {
                "orbit: github auto-push on"
            } else {
                "orbit: github auto-push off"
            },
            |config| {
                config.remote.auto_push = enabled;
                Ok(())
            },
        )?;
        warnings.extend(crate::mutation::commit_warning(outcome));
    }
    if changed && enabled {
        let mut repositories = crate::workspace::export_repositories(board)?.unwrap_or_default();
        for repository in &github_config(board).repositories {
            if !repositories
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(repository))
            {
                repositories.push(repository.clone());
            }
        }
        match repositories.len() {
            1 => {}
            0 => warnings.push(
                "new Tasks will stay local: auto-push needs one available GitHub repository to create an Issue"
                    .to_string(),
            ),
            count => warnings.push(format!(
                "new Tasks will stay local: {count} repositories are available, so Issue creation has no single destination; keep one linked repository or pass `--repo OWNER/REPO` to `orbit github create`"
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
    link: &crate::remote::link::RemoteLinkView,
    plan: &SyncPlan,
    relationships: &relationships::RelationshipReport,
) -> String {
    let mut output = format!(
        "\nTask: {} — {}\nIssue: {}#{}\n",
        task.id, task.front.title, link.project, link.id
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
    push_preview_field(&mut pushes, "status", &plan.push_status);
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
                format_preview_value(&conflict.remote_value),
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
                "  - {} {} {}#{}\n",
                change.action.as_str(),
                change.kind.as_str(),
                change.issue.project,
                change.issue.id
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

fn apply_issue_to_task(task: &mut Task, issue: &RemoteIssue, include_fields: bool) -> Result<()> {
    task.front.title = issue.title.clone();
    task.body = issue.body.clone();
    // `RemoteIssue.labels` already excludes adapter-managed markers.
    task.front.labels = issue.labels.clone();
    if include_fields {
        task.front.area = match &issue.area {
            Some(area) => {
                let normalized = area.trim().to_ascii_lowercase();
                if !task::is_valid_area(&normalized) {
                    return Err(OrbitError::invalid_argument(
                        "area must be a short lowercase slug of letters, digits, and `-`",
                    ));
                }
                Some(normalized)
            }
            None => None,
        };
        task.front.verification = issue.verification.clone();
    }
    // The adapter already mapped the Remote Issue onto Orbit's Status.
    task.front.status = issue.status;
    execution::set_baseline(task, issue);
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
