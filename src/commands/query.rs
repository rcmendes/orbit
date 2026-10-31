use chrono::Utc;
use serde_json::{Value, json};

use crate::cli::{ListArgs, PrimeArgs, ReadyArgs, ShowArgs, TreeArgs};
use crate::git::CommitOutcome;
use crate::graph::LoadMode;
use crate::setup::COMPLETION_LINE;
use crate::task::TaskType;

use super::*;
use super::render::*;

pub(super) fn cmd_list(board: &Board, args: ListArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    if let Some(priority) = args.priority {
        validate_priority(priority)?;
    }
    let label_filters = normalize_label_filters(&args.label, args.repo.as_deref())?;
    let excluded_labels = normalize_labels(&args.exclude_label)?;
    if let Some(parent) = args.parent.as_deref() {
        let parent_task = graph.get(parent)?;
        if parent_task.front.parent.is_some() {
            return Err(OrbitError::invalid_argument(format!(
                "{parent} is a SubTask; `--parent` expects a Parent Task"
            )));
        }
    }

    let mut tasks: Vec<&Task> = graph
        .tasks
        .values()
        .filter(|task| {
            if let Some(status) = args.status {
                if task.front.status != status {
                    return false;
                }
            } else if !args.all && task.front.status.is_terminal() {
                return false;
            }

            if let Some(priority) = args.priority
                && task.front.priority != priority
            {
                return false;
            }
            if let Some(claimed_by) = &args.claimed_by
                && task.holder() != Some(claimed_by.as_str())
            {
                return false;
            }
            if !args.task_type.is_empty() && !args.task_type.contains(&task.front.task_type) {
                return false;
            }
            if args.exclude_type.contains(&task.front.task_type) {
                return false;
            }
            if !args.area.is_empty() && !args.area.iter().any(|area| task.front.area == Some(*area))
            {
                return false;
            }
            if args
                .exclude_area
                .iter()
                .any(|area| task.front.area == Some(*area))
            {
                return false;
            }
            if !task.has_labels(&label_filters) {
                return false;
            }
            if task.has_any_label(&excluded_labels) {
                return false;
            }
            if args.unlabeled && !task.front.labels.is_empty() {
                return false;
            }
            if args.linked && task.github_link().is_none() {
                return false;
            }
            if args.local && task.github_link().is_some() {
                return false;
            }
            if let Some(parent) = args.parent.as_deref()
                && task.front.parent.as_deref() != Some(parent)
            {
                return false;
            }
            !args.waiting || task.front.waiting_on.is_some()
        })
        .collect();

    tasks.sort_by(|left, right| list_sort_key(&graph, left).cmp(&list_sort_key(&graph, right)));

    let summaries: Vec<Value> = tasks
        .iter()
        .map(|task| graph::task_summary(&graph, task))
        .collect();
    let count = summaries.len();
    let human = if tasks.is_empty() {
        "no tasks".to_string()
    } else if args.group {
        format_grouped_table(&group_tasks(&graph, &tasks)?)
    } else {
        format_table(&tasks)
    };

    Ok(
        crate::output::CommandOutput::new(json!({ "tasks": summaries, "count": count }), human)
            .with_meta(json!({ "count": count })),
    )
}

