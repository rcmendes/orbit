use chrono::Utc;
use serde_json::json;

use crate::cli::{AddArgs, DeleteArgs, UpdateArgs};
use crate::graph::LoadMode;
use crate::task::{Status, TaskFrontmatter, TaskType};

use super::*;

pub(super) fn cmd_add(board: &Board, args: AddArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;

    let title = args.title.trim().to_string();
    if title.is_empty() {
        return Err(OrbitError::invalid_argument("title must not be empty"));
    }

    let priority = args.priority.unwrap_or(board.config.board.default_priority);
    validate_priority(priority)?;
    let task_type = args.task_type.unwrap_or(TaskType::Task);
    let labels = normalize_labels(&args.label)?;

    let mut depends_on = Vec::new();
    for dep in args.depends_on {
        if !graph.tasks.contains_key(&dep) {
            return Err(OrbitError::not_found(&dep));
        }
        if !depends_on.contains(&dep) {
            depends_on.push(dep);
        }
    }

    let parent = match args.parent {
        Some(parent_id) => {
            let parent_task = graph.get(&parent_id)?;
            if parent_task.front.status.is_terminal() {
                return Err(OrbitError::invalid_argument(format!(
                    "parent {parent_id} is {}; a SubTask needs an active Parent Task",
                    parent_task.front.status.as_str()
                )));
            }
            if parent_task.front.parent.is_some() {
                return Err(OrbitError::invalid_argument(format!(
                    "parent {parent_id} is a SubTask; SubTasks are one level deep"
                )));
            }
            Some(parent_id)
        }
        None => None,
    };

    if parent.is_some() && task_type == TaskType::Epic {
        return Err(OrbitError::new(
            code::INVALID_TYPE,
            "epic type is only allowed on a top-level Task",
        ));
    }

    if let Some(parent_id) = &parent
        && depends_on.iter().any(|dep| dep == parent_id)
    {
        return Err(OrbitError::invalid_argument(
            "a SubTask cannot depend on its Parent Task; parentage already makes the parent wait",
        ));
    }

    let waiting_on = normalize_waiting(args.waiting_on)?;
    let area = parse_area_option(args.area)?;
    let verification = task::normalize_verification(args.verification)?;
    let body = resolve_body(args.body, args.body_file)?;
    let id = generate_unique_id(board, parent.as_deref())?;
    let now = Utc::now();

    let task = Task {
        id: id.clone(),
        front: TaskFrontmatter {
            title: title.clone(),
            status: Status::Todo,
            task_type,
            priority,
            labels,
            depends_on,
            parent,
            waiting_on,
            checkpoint: None,
            area,
            verification,
            github: None,
            claim: None,
            stray_claim_times: None,
            created_at: now,
            updated_at: now,
            extra: Default::default(),
        },
        body,
    };

    crate::mutation::create_task(board, &task)?;
    graph.insert(task.clone());

    let type_prefix = if task_type == TaskType::Task {
        String::new()
    } else {
        format!("{}  ", task_type.as_str())
    };
    let human = match &task.front.parent {
        Some(parent) => {
            format!("created {id}  P{priority}  {type_prefix}{title}  (subtask of {parent})")
        }
        None => format!("created {id}  P{priority}  {type_prefix}{title}"),
    };
    let warning = crate::mutation::commit(board, &format!("orbit: add {id}"))?;
    Ok(task_output(&graph, &task, human, warning))
}

fn subtasks_exist_error(id: &str, subtasks: &[String]) -> OrbitError {
    OrbitError::new(
        code::SUBTASKS_EXIST,
        format!("{id} has SubTasks: {}", subtasks.join(", ")),
    )
    .with_details(json!({ "task": id, "subtasks": subtasks }))
}

