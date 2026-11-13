use crate::github::adapter::GitHubAdapter;
use crate::github::client::client_from_env;
use crate::github::config::GitHubConfig;
use crate::github::diagnosis::{self, DiagnosisOptions, NextActionInput, StatusIssue};
use crate::mutation::persist_task;
use chrono::Utc;
use serde_json::{Value, json};

use crate::board::Board;
use crate::cli::{
    AutoPushState, RemoteArgs, RemoteAutoPushArgs, RemoteCommand, RemoteConnectArgs,
    RemoteCreateArgs, RemoteImportArgs, RemoteIssueFilters, RemoteIssueRefArgs, RemoteIssuesArgs,
    RemoteIssuesCommand, RemoteLinkArgs, RemoteMutationArgs, RemoteProjectArgs, RemoteStatusArgs,
    RemoteSyncArgs, RemoteSyncDirection,
};
use crate::error::{OrbitError, Result, code};
use crate::graph::{Graph, LoadMode};
use crate::remote::catalog::{self, RemoteIssueCatalog};
use crate::remote::execution::{self, CreateFollowUp, SyncOutcome};
use crate::remote::link::{active_link, github_compat_json, link_json, link_view};
use crate::remote::model::{
    RemoteIssue, RemoteIssueRef, RemoteIssueSummary, SyncDirection, SyncPlan,
};
use crate::remote::port::{BoardResolver, RemoteAdapter, RemoteResolver};
use crate::remote::{
    RemoteProject, configured_projects, parse_issue_ref, parse_remote_project, relationships, sync,
};
use crate::task::{self, Status, Task};

pub fn run(board: &Board, args: RemoteArgs) -> Result<crate::output::CommandOutput> {
    match args.command {
        RemoteCommand::Status(args) => cmd_status(board, args),
        RemoteCommand::Connect(args) => cmd_connect(board, args),
        RemoteCommand::Disconnect(args) => cmd_disconnect(board, args),
        RemoteCommand::List => cmd_list(board),
        RemoteCommand::Issues(args) => cmd_issues(board, args),
        RemoteCommand::Import(args) => cmd_import(board, args),
        RemoteCommand::Link(args) => cmd_link(board, args),
        RemoteCommand::Create(args) => cmd_create(board, args),
        RemoteCommand::Detach(args) => cmd_detach(board, args),
        RemoteCommand::Reattach(args) => cmd_reattach(board, args),
        RemoteCommand::Sync(args) => cmd_sync(board, args),
        RemoteCommand::AutoPush(args) => cmd_auto_push(board, args),
    }
}

/// The command edge's shared-client resolver: one `GitHubClient` serves
/// every adapter the operation resolves.
fn github_resolver(board: &Board) -> Result<BoardResolver<'_>> {
    Ok(BoardResolver::with_client(board, client_from_env()?))
}

fn cmd_status(board: &Board, args: RemoteStatusArgs) -> Result<crate::output::CommandOutput> {
    let client = client_from_env()?;
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let github = github_config(board);
    let repo_scope = args
        .repo
        .as_deref()
        .map(GitHubConfig::normalize_repo)
        .transpose()?;
    // The shared health diagnosis (`src/github/diagnosis.rs`) owns every
    // configuration/auth/repository/field rule; this command projects it
    // into the status envelope and adds its own per-task sync preview.
    let diagnosis = diagnosis::diagnose(
        board,
        client.as_ref(),
        &github,
        &DiagnosisOptions {
            repository_scope: repo_scope.clone(),
            strict_connected_projects: true,
        },
    )?;
    let configured = diagnosis.configured;
    let basic_mode = diagnosis.basic_mode;
    let auto_push = diagnosis.auto_push;

    let mut issues = diagnosis.status_issues();

    let github_link = |task: &Task| active_link(task).filter(|link| link.tracker == "github");
    let mut pending_pull = Vec::new();
    let mut pending_push = Vec::new();
    let mut conflicts = Vec::new();
    let mut unsupported = Vec::new();
    let adapter =
        GitHubAdapter::with_client(client.clone(), GitHubAdapter::effective_config(board));
    let targets: Vec<_> = graph
        .iter()
        .filter(|task| github_link(task).is_some())
        .map(|task| {
            (
                task.clone(),
                github_link(task).expect("filtered linked task"),
            )
        })
        .filter(|(_, link)| {
            repo_scope
                .as_deref()
                .is_none_or(|scope| link.project.eq_ignore_ascii_case(scope))
        })
        .collect();
    // Validate availability and relationships locally, then fetch every
    // linked Issue in one batched pass instead of one API call per Task.
    let mut issue_results: Vec<Option<Result<RemoteIssue>>> =
        targets.iter().map(|_| None).collect();
    let mut fetch: Vec<(usize, RemoteIssueRef)> = Vec::new();
    for (index, (task, link)) in targets.iter().enumerate() {
        // Availability is membership in the diagnosis's connected-projects
        // set: resolving it once avoids a member-origin `git remote` scan
        // per linked Task. Every target links the github tracker.
        let linked_repo = diagnosis
            .connected_projects
            .iter()
            .any(|name| name.eq_ignore_ascii_case(link.project.trim()));
        if !linked_repo {
            issues.push(
                StatusIssue::new(
                    code::GITHUB_REPO_NOT_LINKED,
                    format!(
                        "Task {} links to {}, which is not linked to this Board",
                        task.id, link.project
                    ),
                )
                .with_task(task.id.as_str())
                .with_repository(link.project.as_str()),
            );
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
            link,
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

        if !diagnosis.auth.authenticated || !linked_repo {
            continue;
        }
        fetch.push((
            index,
            RemoteIssueRef {
                project: link.project.clone(),
                id: link.id.clone(),
            },
        ));
    }
    let (fetch_positions, fetch_refs): (Vec<usize>, Vec<RemoteIssueRef>) =
        fetch.into_iter().unzip();
    for (position, result) in fetch_positions
        .into_iter()
        .zip(adapter.fetch_issues(&fetch_refs))
    {
        issue_results[position] = Some(result);
    }
    for (index, (task, link)) in targets.iter().enumerate() {
        let Some(result) = issue_results[index].take() else {
            continue;
        };
        match result {
            Ok(issue) => {
                let direction = if basic_mode {
                    SyncDirection::Pull
                } else {
                    SyncDirection::Both
                };
                let plan = sync::plan_sync(
                    &adapter,
                    sync::OrbitSyncView { task, link },
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
            Err(error) => {
                issues.push(StatusIssue::new(error.code, error.message).with_task(task.id.as_str()))
            }
        }
    }

    let fields = json!({
        "area": {
            "name": github.issue_fields.area_name,
            "applicable": !basic_mode,
            "configured": if basic_mode { Value::Null } else { json!(diagnosis.fields.area.configured) },
            "resolved": diagnosis.fields.area.resolved,
            "matches": diagnosis.fields.area.matches,
        },
        "verification": {
            "name": github.issue_fields.verification_name,
            "applicable": !basic_mode,
            "configured": if basic_mode { Value::Null } else { json!(diagnosis.fields.verification.configured) },
            "resolved": diagnosis.fields.verification.resolved,
            "matches": diagnosis.fields.verification.matches,
        },
    });
    let first_repo_not_linked = issues
        .iter()
        .find(|issue| issue.code == code::GITHUB_REPO_NOT_LINKED);
    let next = diagnosis::next_action(&NextActionInput {
        authenticated: diagnosis.auth.authenticated,
        missing_scopes: &diagnosis.auth.missing_scopes,
        configured,
        basic_mode,
        organization_configured: github.organization.is_some(),
        repositories_configured: !github.repositories.is_empty(),
        field_validation_failed: diagnosis.fields.validation_failed,
        fields: &diagnosis.fields,
        unsupported_relationships: unsupported.len(),
        has_conflicts: !conflicts.is_empty(),
        first_conflict_task: conflicts
            .first()
            .and_then(|conflict| conflict["task"].as_str()),
        pending_changes: !pending_pull.is_empty() || !pending_push.is_empty(),
        repo_not_linked: first_repo_not_linked.is_some(),
        repo_not_linked_repository: first_repo_not_linked
            .and_then(|issue| issue.repository.as_deref()),
        repository_issue: issues.iter().find_map(|issue| issue.repository.as_deref()),
    });
    let next_action = next.action();
    let next_command = next.command();
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
            "authenticated": diagnosis.auth.authenticated,
            "username": diagnosis.auth.username,
            "scopes": diagnosis.auth.scopes,
            "scopes_known": diagnosis.auth.scopes_known,
            "required_scopes": diagnosis.auth.required_scopes,
            "missing_scopes": diagnosis.auth.missing_scopes,
            "message": diagnosis.auth.message,
        },
        "organization": github.organization,
        "repositories": diagnosis
            .repositories
            .iter()
            .map(|repository| {
                json!({
                    "name": repository.name,
                    "accessible": repository.accessible,
                    "owner_type": repository.owner_type,
                    "reserved_label_present": if repository.accessible == Some(true) {
                        repository.label.as_ref().and_then(|label| label.present)
                    } else {
                        None
                    },
                })
            })
            .collect::<Vec<_>>(),
        "fields": fields,
        "auto_push": {
            "enabled": auto_push,
            "reserved_label": crate::github::status::IN_PROGRESS_LABEL,
        },
        "repository_scope": repo_scope,
        "linked_tasks": targets.len(),
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
        "issues": issues.iter().map(StatusIssue::to_json).collect::<Vec<_>>(),
        "next_action": next_action,
        "next_command": next_command,
    });
    let human = render_status(&data);
    Ok(crate::output::CommandOutput::new(data, human))
}

