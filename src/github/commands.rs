use chrono::Utc;
use serde_json::json;

use crate::atomic;
use crate::board::Board;
use crate::cli::{
    GitHubAdoptArgs, GitHubArgs, GitHubCommand, GitHubCreateArgs, GitHubRepoCommand,
    GitHubRepoLinkArgs, GitHubSetupArgs, GitHubSyncArgs, GitHubSyncDirection,
};
use crate::error::{OrbitError, Result, code};
use crate::git::CommitOutcome;
use crate::github::client::{CreateIssueInput, GitHubClient, UpdateIssueInput, client_from_env};
use crate::github::link::{GitHubLink, GitHubSyncBaseline};
use crate::github::model::{IssueSnapshot, SyncDirection, SyncPlan};
use crate::github::relationships;
use crate::github::fields::parse_area;
use crate::github::sync::{self, OrbitSyncView, validate_orbit_metadata};
use crate::graph::{Graph, LoadMode};
use crate::task::{self, Task};

pub fn run(board: &Board, args: GitHubArgs) -> Result<crate::output::CommandOutput> {
    require_github_section(board)?;
    match args.command {
        GitHubCommand::Setup(args) => cmd_setup(board, args),
        GitHubCommand::Repo(command) => cmd_repo(board, command),
        GitHubCommand::Adopt(args) => cmd_adopt(board, args),
        GitHubCommand::Create(args) => cmd_create(board, args),
        GitHubCommand::Sync(args) => cmd_sync(board, args),
    }
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
