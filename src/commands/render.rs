use serde_json::Value;
use std::collections::BTreeMap;

use crate::comment::Comment;

use super::*;

/// The owned sort key, ordered for keyset pagination:
/// `rank` leads (0 for `list`, the search relevance rank for `search`), then
/// the Parent Task's priority/creation/ID, the subtask flag, and the Task's
/// own priority/creation/ID. `Ord` is the page order; a `--cursor` stores
/// one of these so the next page resumes strictly after it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct SortKey {
    pub rank: i64,
    pub root_priority: usize,
    pub root_created_ms: i64,
    pub root_id: String,
    pub is_subtask: bool,
    pub priority: usize,
    pub created_ms: i64,
    pub id: String,
}

pub(super) fn sort_key(graph: &Graph, task: &Task, rank: i64) -> SortKey {
    let root = task
        .front
        .parent
        .as_deref()
        .and_then(|parent_id| graph.get(parent_id).ok())
        .unwrap_or(task);
    SortKey {
        rank,
        root_priority: graph.priority_ordinal(&root.front.priority),
        root_created_ms: root.front.created_at.timestamp_millis(),
        root_id: root.id.clone(),
        is_subtask: task.front.parent.is_some(),
        priority: graph.priority_ordinal(&task.front.priority),
        created_ms: task.front.created_at.timestamp_millis(),
        id: task.id.clone(),
    }
}

/// The cursor payload: `[rank, root_priority, root_created_ms, root_id,
/// is_subtask, priority, created_ms, id]`.
pub(super) fn key_to_json(key: &SortKey) -> Value {
    serde_json::json!([
        key.rank,
        key.root_priority,
        key.root_created_ms,
        key.root_id,
        key.is_subtask,
        key.priority,
        key.created_ms,
        key.id,
    ])
}

pub(super) fn key_from_json(value: &Value) -> Result<SortKey> {
    let invalid = || {
        OrbitError::invalid_argument(
            "invalid cursor; pass the `next_cursor` value a previous page returned",
        )
    };
    let items = value.as_array().ok_or_else(invalid)?;
    if items.len() != 8 {
        return Err(invalid());
    }
    let number = |index: usize| items[index].as_i64().ok_or_else(invalid);
    let text = |index: usize| {
        items[index]
            .as_str()
            .map(str::to_string)
            .ok_or_else(invalid)
    };
    let flag = |index: usize| items[index].as_bool().ok_or_else(invalid);
    Ok(SortKey {
        rank: number(0)?,
        root_priority: number(1)?.max(0) as usize,
        root_created_ms: number(2)?,
        root_id: text(3)?,
        is_subtask: flag(4)?,
        priority: number(5)?.max(0) as usize,
        created_ms: number(6)?,
        id: text(7)?,
    })
}

pub(super) struct ListGroup<'a> {
    parent: Option<&'a Task>,
    tasks: Vec<&'a Task>,
}