/// `orbit doctor`'s GitHub health projection: the shared
/// [`diagnosis::Diagnosis`] rendered as doctor issues over the connected
/// GitHub projects universe — configured Remote Projects plus member
/// Repository origins, the same set `remote status` reports. A Board with
/// no GitHub configuration reports nothing; a client or diagnosis failure
/// degrades to a single issue entry rather than aborting doctor.
pub(crate) fn doctor_issues(board: &Board) -> Vec<crate::graph::Issue> {
    let github = github_config(board);
    if !github.is_enabled() {
        return Vec::new();
    }
    let client = match client_from_env() {
        Ok(client) => client,
        Err(error) => {
            return vec![crate::graph::Issue::new(error.code, None, error.message)];
        }
    };
    match diagnosis::diagnose(
        board,
        client.as_ref(),
        &github,
        &DiagnosisOptions {
            repository_scope: None,
            strict_connected_projects: false,
        },
    ) {
        Ok(diagnosis) => diagnosis.doctor_issues(),
        Err(error) => vec![crate::graph::Issue::new(error.code, None, error.message)],
    }
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

fn cmd_connect(board: &Board, args: RemoteConnectArgs) -> Result<crate::output::CommandOutput> {
    if args.fields {
        return cmd_connect_fields(board, args.validate_only, args.create_fields);
    }
    if let Some(project) = args.project.as_deref() {
        return cmd_connect_project(board, project, args.create_fields);
    }
    connect_origin(board, args.validate_only, args.create_fields)
}

/// `orbit remote connect TRACKER:PROJECT` — link an explicit Remote Project
/// to the Board (the successor of `orbit github repo link`).
fn cmd_connect_project(
    board: &Board,
    project: &str,
    create_fields: bool,
) -> Result<crate::output::CommandOutput> {
    let (tracker, project_id) = parse_remote_project(project)?;
    let remote_project = RemoteProject {
        tracker: tracker.clone(),
        project: project_id.clone(),
    };
    remote_project.validate()?;
    link_remote_project(board, &remote_project)?;
    let mut output = crate::output::CommandOutput::new(
        json!({
            "status": "connected",
            "tracker": tracker,
            "project": project_id,
            "changed": true,
        }),
        format!("Connected Remote Project `{tracker}:{project_id}`"),
    );
    if tracker == "github" {
        if let Ok(client) = client_from_env()
            && let Err(error) =
                client.ensure_label(&project_id, crate::github::status::IN_PROGRESS_LABEL)
        {
            output.warnings.push(format!(
                "could not ensure the `{}` label on `{project_id}`: {}; auto-push will keep pushing other fields",
                crate::github::status::IN_PROGRESS_LABEL,
                error.message
            ));
        }
        if create_fields {
            // Creating missing organization Issue Fields stays with the
            // fields flow; point the user at it instead of guessing scope.
            output.warnings.push(
                "pass `--fields --create-fields` to create missing organization Issue Fields"
                    .to_string(),
            );
        }
    }
    Ok(output)
}

/// `orbit remote connect` with no project — detect the `origin` remote and
/// connect its Remote Tracker (the successor of `orbit github connect`).
fn connect_origin(
    board: &Board,
    validate_only: bool,
    create_fields: bool,
) -> Result<crate::output::CommandOutput> {
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
                "remediation": "Install GitHub CLI if needed, then run `gh auth login` and rerun `orbit remote connect`.",
                "auth_message": auth.message,
            }),
            "GitHub CLI authentication is missing or insufficient. Install `gh` if needed, run `gh auth login`, then rerun `orbit remote connect`.".to_string(),
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
                    "remediation": format!("Confirm `{repo}` exists and that the account from `gh auth status` can access it, then rerun `orbit remote connect`."),
                    "access_error": error.message,
                }),
                format!(
                    "Cannot access `{repo}` with the current GitHub account. Confirm the repository exists and grant that account access, then rerun `orbit remote connect`."
                ),
            ));
        }
    };

    if repository.owner_type.eq_ignore_ascii_case("user") {
        let remediation = format!(
            "To use `{repo}` in basic mode, edit the Board's `config.toml` and add or merge these settings (preserve any other linked repositories):\n\n[github]\nmode = \"basic\"\nrepositories = [\"{repo}\"]\n\nThen run `orbit remote status`. See `docs/github-sync.md#basic-mode-personal-repositories`."
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
        Err(_) if create_fields => {
            let area_options = crate::github::client::area_field_options(&board.config.board.areas);
            match client.create_org_fields(organization, &effective.issue_fields, &area_options) {
                Ok(setup) => setup,
                Err(create_error) => {
                    return Ok(connect_blocked(
                        json!({
                            "repository": repo,
                            "owner_type": "organization",
                            "organization": organization,
                            "field_status": {"area": "unknown", "verification": "unknown"},
                            "next_action": "create_issue_fields",
                            "remediation": format!("Creating organization Issue Field(s) failed: {}. Create `{}` and `{}` for `{organization}` as an organization administrator, or rerun `orbit remote connect --fields --create-fields`.", create_error.message, effective.issue_fields.area_name, effective.issue_fields.verification_name),
                            "field_error": create_error.message,
                        }),
                        format!(
                            "Could not create organization Issue Fields for `{organization}`: {}",
                            create_error.message
                        ),
                    ));
                }
            }
        }
        Err(error) => {
            let message = error.message.clone();
            let missing = diagnosis::missing_field_names(&error);
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
                    "remediation": format!("Create the missing organization Issue Field(s) `{}` and `{}` for `{organization}`, then rerun `orbit remote connect`. If they exist, run `gh auth refresh --hostname github.com --scopes read:org,repo` and retry.", effective.issue_fields.area_name, effective.issue_fields.verification_name),
                    "field_error": message,
                }),
                format!(
                    "Organization `{organization}` is missing a required Issue Field or the current account cannot read it. Create `{}` and `{}`, or refresh `gh` scopes with `gh auth refresh --hostname github.com --scopes read:org,repo`, then rerun `orbit remote connect`.",
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
    if validate_only {
        return Ok(crate::output::CommandOutput::new(
            json!({
                "status": "validated",
                "repository": repo,
                "owner_type": "organization",
                "organization": organization,
                "field_status": {"area": "ready", "verification": "ready"},
                "changed": changed,
            }),
            format!(
                "Validated `{repo}` for the Board (organization `{organization}`; Area and Verification fields ready; nothing was written)."
            ),
        ));
    }
    if changed {
        save_github_settings(board, "orbit: github connect", |config| {
            config.organization = Some(organization.to_string());
            config.issue_fields.area_id = Some(setup.area_id.clone());
            config.issue_fields.verification_id = Some(setup.verification_id.clone());
            Ok(())
        })?;
        link_remote_project(
            board,
            &RemoteProject {
                tracker: "github".to_string(),
                project: repo.clone(),
            },
        )?;
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
    crate::github::GitHubConfig::repo_of_origin(remote).ok_or_else(|| {
        OrbitError::new(
            code::GITHUB_NOT_CONFIGURED,
            format!(
                "origin `{remote}` is not a supported github.com remote; use HTTPS or SSH and rerun `orbit remote connect`"
            ),
        )
    })
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

/// The GitHub-shaped JSON view of a Remote Issue for command
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

/// `orbit remote connect --fields` — resolve (and optionally create) the
/// connected Remote Tracker's mapped fields and store their ids. For the
/// GitHub adapter these are the organization Issue Fields.
fn cmd_connect_fields(
    board: &Board,
    validate_only: bool,
    create_fields: bool,
) -> Result<crate::output::CommandOutput> {
    let effective = github_config(board);
    let org = effective.require_organization()?;
    let client = client_from_env()?;
    let setup = if create_fields {
        let area_options = crate::github::client::area_field_options(&board.config.board.areas);
        client.create_org_fields(org, &effective.issue_fields, &area_options)?
    } else {
        client.validate_org_fields(org, &effective.issue_fields)?
    };
    if validate_only {
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
    save_github_settings(board, "orbit: remote connect --fields", |config| {
        config.issue_fields.area_id = Some(setup.area_id);
        config.issue_fields.verification_id = Some(setup.verification_id);
        Ok(())
    })?;
    let mut data = json!({
        "organization": org,
        "area_id": area_id,
        "verification_id": verification_id,
    });
    if create_fields {
        data["created_fields"] = json!(setup.created_fields);
    }
    let mut output = crate::output::CommandOutput::new(
        data,
        if create_fields {
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
    for project in configured_projects(board) {
        if project.tracker != "github" {
            continue;
        }
        if let Err(error) =
            client.ensure_label(&project.project, crate::github::status::IN_PROGRESS_LABEL)
        {
            output.warnings.push(format!(
                "could not ensure the `{}` label on `{}`: {}; auto-push will keep pushing other fields",
                crate::github::status::IN_PROGRESS_LABEL,
                project.project,
                error.message
            ));
        }
    }
    Ok(output)
}

/// `orbit remote list` — every Remote Project connected to this Board.
fn cmd_list(board: &Board) -> Result<crate::output::CommandOutput> {
    let projects = configured_projects(board);
    let values: Vec<Value> = projects
        .iter()
        .map(|project| json!({ "tracker": project.tracker, "project": project.project }))
        .collect();
    Ok(crate::output::CommandOutput::new(
        json!({ "projects": values, "count": values.len() }),
        if values.is_empty() {
            "No Remote Projects are connected to this Board".to_string()
        } else {
            format!(
                "Connected Remote Projects: {}",
                projects
                    .iter()
                    .map(|project| format!("{}:{}", project.tracker, project.project))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
    ))
}

/// `orbit remote disconnect TRACKER:PROJECT` — remove a Remote Project
/// from this Board's configuration (the successor of
/// `orbit github repo unlink`). The tracker itself is never changed.
fn cmd_disconnect(board: &Board, args: RemoteProjectArgs) -> Result<crate::output::CommandOutput> {
    let (tracker, project) = parse_remote_project(&args.project)?;
    if !disconnect_remote_project(board, &tracker, &project)? {
        return Err(OrbitError::new(
            code::REMOTE_PROJECT_NOT_LINKED,
            format!("Remote Project `{tracker}:{project}` is not connected to this Board"),
        )
        .with_details(json!({
            "tracker": tracker,
            "project": project,
        })));
    }
    let linked_tasks = Graph::load(board, LoadMode::Lenient)?
        .0
        .iter()
        .filter(|task| {
            active_link(task).is_some_and(|link| {
                link.tracker == tracker && link.project.eq_ignore_ascii_case(&project)
            })
        })
        .count();
    let mut output = crate::output::CommandOutput::new(
        json!({
            "tracker": tracker,
            "project": project,
            "connected": false,
            "linked_tasks": linked_tasks,
        }),
        format!("Disconnected Remote Project `{tracker}:{project}`"),
    );
    if linked_tasks > 0 {
        output.warnings.push(format!(
            "{linked_tasks} Linked Task(s) still point at `{tracker}:{project}`; sync will report them as unavailable until the project is reconnected"
        ));
    }
    Ok(output)
}

/// Link a Remote Project to the Board in whichever config shape the Board
/// uses: `[remote] projects` post-migration, the tracker's legacy section
/// (`[github] repositories`) otherwise.
fn link_remote_project(board: &Board, project: &RemoteProject) -> Result<()> {
    project.validate()?;
    let remote = github_uses_remote_config(board);
    let tracker = project.tracker.clone();
    let project_id = project.project.clone();
    board.save_config("orbit: remote connect", move |config| {
        if remote {
            let entry = RemoteProject {
                tracker: tracker.clone(),
                project: project_id.clone(),
            };
            // Case-insensitive like every other connected-project check:
            // `acme/Widget` must not duplicate a configured `acme/widget`.
            if !config.remote.projects.iter().any(|candidate| {
                candidate.tracker == tracker && candidate.project.eq_ignore_ascii_case(&project_id)
            }) {
                config.remote.projects.push(entry);
            }
            if let Some(github) = config.remote.github.as_mut()
                && tracker == "github"
            {
                github
                    .repositories
                    .retain(|repo| !repo.eq_ignore_ascii_case(&project_id));
            }
            if tracker == "github" {
                config
                    .github
                    .repositories
                    .retain(|repo| !repo.eq_ignore_ascii_case(&project_id));
            }
        } else if tracker == "github" {
            config.github.link_repo(&project_id)?;
        } else {
            return Err(OrbitError::config(format!(
                "no configuration section exists for Remote Tracker `{tracker}`"
            )));
        }
        Ok(())
    })?;
    Ok(())
}

/// Remove a Remote Project link from every config shape that could carry
/// it. Returns whether anything was removed.
fn disconnect_remote_project(board: &Board, tracker: &str, project: &str) -> Result<bool> {
    let remote = github_uses_remote_config(board);
    let was_linked = configured_projects(board).iter().any(|candidate| {
        candidate.tracker == tracker && candidate.project.eq_ignore_ascii_case(project)
    }) || (tracker == "github"
        && (board
            .config
            .remote
            .github
            .as_ref()
            .is_some_and(|github| github.contains_repo(project))
            || board.config.github.contains_repo(project)));
    if !was_linked {
        return Ok(false);
    }
    let tracker = tracker.to_string();
    let project = project.to_string();
    board.save_config("orbit: remote disconnect", move |config| {
        config.remote.projects.retain(|candidate| {
            !(candidate.tracker == tracker && candidate.project.eq_ignore_ascii_case(&project))
        });
        if tracker == "github" {
            if let Some(github) = config.remote.github.as_mut() {
                github.unlink_repo(&project)?;
            }
            config.github.unlink_repo(&project)?;
        }
        // The last GitHub project is gone: drop the adapter settings too, or
        // doctor keeps flagging a configured-but-unlinked GitHub.
        let projects_left = config.remote.projects.iter().any(|p| p.tracker == "github");
        if tracker == "github"
            && !projects_left
            && config
                .remote
                .github
                .as_ref()
                .is_some_and(|github| github.repositories.is_empty())
        {
            config.remote.github = None;
        }
        let _ = remote;
        Ok(())
    })?;
    Ok(true)
}

/// `orbit remote link TASK REMOTE_ISSUE` — adopt an existing Remote Issue
/// for a Local Task (the successor of `orbit github adopt`).
fn cmd_link(board: &Board, args: RemoteLinkArgs) -> Result<crate::output::CommandOutput> {
    let (project, id) = parse_issue_ref(&args.issue)?;
    let resolver = github_resolver(board)?;
    crate::remote::execution::require_project_available(board, &resolver, &project)?;
    let adapter = resolver.adapter(&project.tracker)?;
    let issue = adapter.fetch_issue(&project, &id)?;
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let task = graph.get(&args.task)?.clone();
    if let Some(link) =
        link_view(&task).filter(|link| link.tracker == project.tracker && !link.active)
    {
        return Err(OrbitError::invalid_argument(format!(
            "task {} retains inactive {} provenance from {}#{}; review it before linking another Issue",
            args.task, project.tracker, link.project, link.id,
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
            link.tracker == project.tracker
                && link.project.eq_ignore_ascii_case(&project.project)
                && link.id == id
        })
    }) {
        return Err(OrbitError::invalid_argument(format!(
            "Remote Issue {}:{}#{} is already linked to Task {}; one Remote Issue can be paired with only one Orbit Task",
            project.tracker, project.project, id, existing.id
        )));
    }
    let mut task = task;
    task.front.remote = Some(crate::remote::RemoteLink {
        tracker: project.tracker.clone(),
        project: project.project.clone(),
        id: id.clone(),
        url: issue.url.clone(),
        active: true,
        baseline: None,
    });
    task.front.updated_at = Utc::now();
    crate::remote::execution::set_baseline(&mut task, &issue);
    let include_fields = match project.tracker.as_str() {
        "github" => !github_config(board).mode.is_basic(),
        _ => true,
    };
    apply_issue_to_task(&mut task, &issue, include_fields)?;
    sync::validate_orbit_metadata(&task)?;
    let outcome = persist_task(board, &task, &format!("orbit: remote link {}", task.id))?;
    let human = format!(
        "Linked {}:{}#{} for {}",
        project.tracker, project.project, id, task.id
    );
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

fn cmd_create(board: &Board, args: RemoteCreateArgs) -> Result<crate::output::CommandOutput> {
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let resolver = github_resolver(board)?;
    let candidates = crate::remote::connected_projects(board)?;
    let create_project = select_create_project(&candidates, args.project.as_deref())?;
    let project_label = format!("{}:{}", create_project.tracker, create_project.project);
    crate::remote::execution::require_project_available(board, &resolver, &create_project)?;
    let mapped = resolver
        .adapter(&create_project.tracker)?
        .mapped_fields(&create_project);

    let mut plans = Vec::new();
    let mut selected = std::collections::HashSet::new();
    for id in &args.tasks {
        let task = graph.get(id)?;
        let unmapped: Vec<&str> = [
            ("area", !mapped.area && task.front.area.is_some()),
            (
                "verification",
                !mapped.verification && task.front.verification.is_some(),
            ),
        ]
        .into_iter()
        .filter_map(|(name, present)| present.then_some(name))
        .collect();
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
                "task {id} already links to a Remote Issue; use `orbit remote sync {id}` instead"
            ))
        } else if task.front.status.is_terminal() && !args.include_closed {
            Some(format!(
                "task {id} is {}; pass --include-closed to export done and cancelled Tasks",
                task.front.status.as_str()
            ))
        } else if !unmapped.is_empty() {
            Some(unmapped_metadata_error(task, &create_project, &unmapped))
        } else {
            None
        };
        plans.push((task.clone(), rejected));
    }

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
                "project": project_label,
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
                format_create_preview(&task, &project_label, issue_state)
            ));
            continue;
        }

        let preview = json!({
            "task": task.id,
            "project": project_label,
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
                format_create_preview(&task, &project_label, issue_state)
            ));
            continue;
        }

        let created = match crate::remote::execution::create_and_link(
            board,
            &resolver,
            &project,
            &task,
            &format!("orbit: remote create {}", task.id),
        ) {
            Ok(created) => created,
            Err(error) => {
                results.push(json!({
                    "task": task.id,
                    "project": project_label,
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
                    format_create_preview(&task, &project_label, issue_state),
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
                        "{}: created {}#{} but could not update the Remote Issue cache: {}; run `orbit remote issues refresh`",
                        task.id, project_label, final_issue.id, error.message
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
                        "run `orbit remote link {} {}#{}` to link the created Issue",
                        task.id, project_label, final_issue.id
                    ),
                    "created, but could not link the Task",
                ),
                CreateFollowUp::CloseFailed(error) => (
                    true,
                    error,
                    format!(
                        "run `orbit remote sync {} --apply` to close the Issue",
                        task.id
                    ),
                    "created and linked, but could not close Issue",
                ),
                CreateFollowUp::CatalogUpdateFailed(_) => unreachable!("handled above"),
            };
            results.push(json!({
                "task": task.id,
                "project": project_label,
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
                format_create_preview(&task, &project_label, issue_state),
                error.message
            ));
            continue;
        }
        warnings.append(&mut commit_warnings);
        results.push(json!({
            "task": task.id,
            "project": project_label,
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
            "{}\n  Result: created {}#{}",
            format_create_preview(&task, &project_label, issue_state),
            project_label,
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
        "project": project_label,
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

/// The rejection message for Orbit metadata the Remote Tracker's adapter
/// does not map for the chosen Remote Project.
fn unmapped_metadata_error(task: &Task, project: &RemoteProject, fields: &[&str]) -> String {
    format!(
        "Remote Tracker `{}` does not map task {} field(s) {} for `{}`; remove them or choose a Remote Project that maps them",
        project.tracker,
        task.id,
        fields.join(", "),
        project.project,
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
        "Task: {}\n  Remote Project: {}\n  Title: {}\n  Body:\n{}\n  Labels: {}\n  Area: {}\n  Verification: {}\n  Task status: {}\n  Remote Issue state: {}",
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

fn select_create_project(
    candidates: &[RemoteProject],
    requested: Option<&str>,
) -> Result<RemoteProject> {
    if let Some(requested) = requested {
        let (tracker, project) = parse_remote_project(requested)?;
        if let Some(candidate) = candidates.iter().find(|candidate| {
            candidate.tracker == tracker && candidate.project.eq_ignore_ascii_case(&project)
        }) {
            return Ok(candidate.clone());
        }
        return Ok(RemoteProject { tracker, project });
    }
    match candidates {
        [project] => Ok(project.clone()),
        [] => Err(OrbitError::invalid_argument(
            "no Remote Projects are connected; run `orbit remote connect` or pass `--project TRACKER:PROJECT` before exporting",
        )),
        _ => Err(OrbitError::invalid_argument(
            "multiple Remote Projects are connected; choose one with `--project TRACKER:PROJECT`",
        )),
    }
}

fn cmd_sync(board: &Board, args: RemoteSyncArgs) -> Result<crate::output::CommandOutput> {
    let direction = match args.direction {
        RemoteSyncDirection::Both => SyncDirection::Both,
        RemoteSyncDirection::Pull => SyncDirection::Pull,
        RemoteSyncDirection::Push => SyncDirection::Push,
    };
    let resolver = github_resolver(board)?;
    let report = execution::synchronize(
        board,
        &resolver,
        execution::SyncRequest {
            task: args.task.as_deref(),
            direction,
            resolutions: &args.resolutions,
            apply: args.apply,
            tracker: None,
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
            "Applied Remote Tracker sync for {}/{} task(s); {} failed and can be retried",
            report.success_count,
            report.tasks.len(),
            report.failures.len()
        )
    } else {
        format!(
            "Remote Tracker sync preview (dry run; nothing was written)\n{}",
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

fn cmd_auto_push(board: &Board, args: RemoteAutoPushArgs) -> Result<crate::output::CommandOutput> {
    let enabled = matches!(args.state, AutoPushState::On);
    let previous = auto_push_enabled(board);
    let changed = previous != enabled;
    let mut warnings = Vec::new();
    if changed {
        // Auto-push is provider-neutral: the flag lives in `[remote]` and
        // covers every configured Remote Tracker.
        let outcome = board.save_config(
            if enabled {
                "orbit: remote auto-push on"
            } else {
                "orbit: remote auto-push off"
            },
            |config| {
                config.remote.auto_push = enabled;
                Ok(())
            },
        )?;
        warnings.extend(crate::mutation::commit_warning(outcome));
    }
    if changed && enabled {
        let repositories = crate::remote::connected_github_projects(board)?;
        match repositories.len() {
            1 => {}
            0 => warnings.push(
                "new Tasks will stay local: auto-push needs one available GitHub repository to create an Issue"
                    .to_string(),
            ),
            count => warnings.push(format!(
                "new Tasks will stay local: {count} Remote Projects are available, so Issue creation has no single destination; keep one connected project or pass `--project TRACKER:PROJECT` to `orbit remote create`"
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

// ===== Remote Issue catalog and link lifecycle =====

fn cmd_issues(board: &Board, args: RemoteIssuesArgs) -> Result<crate::output::CommandOutput> {
    match args.command {
        Some(RemoteIssuesCommand::Refresh) => issues_refresh(board),
        Some(RemoteIssuesCommand::Show(ref ref_args)) => issues_show(board, ref_args),
        Some(RemoteIssuesCommand::List(ref filters)) => issues_list(board, filters),
        None => issues_list(board, &args.filters),
    }
}

/// `orbit remote issues refresh` — refresh the Remote Issue catalog for
/// every connected Remote Project.
fn issues_refresh(board: &Board) -> Result<crate::output::CommandOutput> {
    let root = catalog::root_for_board(board)?;
    let path = catalog::cache_path(&root);
    let mut catalog = catalog::load(&path)?;
    let resolver = BoardResolver::with_client(board, crate::github::client::client_from_env()?);
    let projects = configured_projects(board);
    let mut errors = Vec::new();
    let mut refreshed_count = 0;

    for project in &projects {
        let result = resolver
            .adapter(&project.tracker)
            .and_then(|adapter| adapter.list_issues(project));
        let entry = catalog
            .projects
            .entry(project.tracker.clone())
            .or_default()
            .entry(project.project.clone())
            .or_default();
        match result {
            Ok(issues) => {
                entry.issues = issues;
                entry.last_refreshed_at = Some(Utc::now());
                entry.error = None;
                refreshed_count += 1;
            }
            Err(error) => {
                let had_snapshot = entry.last_refreshed_at.is_some();
                entry.error = Some(error.message.clone());
                errors.push(json!({
                    "tracker": project.tracker,
                    "project": project.project,
                    "error": error.message,
                    "stale": had_snapshot,
                    "status": if had_snapshot { "stale" } else { "unavailable" },
                }));
            }
        }
    }
    catalog::save(&path, &catalog)?;
    let data = json!({
        "refreshed_count": refreshed_count,
        "project_count": projects.len(),
        "errors": errors,
        "projects": project_statuses(&catalog),
        "issue_count": catalog
            .projects
            .values()
            .flat_map(|projects| projects.values())
            .map(|entry| entry.issues.len())
            .sum::<usize>(),
    });
    let summary = format!(
        "Refreshed Remote Issue catalog for {refreshed_count}/{} Remote Projects ({} errors)",
        projects.len(),
        data["errors"].as_array().map_or(0, Vec::len)
    );
    Ok(crate::output::CommandOutput::new(data, summary))
}

fn project_statuses(catalog: &RemoteIssueCatalog) -> Vec<Value> {
    let mut statuses = Vec::new();
    for (tracker, projects) in &catalog.projects {
        for (project, entry) in projects {
            statuses.push(json!({
                "tracker": tracker,
                "project": project,
                "last_refreshed_at": entry.last_refreshed_at,
                "status": match (entry.error.is_some(), entry.last_refreshed_at.is_some()) {
                    (false, _) => "fresh",
                    (true, true) => "stale",
                    (true, false) => "unavailable",
                },
                "stale": entry.error.is_some() && entry.last_refreshed_at.is_some(),
                "available": entry.last_refreshed_at.is_some(),
                "error": entry.error,
                "issue_count": entry.issues.len(),
            }));
        }
    }
    statuses
}

/// `orbit remote issues` / `issues list` — cached Remote Issues with
/// optional filters; never fetches issue bodies.
fn issues_list(
    board: &Board,
    filters: &RemoteIssueFilters,
) -> Result<crate::output::CommandOutput> {
    let root = catalog::root_for_board(board)?;
    let catalog = catalog::load(&catalog::cache_path(&root))?;
    let graph = Graph::load(board, LoadMode::Lenient)?.0;
    let linked = linked_remote_identities(&graph);

    let project_filter = filters
        .project
        .as_deref()
        .map(|project| parse_remote_project(project).map(|(_, project)| project.to_lowercase()))
        .transpose()?;
    let text_filter = filters.text.as_deref().map(str::to_lowercase);
    let mut issues = Vec::new();
    for (tracker, projects) in &catalog.projects {
        if filters
            .tracker
            .as_deref()
            .is_some_and(|filter| !filter.eq_ignore_ascii_case(tracker))
        {
            continue;
        }
        for (project, cached) in projects {
            if project_filter
                .as_deref()
                .is_some_and(|filter| filter != project.to_lowercase())
            {
                continue;
            }
            for issue in &cached.issues {
                if filters
                    .state
                    .as_deref()
                    .is_some_and(|state| state_name(issue.status) != state)
                {
                    continue;
                }
                if filters.label.iter().any(|label| {
                    !issue
                        .labels
                        .iter()
                        .any(|item| item.eq_ignore_ascii_case(label))
                }) {
                    continue;
                }
                if text_filter
                    .as_deref()
                    .is_some_and(|text| !issue.title.to_lowercase().contains(text))
                {
                    continue;
                }
                let key = format!("{tracker}:{}#{}", project.to_lowercase(), issue.id);
                let is_linked = linked.contains(&key);
                if filters.linked && !is_linked || filters.unlinked && is_linked {
                    continue;
                }
                let mut value = serde_json::to_value(issue).unwrap_or(Value::Null);
                if let Some(object) = value.as_object_mut() {
                    object.insert("tracker".to_string(), json!(tracker));
                    object.insert("identity".to_string(), json!(key));
                    object.insert("linked".to_string(), json!(is_linked));
                    object.insert("stale".to_string(), json!(cached.error.is_some()));
                    object.insert(
                        "last_refreshed_at".to_string(),
                        json!(cached.last_refreshed_at),
                    );
                }
                issues.push(value);
            }
        }
    }
    issues.sort_by(|a, b| a["identity"].as_str().cmp(&b["identity"].as_str()));
    let data = json!({
        "count": issues.len(),
        "issues": issues,
        "projects": project_statuses(&catalog),
    });
    Ok(crate::output::CommandOutput::new(
        data,
        format!("{} Remote Issues", issues.len()),
    ))
}

/// `orbit remote issues show` — one Remote Issue fetched live from its
/// tracker, with catalog staleness and link state.
fn issues_show(board: &Board, args: &RemoteIssueRefArgs) -> Result<crate::output::CommandOutput> {
    let (project, id) = parse_issue_ref(&args.issue)?;
    let resolver = github_resolver(board)?;
    let adapter = resolver.adapter(&project.tracker)?;
    let issue = adapter.fetch_issue(&project, &id)?;
    let root = catalog::root_for_board(board)?;
    let catalog = catalog::load(&catalog::cache_path(&root))?;
    let cached = catalog
        .projects
        .get(&project.tracker)
        .and_then(|projects| projects.get(&project.project));
    let graph = Graph::load(board, LoadMode::Lenient)?.0;
    let linked = linked_remote_identities(&graph).contains(&format!(
        "{}:{}#{}",
        project.tracker,
        project.project.to_lowercase(),
        id
    ));
    let data = json!({
        "tracker": project.tracker,
        "project": project.project,
        "id": issue.id,
        "identity": format!("{}:{}#{}", project.tracker, project.project, issue.id),
        "url": issue.url,
        "title": issue.title,
        "body": issue.body,
        "status": issue.status,
        "labels": issue.labels,
        "markers": issue.markers,
        "area": issue.area,
        "verification": issue.verification,
        "actors": issue.actors,
        "relations": issue.relations,
        "updated_at": issue.updated_at,
        "linked": linked,
        "stale": cached.is_some_and(|entry| entry.error.is_some()),
        "last_refreshed_at": cached.and_then(|entry| entry.last_refreshed_at),
    });
    Ok(crate::output::CommandOutput::new(
        data,
        format!(
            "{}:{}#{} — {}",
            project.tracker, project.project, id, issue.title
        ),
    ))
}

/// Active Remote links keyed `tracker:project#id` for catalog `linked`
/// flags; detached provenance does not count as linked.
fn linked_remote_identities(graph: &Graph) -> std::collections::BTreeSet<String> {
    graph
        .iter()
        .filter_map(active_link)
        .map(|link| {
            format!(
                "{}:{}#{}",
                link.tracker,
                link.project.to_lowercase(),
                link.id
            )
        })
        .collect()
}

fn state_name(status: Status) -> &'static str {
    if status.is_terminal() {
        "closed"
    } else {
        "open"
    }
}

fn parse_link_mappings(
    mappings: &[String],
    identities: &[String],
) -> Result<std::collections::BTreeMap<String, String>> {
    let selected = identities
        .iter()
        .filter_map(|identity| parse_issue_ref(identity).ok())
        .map(|(project, id)| {
            format!(
                "{}:{}#{}",
                project.tracker,
                project.project.to_lowercase(),
                id
            )
        })
        .collect::<std::collections::BTreeSet<_>>();
    let mut parsed = std::collections::BTreeMap::new();
    for mapping in mappings {
        let (identity, task) = mapping
            .split_once('=')
            .ok_or_else(|| OrbitError::invalid_argument("--link must use REMOTE_ISSUE=TASK"))?;
        let (project, id) = parse_issue_ref(identity)?;
        let key = format!(
            "{}:{}#{}",
            project.tracker,
            project.project.to_lowercase(),
            id
        );
        let task = task.trim();
        if task.is_empty() {
            return Err(OrbitError::invalid_argument(
                "--link task ID must not be empty",
            ));
        }
        if !selected.contains(&key) {
            return Err(OrbitError::invalid_argument(format!(
                "--link identity `{identity}` is not in the selected import batch"
            )));
        }
        if parsed.insert(key.clone(), task.to_string()).is_some() {
            return Err(OrbitError::invalid_argument(format!(
                "multiple --link mappings were provided for `{key}`"
            )));
        }
    }
    Ok(parsed)
}

fn remote_issue_key(project: &RemoteProject, id: &str) -> String {
    format!(
        "{}:{}#{}",
        project.tracker,
        project.project.to_lowercase(),
        id
    )
}

/// `orbit remote import` — preview or apply importing Remote Issues as
/// Local Tasks or Linked Tasks. The tracker is never written.
fn cmd_import(board: &Board, args: RemoteImportArgs) -> Result<crate::output::CommandOutput> {
    let root = catalog::root_for_board(board)?;
    let catalog = catalog::load(&catalog::cache_path(&root))?;
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let mappings = parse_link_mappings(&args.link, &args.issues)?;
    let resolver = github_resolver(board)?;

    let mut linked_issues = std::collections::BTreeMap::new();
    for task in graph.iter() {
        if let Some(link) = link_view(task).filter(|link| link.active) {
            linked_issues.insert(
                remote_issue_key(
                    &RemoteProject {
                        tracker: link.tracker.clone(),
                        project: link.project.clone(),
                    },
                    &link.id,
                ),
                task.id.clone(),
            );
        }
    }

    let mut candidates = Vec::new();
    let mut creates = Vec::new();
    let mut links = Vec::new();
    let mut terminal_tasks = Vec::new();
    let mut skipped_duplicates = Vec::new();
    let mut errors = Vec::new();
    let mut seen_issues = std::collections::BTreeSet::new();
    let mut seen_tasks = std::collections::HashSet::new();

    for raw_identity in &args.issues {
        let (project, id) = match parse_issue_ref(raw_identity) {
            Ok(parsed) => parsed,
            Err(error) => {
                errors.push(json!({"identity": raw_identity, "message": error.message}));
                continue;
            }
        };
        let key = remote_issue_key(&project, &id);
        let identity = format!("{}:{}#{}", project.tracker, project.project, id);
        if !seen_issues.insert(key.clone()) {
            skipped_duplicates.push(json!({
                "identity": identity,
                "reason": "issue_selected_more_than_once",
            }));
            continue;
        }
        if !crate::remote::execution::project_available(board, &resolver, &project)? {
            errors.push(json!({
                "identity": identity,
                "message": format!(
                    "Remote Project `{}:{}` is not connected to this Board",
                    project.tracker, project.project
                ),
            }));
            continue;
        }
        let in_catalog = catalog
            .projects
            .get(&project.tracker)
            .and_then(|projects| projects.get(&project.project))
            .is_some_and(|cached| cached.issues.iter().any(|issue| issue.id == id));
        if !in_catalog {
            errors.push(json!({
                "identity": identity,
                "message": "Remote Issue is not in the local catalog; run `orbit remote issues refresh` first",
            }));
            continue;
        }
        if let Some(task) = linked_issues.get(&key) {
            skipped_duplicates.push(json!({
                "identity": identity,
                "reason": "issue_already_linked",
                "task": task,
            }));
            continue;
        }

        let existing_task = if let Some(task_id) = mappings.get(&key) {
            let Some(task) = graph.tasks.get(task_id) else {
                errors.push(json!({
                    "identity": identity,
                    "task": task_id,
                    "message": format!("Local Task `{task_id}` does not exist"),
                }));
                continue;
            };
            if let Some(link) = link_view(task).filter(|link| !link.active) {
                errors.push(json!({
                    "identity": identity,
                    "task": task_id,
                    "message": format!(
                        "Local Task `{task_id}` retains inactive {} provenance from {}#{}; use `orbit remote reattach`",
                        link.tracker, link.project, link.id
                    ),
                }));
                continue;
            }
            if link_view(task).is_some() {
                skipped_duplicates.push(json!({
                    "identity": identity,
                    "reason": "task_already_linked",
                    "task": task_id,
                }));
                continue;
            }
            if !seen_tasks.insert(task_id.clone()) {
                skipped_duplicates.push(json!({
                    "identity": identity,
                    "reason": "task_selected_more_than_once",
                    "task": task_id,
                }));
                continue;
            }
            Some(task.clone())
        } else {
            None
        };

        let issue = match resolver
            .adapter(&project.tracker)
            .and_then(|adapter| adapter.fetch_issue(&project, &id))
        {
            Ok(issue) => issue,
            Err(error) => {
                errors.push(json!({"identity": identity, "message": error.message}));
                continue;
            }
        };
        if issue.title.trim().is_empty() {
            errors.push(json!({
                "identity": identity,
                "message": "Remote Issue title must not be empty",
            }));
            continue;
        }

        match existing_task {
            Some(task) => {
                let differences = task_issue_differences(&task, &issue);
                links.push(json!({
                    "identity": identity,
                    "task": task.id,
                    "status": task.front.status,
                    "remote_status": issue.status,
                    "differences": differences,
                }));
                if task.front.status.is_terminal() {
                    terminal_tasks.push(json!({
                        "identity": identity,
                        "task": task.id,
                        "status": task.front.status,
                    }));
                }
                candidates.push((
                    project.clone(),
                    ImportCandidate::Link(issue, Box::new(task)),
                ));
            }
            None => {
                creates.push(json!({
                    "identity": identity,
                    "title": issue.title,
                    "status": issue.status,
                    "terminal": issue.status.is_terminal(),
                    "url": issue.url,
                }));
                if issue.status.is_terminal() {
                    terminal_tasks.push(json!({
                        "identity": identity,
                        "status": issue.status,
                    }));
                }
                candidates.push((project.clone(), ImportCandidate::Create(issue)));
            }
        }
    }

    let mut imported = Vec::new();
    let mut commit_warning = None;
    if args.apply {
        let mut reserved_ids = graph
            .tasks
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        let mut writes = Vec::new();
        for (project, candidate) in candidates {
            let (task, created, identity) = match candidate {
                ImportCandidate::Create(issue) => {
                    let identity = format!("{}:{}#{}", project.tracker, issue.project, issue.id);
                    let task = create_imported_task(board, &project, &issue, &mut reserved_ids)?;
                    (task, true, identity)
                }
                ImportCandidate::Link(issue, task) => {
                    let identity = format!("{}:{}#{}", project.tracker, issue.project, issue.id);
                    let mut task = *task;
                    task.touch(Utc::now());
                    task.front.remote = Some(crate::remote::RemoteLink {
                        tracker: project.tracker.clone(),
                        project: issue.project.clone(),
                        id: issue.id.clone(),
                        url: issue.url.clone(),
                        active: true,
                        baseline: None,
                    });
                    execution::set_baseline(&mut task, &issue);
                    (task, false, identity)
                }
            };
            writes.push((task, created, identity));
        }

        for (task, created, identity) in &writes {
            if *created {
                crate::mutation::create_task(board, task)?;
            } else {
                crate::mutation::write_task(board, task)?;
            }
            imported.push(json!({
                "identity": identity,
                "task": task.id,
                "action": if *created { "created" } else { "linked" },
                "status": task.front.status,
            }));
        }
        if !writes.is_empty() {
            commit_warning = crate::mutation::commit(board, "orbit: import Remote Issues")?;
        }
    }

    let data = json!({
        "apply": args.apply,
        "creates": creates,
        "links": links,
        "terminal_tasks": terminal_tasks,
        "skipped_duplicates": skipped_duplicates,
        "errors": errors,
        "imported": imported,
    });
    let summary = if args.apply {
        format!(
            "Imported {} Remote Issue(s); the tracker was not changed",
            data["imported"].as_array().map_or(0, Vec::len)
        )
    } else {
        format!(
            "Remote Issue import preview: {} create(s), {} link(s), {} duplicate(s), {} error(s); nothing was written",
            data["creates"].as_array().map_or(0, Vec::len),
            data["links"].as_array().map_or(0, Vec::len),
            data["skipped_duplicates"].as_array().map_or(0, Vec::len),
            data["errors"].as_array().map_or(0, Vec::len),
        )
    };
    let mut output = crate::output::CommandOutput::new(data, summary);
    output.warnings.extend(commit_warning);
    Ok(output)
}

enum ImportCandidate {
    Create(RemoteIssue),
    Link(RemoteIssue, Box<Task>),
}

fn task_issue_differences(task: &Task, issue: &RemoteIssue) -> Vec<Value> {
    let fields = [
        ("title", json!(task.front.title), json!(issue.title)),
        ("body", json!(task.body), json!(issue.body)),
        ("labels", json!(task.front.labels), json!(issue.labels)),
        ("status", json!(task.front.status), json!(issue.status)),
        ("area", json!(task.front.area), json!(issue.area)),
        (
            "verification",
            json!(task.front.verification),
            json!(issue.verification),
        ),
    ];
    fields
        .into_iter()
        .filter(|(_, local, remote)| local != remote)
        .map(|(field, orbit, remote)| json!({"field": field, "orbit": orbit, "remote": remote}))
        .collect()
}

fn create_imported_task(
    board: &Board,
    project: &RemoteProject,
    issue: &RemoteIssue,
    reserved_ids: &mut std::collections::HashSet<String>,
) -> Result<Task> {
    let id = loop {
        let id = task::generate_id(&board.config.id.prefix, board.config.id.length);
        if reserved_ids.insert(id.clone()) && !board.task_path(&id).exists() {
            break id;
        }
    };
    let now = Utc::now();
    let mut labels = Vec::with_capacity(issue.labels.len());
    for label in &issue.labels {
        let label = task::normalize_label(label)?;
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels.sort();
    let area = issue
        .area
        .as_deref()
        .map(|area| {
            let normalized = area.trim().to_lowercase();
            if !task::is_valid_area(&normalized) {
                return Err(OrbitError::invalid_argument(
                    "area must be a short lowercase slug of letters, digits, and `-`",
                ));
            }
            Ok(normalized)
        })
        .transpose()?;
    let verification = task::normalize_verification(issue.verification.clone())?;
    let mut task = Task {
        id,
        front: crate::task::TaskFrontmatter {
            title: issue.title.trim().to_string(),
            status: issue.status,
            task_type: crate::task::TaskType::Task,
            priority: board.config.board.default_priority,
            labels,
            depends_on: Vec::new(),
            parent: None,
            waiting_on: None,
            checkpoint: None,
            area,
            verification,
            github: None,
            remote: Some(crate::remote::RemoteLink {
                tracker: project.tracker.clone(),
                project: issue.project.clone(),
                id: issue.id.clone(),
                url: issue.url.clone(),
                active: true,
                baseline: None,
            }),
            repo: None,
            claim: None,
            stray_claim_times: None,
            source_key: None,
            annotations: std::collections::BTreeMap::new(),
            created_at: now,
            updated_at: now,
            extra: std::collections::BTreeMap::new(),
        },
        body: issue.body.clone(),
    };
    execution::set_baseline(&mut task, issue);
    Ok(task)
}

/// Resolve a detach/reattach target: a Remote Issue identity
/// (`TRACKER:PROJECT#ID`, `PROJECT#ID`, or URL) or a Task id.
enum DetachTarget {
    Issue(RemoteProject, String),
    Task(String),
}

fn parse_detach_target(target: &str) -> Result<DetachTarget> {
    if target.contains('#') || target.contains("://") {
        let (project, id) = parse_issue_ref(target)?;
        return Ok(DetachTarget::Issue(project, id));
    }
    Ok(DetachTarget::Task(target.trim().to_string()))
}

/// `orbit remote detach` — preview or detach a Linked Task from its Remote
/// Issue. The Task becomes a Local Task retaining Remote provenance.
fn cmd_detach(board: &Board, args: RemoteMutationArgs) -> Result<crate::output::CommandOutput> {
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let root = catalog::root_for_board(board)?;
    let catalog_path = catalog::cache_path(&root);
    let mut catalog = catalog::load(&catalog_path)?;

    // (task, link view, already_detached)
    let mut matched: Vec<(Task, crate::remote::link::RemoteLinkView, bool)> = Vec::new();
    let mut not_found = None;
    match parse_detach_target(&args.target)? {
        DetachTarget::Issue(project, id) => {
            let key_project = project.project.to_lowercase();
            for task in graph.iter() {
                if let Some(link) = link_view(task)
                    && link.tracker == project.tracker
                    && link.project.eq_ignore_ascii_case(&key_project)
                    && link.id == id
                {
                    let detached = !link.active;
                    matched.push((task.clone(), link, detached));
                }
            }
            if matched.is_empty() {
                not_found = Some(format!("{}:{}#{}", project.tracker, project.project, id));
            }
        }
        DetachTarget::Task(task_id) => {
            let Some(task) = graph.tasks.get(&task_id) else {
                return Err(OrbitError::not_found(&task_id));
            };
            let Some(link) = link_view(task) else {
                return Err(OrbitError::invalid_argument(format!(
                    "task {task_id} has no Remote Issue link"
                )));
            };
            let detached = !link.active;
            matched.push((task.clone(), link, detached));
        }
    }
    let Some(first) = matched.first() else {
        return Err(OrbitError::not_found(
            not_found.as_deref().unwrap_or(&args.target),
        ));
    };
    let already_detached = matched.iter().all(|(_, _, detached)| *detached);
    let identity = format!("{}:{}#{}", first.1.tracker, first.1.project, first.1.id);
    let tasks_preview: Vec<Value> = matched
        .iter()
        .map(|(task, link, _)| {
            json!({
                "task": task.id,
                "title": task.front.title,
                "identity": format!("{}:{}#{}", link.tracker, link.project, link.id),
            })
        })
        .collect();
    let mut detached = Vec::new();
    let mut commit_warning = None;
    if args.apply && !already_detached {
        for (task, link, inactive) in &matched {
            if *inactive {
                continue;
            }
            let mut task = task.clone();
            if task.front.remote.is_none() {
                // Legacy `github:` links materialize on write.
                crate::remote::link::materialize(&mut task);
            }
            let stored = task.front.remote.as_mut().expect("remote link exists");
            stored.active = false;
            task.touch(Utc::now());
            crate::mutation::write_task(board, &task)?;
            detached.push(json!({
                "task": task.id,
                "identity": format!("{}:{}#{}", link.tracker, link.project, link.id),
            }));
        }
        // Archive the catalog snapshot so `issues list` stops showing it.
        for (_, link, _) in &matched {
            archive_catalog_issue(&mut catalog, &link.tracker, &link.project, &link.id);
        }
        catalog::save(&catalog_path, &catalog)?;
        if !detached.is_empty() {
            commit_warning = crate::mutation::commit(board, "orbit: remote detach")?;
        }
    }
    let data = json!({
        "identity": identity,
        "apply": args.apply,
        "already_detached": already_detached,
        "tasks": tasks_preview,
        "detached": detached,
    });
    let summary = if args.apply {
        format!("Detached {identity}; the tracker was not changed")
    } else {
        format!("Remote Issue detach preview for {identity}; nothing was written")
    };
    let mut output = crate::output::CommandOutput::new(data, summary);
    output.warnings.extend(commit_warning);
    Ok(output)
}

/// `orbit remote reattach` — preview or reattach a Local Task by its
/// inactive Remote provenance.
fn cmd_reattach(board: &Board, args: RemoteMutationArgs) -> Result<crate::output::CommandOutput> {
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let root = catalog::root_for_board(board)?;
    let catalog_path = catalog::cache_path(&root);
    let mut catalog = catalog::load(&catalog_path)?;

    let mut matched: Vec<(Task, crate::remote::link::RemoteLinkView)> = Vec::new();
    let mut already_attached = false;
    match parse_detach_target(&args.target)? {
        DetachTarget::Issue(project, id) => {
            let key_project = project.project.to_lowercase();
            let mut inactive_count = 0usize;
            for task in graph.iter() {
                if let Some(link) = link_view(task)
                    && link.tracker == project.tracker
                    && link.project.eq_ignore_ascii_case(&key_project)
                    && link.id == id
                {
                    if link.active {
                        already_attached = true;
                    } else {
                        inactive_count += 1;
                        matched.push((task.clone(), link));
                    }
                }
            }
            if matched.is_empty() && !already_attached {
                return Err(OrbitError::not_found(&format!(
                    "{}:{}#{}",
                    project.tracker, project.project, id
                )));
            }
            if inactive_count > 1 {
                return Err(OrbitError::invalid_argument(format!(
                    "cannot reattach {}:{}#{}: multiple Tasks retain that Remote identity",
                    project.tracker, project.project, id
                )));
            }
        }
        DetachTarget::Task(task_id) => {
            let Some(task) = graph.tasks.get(&task_id) else {
                return Err(OrbitError::not_found(&task_id));
            };
            let Some(link) = link_view(task) else {
                return Err(OrbitError::invalid_argument(format!(
                    "task {task_id} has no Remote Issue provenance"
                )));
            };
            if link.active {
                already_attached = true;
            } else {
                matched.push((task.clone(), link));
            }
        }
    }

    if matched.is_empty() {
        if already_attached {
            let identity = match parse_detach_target(&args.target)? {
                DetachTarget::Issue(project, id) => {
                    format!("{}:{}#{}", project.tracker, project.project, id)
                }
                DetachTarget::Task(task_id) => graph
                    .tasks
                    .get(&task_id)
                    .and_then(link_view)
                    .map(|link| format!("{}:{}#{}", link.tracker, link.project, link.id))
                    .unwrap_or_else(|| args.target.clone()),
            };
            return Ok(crate::output::CommandOutput::new(
                json!({
                    "identity": identity,
                    "apply": args.apply,
                    "already_attached": true,
                    "tasks": [],
                    "reattached": [],
                }),
                format!("{identity} is already attached"),
            ));
        }
        return Err(OrbitError::not_found(&args.target));
    }

    let (task, link) = matched.into_iter().next().expect("one match");
    let project = RemoteProject {
        tracker: link.tracker.clone(),
        project: link.project.clone(),
    };
    let identity = format!("{}:{}#{}", link.tracker, link.project, link.id);
    let resolver = github_resolver(board)?;
    if !crate::remote::execution::project_available(board, &resolver, &project)? {
        return Err(OrbitError::invalid_argument(format!(
            "Remote Project `{}:{}` is not connected to this Board; connect it before reattaching",
            project.tracker, project.project
        )));
    }
    let adapter = resolver.adapter(&project.tracker)?;
    let issue = adapter.fetch_issue(&project, &link.id)?;
    validate_issue_identity(&issue, &project, &link.id)?;
    let sync_plan = sync::plan_sync(
        adapter.as_ref(),
        sync::OrbitSyncView {
            task: &task,
            link: &link,
        },
        &issue,
        SyncDirection::Both,
    );
    let preview = json!({
        "task": task.id,
        "identity": identity,
        "issue_title": issue.title,
        "sync_plan": sync_plan,
    });
    let mut reattached = Vec::new();
    let mut commit_warning = None;
    if args.apply {
        let mut task = task.clone();
        crate::remote::link::materialize(&mut task);
        task.front
            .remote
            .as_mut()
            .expect("remote provenance exists")
            .active = true;
        task.touch(Utc::now());
        crate::mutation::write_task(board, &task)?;
        restore_catalog_issue(
            &mut catalog,
            &project.tracker,
            &project.project,
            &link.id,
            &issue,
        );
        catalog::save(&catalog_path, &catalog)?;
        commit_warning = crate::mutation::commit(board, "orbit: remote reattach")?;
        reattached.push(json!({ "task": task.id, "identity": identity }));
    }
    let data = json!({
        "identity": identity,
        "apply": args.apply,
        "tasks": [preview],
        "reattached": reattached,
    });
    let summary = if args.apply {
        format!("Reattached {identity}; review the sync plan before synchronizing")
    } else {
        format!("Remote Issue reattach preview for {identity}; no data was changed")
    };
    let mut output = crate::output::CommandOutput::new(data, summary);
    output.warnings.extend(commit_warning);
    Ok(output)
}

fn archive_catalog_issue(catalog: &mut RemoteIssueCatalog, tracker: &str, project: &str, id: &str) {
    let Some(projects) = catalog.projects.get_mut(tracker) else {
        return;
    };
    let Some(cached) = projects.get_mut(project) else {
        return;
    };
    if let Some(index) = cached.issues.iter().position(|issue| issue.id == id) {
        let issue = cached.issues.remove(index);
        catalog
            .archived
            .entry(tracker.to_string())
            .or_default()
            .entry(project.to_string())
            .or_default()
            .insert(id.to_string(), issue);
    }
}

fn restore_catalog_issue(
    catalog: &mut RemoteIssueCatalog,
    tracker: &str,
    project: &str,
    id: &str,
    issue: &RemoteIssue,
) {
    let archived = catalog
        .archived
        .get_mut(tracker)
        .and_then(|projects| projects.get_mut(project))
        .and_then(|issues| issues.remove(id));
    let entry = catalog
        .projects
        .entry(tracker.to_string())
        .or_default()
        .entry(project.to_string())
        .or_default();
    entry.issues.retain(|cached| cached.id != id);
    entry.issues.push(match archived {
        Some(summary) => summary,
        None => RemoteIssueSummary {
            project: issue.project.clone(),
            id: issue.id.clone(),
            url: issue.url.clone(),
            title: issue.title.clone(),
            status: issue.status,
            labels: issue.labels.clone(),
            area: issue.area.clone(),
            verification: issue.verification.clone(),
            updated_at: issue.updated_at,
            fields: std::collections::BTreeMap::new(),
        },
    });
    entry
        .issues
        .sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.project.cmp(&b.project)));
    if catalog
        .archived
        .get(tracker)
        .and_then(|projects| projects.get(project))
        .is_some_and(std::collections::BTreeMap::is_empty)
        && let Some(projects) = catalog.archived.get_mut(tracker)
    {
        projects.remove(project);
    }
}

fn validate_issue_identity(issue: &RemoteIssue, project: &RemoteProject, id: &str) -> Result<()> {
    if !issue.project.eq_ignore_ascii_case(&project.project) || issue.id != id {
        return Err(OrbitError::invalid_argument(format!(
            "{} returned {}#{} while reattaching {}:{}#{}",
            project.tracker, issue.project, issue.id, project.tracker, project.project, id
        )));
    }
    Ok(())
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
    output.push_str("Push to Remote Tracker:\n");
    append_preview_fields(&mut output, &pushes);

    output.push_str("Conflicts:\n");
    if plan.conflicts.is_empty() {
        output.push_str("  (none)\n");
    } else {
        for conflict in &plan.conflicts {
            output.push_str(&format!(
                "  - {}: Orbit = {}; Remote = {}; choose `--resolve {}=orbit` or `--resolve {}=remote`\n",
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
