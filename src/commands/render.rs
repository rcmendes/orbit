use serde_json::Value;
use std::collections::BTreeMap;

use crate::comment::Comment;
use crate::task::TaskType;

use super::*;

pub(super) fn list_sort_key<'a>(
    graph: &'a Graph,
    task: &'a Task,
) -> (u8, i64, &'a str, bool, u8, i64, &'a str) {
    let root = task
        .front
        .parent
        .as_deref()
        .and_then(|parent_id| graph.get(parent_id).ok())
        .unwrap_or(task);
    (
        root.front.priority,
        root.front.created_at.timestamp_millis(),
        root.id.as_str(),
        task.front.parent.is_some(),
        task.front.priority,
        task.front.created_at.timestamp_millis(),
        task.id.as_str(),
    )
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

fn group_headers() -> [String; 8] {
    [
        "ID",
        "STATUS",
        "P",
        "TYPE",
        "ISSUE",
        "TITLE",
        "LABELS",
        "CLAIMED BY",
    ]
    .map(String::from)
}

fn issue_cell(task: &Task) -> String {
    match &task.front.github {
        Some(link) if link.active => format!("#{}", link.number),
        Some(link) => format!("~#{}", link.number),
        None => "-".to_string(),
    }
}

fn task_cells(task: &Task) -> [String; 8] {
    [
        task.id.to_string(),
        task.front.status.as_str().to_string(),
        format!("P{}", task.front.priority),
        task.front.task_type.as_str().to_string(),
        issue_cell(task),
        truncate(&task.front.title, 60),
        task.front.labels.join(", "),
        task.holder()
            .map(|holder| holder.to_string())
            .unwrap_or_default(),
    ]
}

fn column_widths(rows: &[[String; 8]]) -> Vec<usize> {
    (0..8)
        .map(|column| {
            rows.iter()
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect()
}

pub(super) fn format_table(tasks: &[&Task]) -> String {
    let mut rows = vec![group_headers()];
    rows.extend(tasks.iter().map(|task| task_cells(task)));
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

pub(super) fn format_grouped_table(groups: &[ListGroup]) -> String {
    let headers = group_headers();
    let mut rows = vec![headers.clone()];
    rows.extend(
        groups
            .iter()
            .flat_map(|group| group.tasks.iter().map(|task| task_cells(task))),
    );
    let widths = column_widths(&rows);
    let inner_width = widths.iter().map(|width| width + 2).sum::<usize>() + widths.len() - 1;

    let mut lines = Vec::new();
    lines.push(column_border("╭", "┬", "╮", &widths));
    lines.push(table_row(&headers, &widths));
    for group in groups {
        lines.push(full_border("├", "┤", inner_width));
        if let Some(parent) = group.parent {
            let left = format!("{}  {}", parent.id, truncate(&parent.front.title, 60));
            lines.push(banner_row(&left, &group_meta(parent), inner_width));
        } else {
            lines.push(banner_row("No parent", "", inner_width));
        }
        lines.push(full_border("├", "┤", inner_width));
        for task in &group.tasks {
            lines.push(table_row(&task_cells(task), &widths));
        }
    }
    lines.push(column_border("╰", "┴", "╯", &widths));
    lines.join("\n")
}

fn group_meta(task: &Task) -> String {
    let mut parts = vec![
        task.front.task_type.as_str().to_string(),
        task.front.status.as_str().to_string(),
        format!("P{}", task.front.priority),
    ];
    if !task.front.labels.is_empty() {
        parts.push(task.front.labels.join(", "));
    }
    if let Some(holder) = task.holder() {
        parts.push(format!("claimed by {holder}"));
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

fn table_row(cells: &[String; 8], widths: &[usize]) -> String {
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

pub(super) fn format_summary(task: &Task) -> String {
    format_summary_with_parent(task, true)
}

fn format_summary_with_parent(task: &Task, include_parent: bool) -> String {
    let claim = task
        .holder()
        .map(|holder| format!("  claimed by {holder}"))
        .unwrap_or_default();
    let labels = if task.front.labels.is_empty() {
        String::new()
    } else {
        format!("  [{}]", task.front.labels.join(", "))
    };
    let type_prefix = if task.front.task_type == TaskType::Task {
        String::new()
    } else {
        format!("{}  ", task.front.task_type.as_str())
    };
    let parent = if include_parent {
        task.front
            .parent
            .as_deref()
            .map(|parent| format!("  (subtask of {parent})"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    format!(
        "{}  {:<11}  P{}  {}{}{}{}{}",
        task.id,
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
    let mut line = format_summary_with_parent(task, false);
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
    let blocking = graph.blocked_by(task);
    if !blocking.is_empty() {
        line.push_str(&format!("  blocked by: {}", blocking.join(", ")));
    }
    if root { line } else { format!("  {line}") }
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

pub(super) fn format_detail(graph: &Graph, task: &Task, comments: &[Comment]) -> String {
    let mut lines = Vec::new();
    lines.push(format!("{}  {}", task.id, task.front.title));
    lines.push(format!("status: {}", task.front.status.as_str()));
    lines.push(format!("kind: {}", task.kind()));
    lines.push(format!("type: {}", task.front.task_type.as_str()));
    lines.push(match &task.front.github {
        Some(link) if link.active => format!("github: {}#{}", link.repo, link.number),
        Some(link) => format!("github: {}#{} (detached)", link.repo, link.number),
        None => "github: -".to_string(),
    });
    lines.push(format!("priority: P{}", task.front.priority));
    if let Some(parent) = &task.front.parent {
        lines.push(format!("parent: {parent}"));
    }
    if task.front.depends_on.is_empty() {
        lines.push("depends_on: -".to_string());
    } else {
        lines.push(format!("depends_on: {}", task.front.depends_on.join(", ")));
    }
    if let Some(waiting) = &task.front.waiting_on {
        lines.push(format!("waiting_on: {waiting}"));
    }
    if let Some(checkpoint) = &task.front.checkpoint {
        let label = if task.front.status.is_terminal() {
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
    if let Some(claim) = &task.front.claim {
        let until = claim
            .lease_until
            .map(|until| until.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!("claimed_by: {} (until {until})", claim.holder));
    }
    lines.push(format!("ready: {}", graph.is_ready(task)));
    let blocking = graph.blocked_by(task);
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
    let dependents: Vec<&str> = graph
        .dependents(&task.id)
        .iter()
        .map(|dependent| dependent.id.as_str())
        .collect();
    if !dependents.is_empty() {
        lines.push(format!("dependents: {}", dependents.join(", ")));
    }
    let children: Vec<&str> = graph
        .children(&task.id)
        .iter()
        .map(|child| child.id.as_str())
        .collect();
    if !children.is_empty() {
        lines.push(format!("subtasks: {}", children.join(", ")));
    }
    let open_subtasks = graph.open_children(&task.id);
    if !open_subtasks.is_empty() {
        lines.push(format!("open_subtasks: {}", open_subtasks.join(", ")));
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