pub(super) fn group_tasks<'a>(graph: &'a Graph, tasks: &[&'a Task]) -> Result<Vec<ListGroup<'a>>> {
    let mut groups: Vec<ListGroup<'a>> = Vec::new();
    let mut positions: BTreeMap<&'a str, usize> = BTreeMap::new();
    let mut standalone: Vec<&'a Task> = Vec::new();

    for task in tasks {
        if let Some(parent_id) = task.front.parent.as_deref() {
            if let Some(position) = positions.get(parent_id) {
                groups[*position].tasks.push(task);
            } else {
                let parent = graph.get(parent_id)?;
                positions.insert(parent_id, groups.len());
                groups.push(ListGroup {
                    parent: Some(parent),
                    tasks: vec![task],
                });
            }
        } else {
            standalone.push(task);
        }
    }

    let banner_ids: Vec<&str> = groups
        .iter()
        .filter_map(|group| group.parent.map(|parent| parent.id.as_str()))
        .collect();
    standalone.retain(|task| !banner_ids.contains(&task.id.as_str()));

    if !standalone.is_empty() {
        groups.push(ListGroup {
            parent: None,
            tasks: standalone,
        });
    }

    Ok(groups)
}

fn group_headers(remote: bool) -> Vec<String> {
    let mut headers: Vec<String> = ["ID", "STATUS", "PRIORITY", "TYPE"]
        .into_iter()
        .map(String::from)
        .collect();
    if remote {
        headers.push("ISSUE".to_string());
    }
    headers.extend(
        ["TITLE", "LABELS", "CLAIMED BY"]
            .into_iter()
            .map(String::from),
    );
    headers
}

fn issue_cell(task: &Task) -> String {
    match crate::remote::link::link_view(task) {
        Some(link) if link.active => format!("#{}", link.id),
        Some(link) => format!("~#{}", link.id),
        None => "-".to_string(),
    }
}

fn task_cells(task: &Task, remote: bool) -> Vec<String> {
    let mut cells: Vec<String> = vec![
        task.display_id().to_string(),
        task.front.status.as_str().to_string(),
        task.front.priority.clone(),
        task.front.task_type.clone(),
    ];
    if remote {
        cells.push(issue_cell(task));
    }
    cells.extend([
        truncate(&task.front.title, 60),
        task.front.labels.join(", "),
        task.assignee_name()
            .map(|name| name.to_string())
            .unwrap_or_default(),
    ]);
    cells
}

fn column_widths(rows: &[Vec<String>]) -> Vec<usize> {
    let columns = rows.first().map(Vec::len).unwrap_or(0);
    (0..columns)
        .map(|column| {
            rows.iter()
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect()
}

pub(super) fn format_table(tasks: &[&Task], remote: bool) -> String {
    let mut rows = vec![group_headers(remote)];
    rows.extend(tasks.iter().map(|task| task_cells(task, remote)));
    let widths = column_widths(&rows);
    rows.iter()
        .map(|row| {
            row.iter()
                .zip(&widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn format_grouped_table(groups: &[ListGroup], remote: bool) -> String {
    let headers = group_headers(remote);
    let mut rows = vec![headers.clone()];
    rows.extend(
        groups
            .iter()
            .flat_map(|group| group.tasks.iter().map(|task| task_cells(task, remote))),
    );
    let widths = column_widths(&rows);
    let inner_width = widths.iter().map(|width| width + 2).sum::<usize>() + widths.len() - 1;

    let mut lines = Vec::new();
    lines.push(column_border("╭", "┬", "╮", &widths));
    lines.push(table_row(&headers, &widths));
    for group in groups {
        lines.push(full_border("├", "┤", inner_width));
        if let Some(parent) = group.parent {
            let left = format!(
                "{}  {}",
                parent.display_id(),
                truncate(&parent.front.title, 60)
            );
            lines.push(banner_row(&left, &group_meta(parent), inner_width));
        } else {
            lines.push(banner_row("No parent", "", inner_width));
        }
        lines.push(full_border("├", "┤", inner_width));
        for task in &group.tasks {
            lines.push(table_row(&task_cells(task, remote), &widths));
        }
    }
    lines.push(column_border("╰", "┴", "╯", &widths));
    lines.join("\n")
}

fn group_meta(task: &Task) -> String {
    let mut parts = vec![
        task.front.task_type.clone(),
        task.front.status.as_str().to_string(),
        task.front.priority.clone(),
    ];
    if !task.front.labels.is_empty() {
        parts.push(task.front.labels.join(", "));
    }
    if let Some(name) = task.assignee_name() {
        parts.push(format!("assigned to {name}"));
    }
    parts.join(" · ")
}

fn column_border(left: &str, middle: &str, right: &str, widths: &[usize]) -> String {
    let segments: Vec<String> = widths.iter().map(|width| "─".repeat(width + 2)).collect();
    format!("{left}{}{right}", segments.join(middle))
}

fn full_border(left: &str, right: &str, inner_width: usize) -> String {
    format!("{left}{}{right}", "─".repeat(inner_width))
}

fn table_row(cells: &[String], widths: &[usize]) -> String {
    let padded: Vec<String> = cells
        .iter()
        .zip(widths)
        .map(|(cell, width)| format!(" {cell:<width$} "))
        .collect();
    format!("│{}│", padded.join("│"))
}

fn banner_row(left: &str, right: &str, inner_width: usize) -> String {
    let available = inner_width.saturating_sub(2);
    let mut left = left.to_string();
    let mut right = right.to_string();

    if right.chars().count() + 1 > available {
        right = truncate(&right, available.saturating_sub(1));
    }

    let max_left = if right.is_empty() {
        available
    } else {
        available.saturating_sub(right.chars().count() + 1)
    };
    if left.chars().count() > max_left {
        left = truncate(&left, max_left);
    }

    let gap = if right.is_empty() {
        available.saturating_sub(left.chars().count())
    } else {
        available
            .saturating_sub(left.chars().count() + right.chars().count())
            .max(1)
    };
    format!("│ {left}{}{right} │", " ".repeat(gap))
}

pub(super) fn push_summary_checkpoint(human: &mut String, task: &Value) {
    let Some(checkpoint) = task.get("checkpoint").filter(|value| !value.is_null()) else {
        return;
    };
    if let Some(progress) = checkpoint["progress"].as_str() {
        human.push_str(&format!("    checkpoint: {progress}\n"));
    }
    if let Some(next) = checkpoint["next"].as_str() {
        human.push_str(&format!("    next: {next}\n"));
    }
    if let Some(blocker) = checkpoint["blocker"].as_str() {
        human.push_str(&format!("    blocker: {blocker}\n"));
    }
}

pub(super) fn format_summary(task: &Task, graph: &Graph) -> String {
    format_summary_with_parent(task, true, graph)
}

fn format_summary_with_parent(task: &Task, include_parent: bool, graph: &Graph) -> String {
    let claim = task
        .assignee_name()
        .map(|name| format!("  assigned to {name}"))
        .unwrap_or_default();
    let labels = if task.front.labels.is_empty() {
        String::new()
    } else {
        format!("  [{}]", task.front.labels.join(", "))
    };
    let type_prefix = if task.front.task_type == graph.hierarchy.default_type() {
        String::new()
    } else {
        format!("{}  ", task.front.task_type)
    };
    let parent = if include_parent {
        task.front
            .parent
            .as_deref()
            .map(|parent| format!("  (subtask of {})", graph.display_id(parent)))
            .unwrap_or_default()
    } else {
        String::new()
    };
    format!(
        "{}  {:<11}  {}  {}{}{}{}{}",
        task.display_id(),
        task.front.status.as_str(),
        task.front.priority,
        type_prefix,
        task.front.title,
        labels,
        claim,
        parent
    )
}

pub(super) fn format_tree_line(task: &Task, graph: &Graph, root: bool) -> String {
    let mut line = format_summary_with_parent(task, false, graph);
    if root {
        let subtask_count = graph.children(&task.id).len();
        if subtask_count > 0 {
            let open_count = graph.open_children(&task.id).len();
            let resolved = subtask_count.saturating_sub(open_count);
            line.push_str(&format!("  {resolved}/{subtask_count} resolved"));
            if graph.eligible_for_close(task) {
                line.push_str("  (ready to close)");
            }
        }
    }
    let blocking: Vec<String> = graph
        .blocked_by(task)
        .iter()
        .map(|id| graph.display_id(id))
        .collect();
    if !blocking.is_empty() {
        line.push_str(&format!("  blocked by: {}", blocking.join(", ")));
    }
    line
}

fn push_field_lines(lines: &mut Vec<String>, label: &str, text: &str) {
    let mut text = text.lines();
    if let Some(first) = text.next() {
        lines.push(format!("  {label}: {first}"));
        for line in text {
            lines.push(format!("    {line}"));
        }
    }
}

pub(super) fn format_detail(
    graph: &Graph,
    task: &Task,
    comments: &[Comment],
    remote: bool,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("{}  {}", task.display_id(), task.front.title));
    if let Some(key) = &task.front.key {
        lines.push(format!("key: {key}"));
        lines.push(format!("id: {}", task.id));
    }
    if !task.front.key_aliases.is_empty() {
        lines.push(format!(
            "key_aliases: {}",
            task.front.key_aliases.join(", ")
        ));
    }
    lines.push(format!("status: {}", task.front.status.as_str()));
    if let Some(resolution) = &task.front.resolution {
        lines.push(format!("resolution: {resolution}"));
    }
    let allowed: Vec<&str> = graph
        .workflow
        .transitions_from(&task.front.status)
        .iter()
        .map(|transition| transition.name.as_str())
        .collect();
    if !allowed.is_empty() {
        lines.push(format!("allowed_transitions: {}", allowed.join(", ")));
    }
    lines.push(format!("type: {}", task.front.task_type));
    if remote {
        lines.push(match crate::remote::link::link_view(task) {
            Some(link) if link.tracker == "github" && link.active => {
                format!("github: {}#{}", link.project, link.id)
            }
            Some(link) if link.tracker == "github" => {
                format!("github: {}#{} (detached)", link.project, link.id)
            }
            Some(link) if link.active => {
                format!("remote: {} {}#{}", link.tracker, link.project, link.id)
            }
            Some(link) => format!(
                "remote: {} {}#{} (detached)",
                link.tracker, link.project, link.id
            ),
            None => "github: -".to_string(),
        });
    }
    lines.push(format!("priority: {}", task.front.priority));
    if let Some(parent) = &task.front.parent {
        lines.push(format!("parent: {}", graph.display_id(parent)));
    }
    if task.front.links.is_empty() {
        lines.push("links: -".to_string());
    } else {
        for link in &task.front.links {
            let target_label = match &link.target {
                crate::task::LinkTarget::Task(id) => graph.display_id(id),
                other => other.label(),
            };
            let mut line = format!(
                "link {} {} ({})",
                link.inward_name(),
                target_label,
                link.kind
            );
            if link.is_blocking() {
                line.push_str(" (blocking)");
            }
            for (key, value) in &link.attributes {
                let rendered = serde_yaml_ng::to_string(value)
                    .map(|text| text.trim().to_string())
                    .unwrap_or_else(|_| "?".to_string());
                line.push_str(&format!(" {key}={rendered}"));
            }
            lines.push(line);
        }
    }
    for (owner, value) in &task.front.properties {
        lines.push(format!("property {owner}: {value}"));
    }
    if let Some(checkpoint) = &task.front.checkpoint {
        let label = if graph.workflow.is_terminal(&task.front.status) {
            "checkpoint (archived)"
        } else {
            "checkpoint"
        };
        lines.push(format!(
            "{label}: {} at {}",
            checkpoint.by,
            checkpoint.at.to_rfc3339()
        ));
        push_field_lines(&mut lines, "progress", &checkpoint.progress);
        if let Some(next) = &checkpoint.next {
            push_field_lines(&mut lines, "next", next);
        }
        if let Some(blocker) = &checkpoint.blocker {
            push_field_lines(&mut lines, "blocker", blocker);
        }
    }
    if let Some(assignee) = &task.front.assignee {
        let kind = assignee.kind.as_str();
        match &task.front.lease {
            Some(lease) => {
                let until = lease
                    .lease_until
                    .map(|until| until.to_rfc3339())
                    .unwrap_or_else(|| "unknown".to_string());
                lines.push(format!(
                    "assignee: {} ({kind}, lease until {until})",
                    assignee.name
                ));
            }
            None => lines.push(format!("assignee: {} ({kind})", assignee.name)),
        }
    }
    if let Some(reporter) = &task.front.reporter {
        lines.push(format!("reporter: {reporter}"));
    }
    lines.push(format!("ready: {}", graph.is_ready(task)));
    let blocking: Vec<String> = graph
        .blocked_by(task)
        .iter()
        .map(|id| graph.display_id(id))
        .collect();
    if !blocking.is_empty() {
        lines.push(format!("blocking: {}", blocking.join(", ")));
    }
    lines.push(format!(
        "created_at: {}",
        task.front.created_at.to_rfc3339()
    ));
    lines.push(format!(
        "updated_at: {}",
        task.front.updated_at.to_rfc3339()
    ));
    let dependents: Vec<String> = graph
        .dependents(&task.id)
        .iter()
        .map(|dependent| graph.display_id(&dependent.id))
        .collect();
    if !dependents.is_empty() {
        lines.push(format!("dependents: {}", dependents.join(", ")));
    }
    let children: Vec<String> = graph
        .children(&task.id)
        .iter()
        .map(|child| graph.display_id(&child.id))
        .collect();
    if !children.is_empty() {
        lines.push(format!("sub-issues: {}", children.join(", ")));
    }
    let open_subtasks: Vec<String> = graph
        .open_children(&task.id)
        .iter()
        .map(|id| graph.display_id(id))
        .collect();
    if !open_subtasks.is_empty() {
        lines.push(format!("open sub-issues: {}", open_subtasks.join(", ")));
    }
    if !comments.is_empty() {
        lines.push(format!("comments: {}", comments.len()));
        for comment in comments {
            lines.push(format!(
                "  [{}] {}",
                comment.created_at.to_rfc3339(),
                comment.author
            ));
            for line in comment.body.lines() {
                lines.push(format!("    {line}"));
            }
        }
    }
    lines.push(String::new());
    lines.push(task.body.clone());
    lines.join("\n")
}