pub(super) fn cmd_ready(board: &Board, args: ReadyArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let label_filters = normalize_label_filters(&args.label, args.repo.as_deref())?;
    let excluded_labels = normalize_labels(&args.exclude_label)?;
    if let Some(parent) = args.parent.as_deref() {
        let parent_task = graph.get(parent)?;
        if parent_task.front.parent.is_some() {
            return Err(OrbitError::invalid_argument(format!(
                "{parent} is a SubTask; `--parent` expects a Parent Task"
            )));
        }
    }

    let filters = ReadyFilters {
        parent: args.parent.as_deref(),
        labels: &label_filters,
        excluded: &excluded_labels,
        types: &args.task_type,
        excluded_types: &args.exclude_type,
        areas: &args.area,
        excluded_areas: &args.exclude_area,
    };
    if args.claim {
        let agent = resolve_agent(args.by)?;
        let ready = ready_for_filters(graph.ready_tasks(), filters);
        if ready.is_empty() {
            return Ok(
                crate::output::CommandOutput::new(Value::Null, "no ready tasks")
                    .with_meta(json!({ "reason": "NO_READY_TASKS" })),
            );
        }

        let id = ready[0].id.clone();
        let result = crate::mutation::execute_in(
            board,
            graph,
            &id,
            crate::mutation::Lifecycle::Claim {
                agent: &agent,
                steal: false,
            },
            Utc::now(),
        )?;
        let lease = result
            .task
            .lease_until()
            .map(|until| until.to_rfc3339())
            .unwrap_or_default();
        let mut output = mutation_output(result, format!("claimed {id} by {agent} until {lease}"));
        output.meta = Some(json!({ "claimed": true }));
        return Ok(output);
    }

    let mut ready = ready_for_filters(graph.ready_tasks(), filters);
    if let Some(limit) = args.limit {
        ready.truncate(limit);
    }

    let summaries: Vec<Value> = ready
        .iter()
        .map(|task| graph::task_summary(&graph, task))
        .collect();
    let count = summaries.len();
    let human = if ready.is_empty() {
        "no ready tasks".to_string()
    } else {
        ready
            .iter()
            .map(format_summary)
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(
        crate::output::CommandOutput::new(json!({ "tasks": summaries, "count": count }), human)
            .with_meta(json!({ "count": count })),
    )
}

pub(super) fn cmd_show(board: &Board, args: ShowArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let task = graph.get(&args.id)?;
    let comments = board.read_comments(&args.id)?;
    Ok(crate::output::CommandOutput::new(
        graph::task_detail_with_comments(&graph, task, &comments),
        format_detail(&graph, task, &comments),
    ))
}

pub(super) fn cmd_tree(board: &Board, args: TreeArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let root = graph.get(&args.id)?;
    if root.front.parent.is_some() {
        return Err(OrbitError::invalid_argument(format!(
            "{} is a SubTask; `orbit tree` expects a Parent Task",
            root.id
        )));
    }

    let children = graph.children(&root.id);
    let mut tasks = Vec::with_capacity(children.len() + 1);
    tasks.push(root.clone());
    tasks.extend(children.into_iter().cloned());

    let summaries: Vec<Value> = tasks
        .iter()
        .map(|task| graph::task_summary(&graph, task))
        .collect();
    let count = summaries.len();

    let mut human = format_tree_line(root, &graph, true);
    for child in &tasks[1..] {
        human.push('\n');
        human.push_str(&format_tree_line(child, &graph, false));
    }

    Ok(crate::output::CommandOutput::new(
        json!({ "root": root.id, "tasks": summaries, "count": count }),
        human,
    )
    .with_meta(json!({ "count": count })))
}

pub(super) fn cmd_prime(board: &Board, args: PrimeArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let label_filters = normalize_label_filters(&args.label, args.repo.as_deref())?;
    let counts = graph.counts_matching(&label_filters);
    let agent = optional_agent(args.by).unwrap_or_else(|| "unknown (set ORBIT_AGENT)".to_string());
    let instructions = format!(
        "Orbit is the task tracker for this repository.\n\
Use the CLI; never edit task files directly.\n\
- `orbit ready --claim --by <agent>` takes one ready task.\n\
- `orbit heartbeat <id> --by <agent>` extends the lease while working.\n\
- `orbit checkpoint <id> --by <agent> --progress \"...\"` leaves a handoff that survives release and takeover.\n\
{COMPLETION_LINE}\n\
- `orbit add \"title\" -p N --depends-on <id>` creates work.\n\
- `orbit depend <id> <dep-id>` links work; cycles and unfinished dependencies are rejected.\n\
- A Parent Task whose SubTasks are all resolved appears under close candidates; close it explicitly when the family is done.\n\
- `orbit show <id>` reads the full body."
    );

    let ready: Vec<Value> = graph
        .ready_tasks()
        .into_iter()
        .filter(|task| task.has_labels(&label_filters))
        .take(10)
        .map(|task| graph::task_summary(&graph, &task))
        .collect();
    let close_candidates: Vec<Value> = graph
        .close_candidates()
        .into_iter()
        .filter(|task| task.has_labels(&label_filters))
        .take(10)
        .map(|task| graph::task_summary(&graph, task))
        .collect();
    let stale: Vec<Value> = graph
        .stale_tasks()
        .into_iter()
        .filter(|task| task.has_labels(&label_filters))
        .take(10)
        .map(|task| graph::task_summary(&graph, task))
        .collect();

    let mut human = String::new();
    human.push_str(&instructions);
    human.push_str("\n\n");
    human.push_str(&format!("agent: {agent}\n"));
    if !label_filters.is_empty() {
        human.push_str(&format!("scope: {}\n", label_filters.join(", ")));
    }
    human.push_str(&format!(
        "counts: total={} ready={} stale={} in_progress={} waiting={} blocked={}\n",
        counts.total,
        counts.ready,
        counts.stale,
        counts.in_progress,
        counts.waiting,
        counts.blocked
    ));
    human.push_str("ready:\n");
    if ready.is_empty() {
        human.push_str("  (none)\n");
    } else {
        for task in &ready {
            human.push_str(&format!("  {}\n", format_summary_value(task)));
        }
    }
    human.push_str("close candidates:\n");
    if close_candidates.is_empty() {
        human.push_str("  (none)\n");
    } else {
        for task in &close_candidates {
            human.push_str(&format!(
                "  {}  (ready to close)\n",
                format_summary_value(task)
            ));
        }
    }
    human.push_str("stale claims:\n");
    if stale.is_empty() {
        human.push_str("  (none)\n");
    } else {
        for task in &stale {
            human.push_str(&format!("  {}\n", format_summary_value(task)));
            push_summary_checkpoint(&mut human, task);
        }
    }

    let mut output = crate::output::CommandOutput::new(
        json!({
            "agent": agent,
            "instructions": instructions,
            "labels": label_filters,
            "counts": counts,
            "ready": ready,
            "close_candidates": close_candidates,
            "stale": stale,
        }),
        human,
    );

    if args.hook_json {
        let hook = json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": output.human,
            }
        });
        output.raw_output = Some(serde_json::to_string(&hook).map_err(|error| {
            OrbitError::config(format!("serialize prime hook output: {error}"))
        })?);
    }

    Ok(output)
}