pub(super) fn cmd_update(board: &Board, args: UpdateArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;

    let has_changes = args.title.is_some()
        || args.body.is_some()
        || args.body_file.is_some()
        || args.priority.is_some()
        || args.task_type.is_some()
        || args.waiting_on.is_some()
        || args.clear_waiting
        || args.area.is_some()
        || args.clear_area
        || args.verification.is_some()
        || args.clear_verification;
    if !has_changes {
        return Err(OrbitError::invalid_argument(
            "nothing to update; pass --title, --body, --priority, --type, --area, --verification, or --waiting-on",
        ));
    }

    if let Some(priority) = args.priority {
        validate_priority(priority)?;
    }
    let current = graph.get(&args.id)?;
    let next_type = args.task_type.unwrap_or(current.front.task_type);
    if next_type == TaskType::Epic && current.front.parent.is_some() {
        return Err(OrbitError::new(
            code::INVALID_TYPE,
            "epic type is only allowed on a top-level Task",
        ));
    }
    let body = if args.body.is_some() || args.body_file.is_some() {
        Some(resolve_body(args.body, args.body_file)?)
    } else {
        None
    };
    let waiting_on = normalize_waiting(args.waiting_on)?;
    let area = parse_area_option(args.area)?;
    let verification = task::normalize_verification(args.verification)?;

    let task = {
        let task = graph.get_mut(&args.id)?;
        if let Some(title) = args.title {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err(OrbitError::invalid_argument("title must not be empty"));
            }
            task.front.title = title;
        }
        if let Some(body) = body {
            task.body = body;
        }
        if let Some(priority) = args.priority {
            task.front.priority = priority;
        }
        if let Some(task_type) = args.task_type {
            task.front.task_type = task_type;
        }
        if args.clear_waiting {
            task.front.waiting_on = None;
        } else if waiting_on.is_some() {
            task.front.waiting_on = waiting_on;
        }
        if args.clear_area {
            task.front.area = None;
        } else if let Some(area) = area {
            task.front.area = Some(area);
        }
        if args.clear_verification {
            task.front.verification = None;
        } else if verification.is_some() {
            task.front.verification = verification;
        }
        task.touch(Utc::now());
        task.clone()
    };

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

    graph
        .get(&args.id)?
        .authorize_live(caller.as_deref(), args.force, now)?;

    let child_ids: Vec<String> = graph
        .children(&args.id)
        .iter()
        .map(|task| task.id.clone())
        .collect();
    if !child_ids.is_empty() && !args.force {
        return Err(subtasks_exist_error(&args.id, &child_ids));
    }

    let mut deleted_ids = Vec::new();
    let mut cleaned_ids = Vec::new();
    delete_subtree(
        board,
        &mut graph,
        &args.id,
        args.force,
        now,
        &mut deleted_ids,
        &mut cleaned_ids,
    )?;

    let warning = crate::mutation::commit(board, &format!("orbit: delete {}", args.id))?;
    let subtasks_deleted = deleted_ids.len().saturating_sub(1);
    let human = if subtasks_deleted == 0 {
        format!("deleted {}", args.id)
    } else {
        format!("deleted {} and {} SubTask(s)", args.id, subtasks_deleted)
    };
    let mut output = crate::output::CommandOutput::new(
        json!({
            "id": args.id,
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
        .with_details(json!({ "task": id, "dependents": dependent_ids })));
    }

    if force {
        for dependent in &dependent_ids {
            let updated = {
                let task = graph.get_mut(dependent)?;
                task.remove_dependency(id, now);
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

fn generate_unique_id(board: &Board, parent: Option<&str>) -> Result<String> {
    let config = &board.config.id;
    for attempt in 0..50 {
        let length = if attempt < 40 {
            config.length
        } else {
            (config.length + 2).min(8)
        };
        let id = match parent {
            Some(parent) => task::generate_subtask_id(parent, length),
            None => task::generate_id(&config.prefix, length),
        };
        if !board.task_path(&id).exists() {
            return Ok(id);
        }
    }
    Err(OrbitError::new(
        code::ID_COLLISION,
        "could not generate a unique task id",
    ))
}
