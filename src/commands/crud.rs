use chrono::Utc;
use serde_json::json;

use crate::cli::{AddArgs, DeleteArgs, UpdateArgs};
use crate::graph::LoadMode;
use crate::transition::FieldUpdate;

use super::*;

pub(super) fn cmd_add(board: &Board, args: AddArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let body = resolve_body(args.body, args.body_file)?;
    // Record the member Repository the command runs in; a worktree resolves
    // to its main checkout's Repository.
    let repository = crate::repository::current_member_name(board)?;
    let new = crate::ops::NewTask {
        title: args.title,
        priority: args.priority,
        task_type: args.task_type,
        labels: args.label,
        parent: args.parent,
        component: args.component,
        verification: args.verification,
        repository,
        external_id: args.external_id,
        reporter: args.reporter.or_else(|| optional_agent(None)),
        body,
        source_key: args.source_key,
        properties: Default::default(),
    };
    let outcome = crate::ops::add(&mut graph, board, new, Utc::now())?;
    let task = graph.get(&outcome.id)?.clone();
    let id = &task.id;
    if outcome.matched {
        let human = format!("{id} already exists for source key");
        return Ok(task_output(&graph, &task, human, None).with_meta(json!({ "matched": true })));
    }

    crate::mutation::create_task(board, &task)?;
    // Persist the key high-water mark in the same commit as the new Task.
    crate::key::persist(board, &graph)?;

    let priority = &task.front.priority;
    let title = &task.front.title;
    let type_prefix = if task.front.task_type == board.config.board.default_type {
        String::new()
    } else {
        format!("{}  ", task.front.task_type)
    };
    let display = task.display_id();
    let human = match &task.front.parent {
        Some(parent) => {
            format!(
                "created {display}  {priority}  {type_prefix}{title}  (subtask of {})",
                graph.display_id(parent)
            )
        }
        None => format!("created {display}  {priority}  {type_prefix}{title}"),
    };
    let warning = crate::mutation::commit_as(
        board,
        &format!("orbit: add {id}"),
        task.front.reporter.as_deref(),
    )?;
    Ok(task_output(&graph, &task, human, warning).with_meta(json!({ "matched": false })))
}

fn subtasks_exist_error(id: &str, subtasks: &[String]) -> OrbitError {
    OrbitError::new(
        code::SUBTASKS_EXIST,
        format!("{id} has SubTasks: {}", subtasks.join(", ")),
    )
    .with_details(json!({ "issue": id, "sub_issues": subtasks }))
}

pub(super) fn cmd_update(board: &Board, args: UpdateArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;

    let has_changes = args.title.is_some()
        || args.body.is_some()
        || args.body_file.is_some()
        || args.priority.is_some()
        || args.task_type.is_some()
        || args.component.is_some()
        || args.clear_component
        || args.repository.is_some()
        || args.clear_repository
        || args.external_id.is_some()
        || args.clear_external_id
        || args.verification.is_some()
        || args.clear_verification;
    if !has_changes {
        return Err(OrbitError::invalid_argument(
            "nothing to update; pass --title, --body, --priority, --type, --component, --repository, --external-id, or --verification",
        ));
    }

    let body = if args.body.is_some() || args.body_file.is_some() {
        Some(resolve_body(args.body, args.body_file)?)
    } else {
        None
    };
    let field = |value: Option<String>, clear: bool| {
        if clear {
            FieldUpdate::Clear
        } else {
            value.map_or(FieldUpdate::Keep, FieldUpdate::Set)
        }
    };
    let change = crate::ops::TaskUpdate {
        title: args.title,
        body,
        priority: args.priority,
        task_type: args.task_type,
        component: field(args.component, args.clear_component),
        verification: field(args.verification, args.clear_verification),
        repository: field(args.repository, args.clear_repository),
        external_id: field(args.external_id, args.clear_external_id),
    };
    crate::ops::update(&mut graph, board, &args.id, change, Utc::now())?;
    let task = graph.get(&args.id)?.clone();

    let warning = crate::mutation::save(board, &task, &format!("orbit: update {}", args.id))?;
    Ok(task_output(
        &graph,
        &task,
        format!("updated {}", args.id),
        warning,
    ))
}

pub(super) fn cmd_delete(board: &Board, args: DeleteArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Lenient)?;
    let caller = optional_agent(args.by);
    let now = Utc::now();
    // File removal needs the immutable internal ID, not a project key.
    let id = graph.resolve(&args.id)?;

    graph
        .get(&id)?
        .authorize_live(caller.as_deref(), args.force, now)?;

    let child_ids: Vec<String> = graph
        .children(&id)
        .iter()
        .map(|task| task.id.clone())
        .collect();
    if !child_ids.is_empty() && !args.force {
        return Err(subtasks_exist_error(&id, &child_ids));
    }

    let mut deleted_ids = Vec::new();
    let mut cleaned_ids = Vec::new();
    delete_subtree(
        board,
        &mut graph,
        &id,
        args.force,
        now,
        &mut deleted_ids,
        &mut cleaned_ids,
    )?;

    let warning =
        crate::mutation::commit_as(board, &format!("orbit: delete {id}"), caller.as_deref())?;
    let subtasks_deleted = deleted_ids.len().saturating_sub(1);
    let human = if subtasks_deleted == 0 {
        format!("deleted {id}")
    } else {
        format!("deleted {id} and {} SubTask(s)", subtasks_deleted)
    };
    let mut output = crate::output::CommandOutput::new(
        json!({
            "id": id,
            "deleted": true,
            "deleted_ids": deleted_ids,
            "cleaned": cleaned_ids,
        }),
        human,
    );
    output.warnings.extend(warning);
    Ok(output)
}

fn delete_subtree(
    board: &Board,
    graph: &mut Graph,
    id: &str,
    force: bool,
    now: chrono::DateTime<Utc>,
    deleted: &mut Vec<String>,
    cleaned: &mut Vec<String>,
) -> Result<()> {
    let dependent_ids: Vec<String> = graph
        .dependents(id)
        .iter()
        .map(|dependent| dependent.id.clone())
        .collect();
    if !dependent_ids.is_empty() && !force {
        return Err(OrbitError::new(
            code::DEPENDENCY_BLOCKED,
            format!("{id} has dependents: {}", dependent_ids.join(", ")),
        )
        .with_details(json!({ "issue": id, "dependents": dependent_ids })));
    }

    if force {
        for dependent in &dependent_ids {
            let updated = {
                let task = graph.get_mut(dependent)?;
                task.remove_links_to(id, now);
                task.clone()
            };
            crate::mutation::write_task(board, &updated)?;
            if !cleaned.iter().any(|seen| seen == dependent) {
                cleaned.push(dependent.clone());
            }
        }
    }

    let child_ids: Vec<String> = graph
        .children(id)
        .iter()
        .map(|task| task.id.clone())
        .collect();
    for child in child_ids {
        delete_subtree(board, graph, &child, force, now, deleted, cleaned)?;
    }

    crate::mutation::remove_task(board, id)?;
    graph.remove(id);
    deleted.push(id.to_string());
    Ok(())
}