pub(super) fn cmd_export(board: &Board) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let mut ids: Vec<&String> = graph.tasks.keys().collect();
    ids.sort();

    let mut tasks = Vec::with_capacity(ids.len());
    let mut comment_count = 0;
    for id in ids {
        let comments = board.read_comments(id)?;
        comment_count += comments.len();
        let task = graph.get(id)?;
        tasks.push(graph::task_detail_with_comments(&graph, task, &comments));
    }

    let task_count = tasks.len();
    let snapshot = json!({
        "schema_version": 2,
        "board": {
            "branch": board.config.board.branch,
            "prefix": board.config.id.prefix,
            "task_count": task_count,
            "comment_count": comment_count,
        },
        "tasks": tasks,
    });
    Ok(crate::output::CommandOutput::new(
        snapshot,
        format!("exported {task_count} tasks and {comment_count} comments (schema 2)"),
    ))
}

pub(super) fn cmd_sync(board: &Board) -> Result<crate::output::CommandOutput> {
    match board.commit("orbit: sync", true)? {
        CommitOutcome::Committed => Ok(crate::output::CommandOutput::new(
            json!({ "committed": true }),
            "committed pending board changes",
        )),
        CommitOutcome::NoChanges => Ok(crate::output::CommandOutput::new(
            json!({ "committed": false }),
            "no board changes to commit",
        )),
        CommitOutcome::Skipped => Ok(crate::output::CommandOutput::new(
            json!({ "committed": false }),
            "auto-commit is disabled",
        )),
        CommitOutcome::Failed(message) => Err(OrbitError::git(message)),
    }
}

#[derive(Clone, Copy)]
struct ReadyFilters<'a> {
    parent: Option<&'a str>,
    labels: &'a [String],
    excluded: &'a [String],
    types: &'a [TaskType],
    excluded_types: &'a [TaskType],
    areas: &'a [TaskArea],
    excluded_areas: &'a [TaskArea],
}

fn ready_for_filters(mut ready: Vec<Task>, filters: ReadyFilters<'_>) -> Vec<Task> {
    let ReadyFilters {
        parent,
        labels,
        excluded,
        types,
        excluded_types,
        areas,
        excluded_areas,
    } = filters;
    if let Some(parent) = parent {
        ready.retain(|task| task.front.parent.as_deref() == Some(parent));
    }
    if !labels.is_empty() {
        ready.retain(|task| task.has_labels(labels));
    }
    if !excluded.is_empty() {
        ready.retain(|task| !task.has_any_label(excluded));
    }
    if !types.is_empty() {
        ready.retain(|task| types.contains(&task.front.task_type));
    }
    if !excluded_types.is_empty() {
        ready.retain(|task| !excluded_types.contains(&task.front.task_type));
    }
    if !areas.is_empty() {
        ready.retain(|task| task.front.area.is_some_and(|area| areas.contains(&area)));
    }
    if !excluded_areas.is_empty() {
        ready.retain(|task| {
            !task
                .front
                .area
                .is_some_and(|area| excluded_areas.contains(&area))
        });
    }
    ready
}
