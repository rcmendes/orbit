use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::{Value, json};

use crate::cli::{DigestArgs, ListArgs, PrimeArgs, ReadyArgs, SearchArgs, ShowArgs, TreeArgs};
use crate::git::CommitOutcome;
use crate::graph::LoadMode;
use crate::setup::COMPLETION_LINE;
use crate::task::IdentityKind;

use super::render::*;
use super::*;

/// `--parent` names a Task that must exist; any Task may be a parent
/// filter, at any depth of the Board hierarchy.
fn check_parent_filter(graph: &Graph, parent: &str) -> Result<String> {
    // Resolve keys and aliases: Tasks store the parent by internal id.
    Ok(graph.get(parent)?.id.clone())
}

pub(super) fn cmd_list(board: &Board, mut args: ListArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = crate::index::load(board, LoadMode::Strict)?;
    if let Some(parent) = args.parent.as_deref() {
        args.parent = Some(check_parent_filter(&graph, parent)?);
    }
    args.task_type = normalize_type_filters(&args.task_type);
    args.exclude_type = normalize_type_filters(&args.exclude_type);
    let priority = args
        .priority
        .as_deref()
        .map(|priority| board.config.board.validate_priority(priority))
        .transpose()?;
    let json_filter = args
        .filter
        .as_deref()
        .map(crate::search::parse_filter)
        .transpose()?;
    let fields = args
        .fields
        .as_deref()
        .map(crate::search::parse_fields)
        .transpose()?;
    let label_filters = normalize_labels(&args.label)?;
    let repository_filter = normalize_repository_filter(args.repository.as_deref())?;
    let excluded_labels = normalize_labels(&args.exclude_label)?;
    let components = board.config.board.normalize_components(&args.component)?;
    let excluded_components = board
        .config
        .board
        .normalize_components(&args.exclude_component)?;
    let scope = repository_scope(board, args.all)?;
    if let Some(status) = args.status.as_deref()
        && graph.workflow.status(status).is_none()
    {
        return Err(OrbitError::invalid_argument(format!(
            "unknown status `{status}`; the workflow defines: {}",
            graph.workflow.status_names().join(", ")
        )));
    }

    let tasks: Vec<&Task> = graph
        .tasks
        .values()
        .filter(|task| {
            if let Some(status) = args.status.as_deref() {
                if task.front.status != status {
                    return false;
                }
            } else if !args.include_closed && graph.workflow.is_terminal(&task.front.status) {
                return false;
            }

            if let Some(scope) = scope.as_deref()
                && task
                    .front
                    .repository
                    .as_deref()
                    .is_some_and(|repo| repo != scope)
            {
                return false;
            }
            if let Some(priority) = &priority
                && task.front.priority != *priority
            {
                return false;
            }
            if let Some(assignee) = &args.assignee
                && task.assignee_name() != Some(assignee.as_str())
            {
                return false;
            }
            if !args.task_type.is_empty() && !args.task_type.contains(&task.front.task_type) {
                return false;
            }
            if args.exclude_type.contains(&task.front.task_type) {
                return false;
            }
            if !components.is_empty()
                && !components
                    .iter()
                    .any(|component| task.front.component.as_deref() == Some(component.as_str()))
            {
                return false;
            }
            if excluded_components
                .iter()
                .any(|component| task.front.component.as_deref() == Some(component.as_str()))
            {
                return false;
            }
            if !matches_repository(task, repository_filter.as_deref())
                || !task.has_labels(&label_filters)
            {
                return false;
            }
            if task.has_any_label(&excluded_labels) {
                return false;
            }
            if args.unlabeled && !task.front.labels.is_empty() {
                return false;
            }
            if args.linked && crate::remote::link::active_link(task).is_none() {
                return false;
            }
            if args.local && crate::remote::link::active_link(task).is_some() {
                return false;
            }
            if let Some(parent) = args.parent.as_deref()
                && task.front.parent.as_deref() != Some(parent)
            {
                return false;
            }
            !args.waiting || task.external_blockers().next().is_some()
        })
        .collect();

    // `--filter`/`--fields` names are validated against the summary schema
    // before matching, so a typo fails even when nothing would match.
    // An empty Board has no Task to read the summary schema from; there is
    // nothing to filter or project, so skip validation rather than reject
    // every valid field name.
    if let Some(any_task) = graph.tasks.values().next() {
        let schema_keys = crate::search::filter_field_names(&graph::task_summary(&graph, any_task));
        if let Some(filter) = &json_filter {
            crate::search::validate_filter_fields(filter, &schema_keys)?;
        }
        if let Some(fields) = &fields {
            validate_field_roots(fields, &schema_keys)?;
        }
    }

    // Decorate-sort-undecorate: the page order is the SortKey order, which
    // is also what `--cursor` resumes.
    let mut keyed: Vec<(SortKey, &Task)> = tasks
        .iter()
        .map(|task| (sort_key(&graph, task, 0), *task))
        .collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));

    let index = graph.summary_index();
    // Summaries are costly (descendants, readiness): build them for every Task
    // only when `--filter` needs them, otherwise for the page alone.
    let mut filtered: Vec<(SortKey, &Task, Value)> = Vec::with_capacity(keyed.len());
    for (key, task) in keyed {
        let summary = match &json_filter {
            Some(filter) => {
                let summary = graph::task_summary_in(&graph, task, &index);
                if !crate::search::matches_filter(&summary, filter) {
                    continue;
                }
                summary
            }
            None => Value::Null,
        };
        filtered.push((key, task, summary));
    }

    let scope = {
        let mut scoped = args.clone();
        scoped.cursor = None;
        scoped.limit = None;
        format!("list|{scoped:?}")
    };
    let (page, next_cursor, total) =
        paginate(filtered, args.cursor.as_deref(), args.limit, &scope)?;
    let count = page.len();

    let mut summaries: Vec<Value> = page
        .iter()
        .map(|(_, task, summary)| {
            if summary.is_null() {
                graph::task_summary_in(&graph, task, &index)
            } else {
                summary.clone()
            }
        })
        .collect();
    if let Some(fields) = &fields {
        let mut array = Value::Array(summaries);
        crate::search::project_fields(&mut array, fields);
        summaries = match array {
            Value::Array(items) => items,
            _ => unreachable!(),
        };
    }

    let remote = crate::remote::details_visible(board, &graph);
    let page_tasks: Vec<&Task> = page.iter().map(|(_, task, _)| *task).collect();
    let human = if page_tasks.is_empty() {
        "no tasks".to_string()
    } else if args.group {
        format_grouped_table(&group_tasks(&graph, &page_tasks)?, remote)
    } else {
        format_table(&page_tasks, remote)
    };

    Ok(crate::output::CommandOutput::new(
        json!({
            "issues": summaries,
            "count": count,
            "total": total,
            "next_cursor": next_cursor,
        }),
        human,
    )
    .with_meta(json!({ "count": count })))
}

pub(super) fn cmd_ready(
    board: &Board,
    mut args: ReadyArgs,
    if_version: Option<&str>,
) -> Result<crate::output::CommandOutput> {
    if args.limit == Some(0) {
        return Err(OrbitError::invalid_argument("--limit must be at least 1"));
    }
    // `ready --claim` mutates: it keeps the authoritative load. A plain
    // `ready` listing is a read and may use the derived index.
    let (graph, _) = if args.claim {
        Graph::load(board, LoadMode::Strict)?
    } else {
        crate::index::load(board, LoadMode::Strict)?
    };
    if let Some(parent) = args.parent.as_deref() {
        args.parent = Some(check_parent_filter(&graph, parent)?);
    }
    args.task_type = normalize_type_filters(&args.task_type);
    args.exclude_type = normalize_type_filters(&args.exclude_type);
    let label_filters = normalize_labels(&args.label)?;
    let repository_filter = normalize_repository_filter(args.repository.as_deref())?;
    let excluded_labels = normalize_labels(&args.exclude_label)?;
    let components = board.config.board.normalize_components(&args.component)?;
    let excluded_components = board
        .config
        .board
        .normalize_components(&args.exclude_component)?;
    let scope = repository_scope(board, args.all)?;

    let filters = ReadyFilters {
        parent: args.parent.as_deref(),
        labels: &label_filters,
        excluded: &excluded_labels,
        types: &args.task_type,
        excluded_types: &args.exclude_type,
        components: &components,
        excluded_components: &excluded_components,
        scope: scope.as_deref(),
        repository: repository_filter.as_deref(),
    };
    let strategies = crate::scheduler::resolve(args.strategy.as_deref(), &board.config.scheduler)?;
    if args.claim {
        let agent = resolve_agent(args.by)?;
        let ready = ready_for_filters(graph.ready_tasks(), filters);
        let outcome = crate::scheduler::apply(
            &graph,
            ready,
            &strategies,
            Some(&agent),
            true,
            &board.config.scheduler,
        )?;
        if outcome.refusal == Some(crate::scheduler::Refusal::WipLimit) {
            let limit = board.config.scheduler.wip_limit.unwrap_or(0);
            return Ok(crate::output::CommandOutput::new(
                Value::Null,
                format!(
                    "{agent} already holds {limit} task(s); the scheduler WIP limit is reached"
                ),
            )
            .with_meta(json!({ "reason": "WIP_LIMIT" })));
        }
        let ready = outcome.tasks;
        if ready.is_empty() {
            return Ok(
                crate::output::CommandOutput::new(Value::Null, "no ready tasks")
                    .with_meta(json!({ "reason": "NO_READY_TASKS" })),
            );
        }

        // `--if-version` guards the Task `ready` picked: the caller read its
        // `updated_at` from `orbit show`/`list` and the pick must be
        // unchanged since. Checked inside the same exclusive scope.
        if let Some(expected) = if_version {
            let expected_at = crate::concurrency::parse_version(expected)?;
            let task = graph.get(&ready[0].id)?;
            if task.front.updated_at != expected_at {
                return Err(crate::concurrency::version_conflict(
                    &ready[0].id,
                    expected_at,
                    task.front.updated_at,
                ));
            }
        }
        // A Task a claim policy refuses must not dead-end the queue: try the next
        // candidate and surface the first refusal only when none can be claimed.
        let mut first_refusal = None;
        let mut claimed = None;
        let mut first_graph = Some(graph);
        for candidate in &ready {
            let id = candidate.id.clone();
            // `execute_in` consumes the graph; a refused attempt restores nothing
            // on disk, so later candidates reload the authoritative state.
            let attempt_graph = match first_graph.take() {
                Some(graph) => graph,
                None => Graph::load(board, LoadMode::Strict)?.0,
            };
            match crate::mutation::execute_in(
                board,
                attempt_graph,
                &id,
                crate::mutation::Lifecycle::Claim {
                    agent: &agent,
                    kind: IdentityKind::Agent,
                    steal: false,
                },
                Utc::now(),
            ) {
                Ok(result) => {
                    claimed = Some((id, result));
                    break;
                }
                Err(error) if error.code == code::POLICY_VIOLATION && if_version.is_none() => {
                    first_refusal.get_or_insert(error);
                }
                Err(error) => return Err(error),
            }
        }
        let Some((id, result)) = claimed else {
            return Err(first_refusal.expect("ready is non-empty"));
        };
        let lease = result
            .task
            .lease_until()
            .map(|until| until.to_rfc3339())
            .unwrap_or_default();
        let mut output = mutation_output(result, format!("claimed {id} by {agent} until {lease}"));
        output.meta = Some(json!({ "claimed": true }));
        return Ok(output);
    }

    let ready = ready_for_filters(graph.ready_tasks(), filters);
    let mut ready = crate::scheduler::apply(
        &graph,
        ready,
        &strategies,
        None,
        false,
        &board.config.scheduler,
    )?
    .tasks;
    if let Some(limit) = args.limit {
        ready.truncate(limit);
    }

    let index = graph.summary_index();
    let summaries: Vec<Value> = ready
        .iter()
        .map(|task| graph::task_summary_in(&graph, task, &index))
        .collect();
    let count = summaries.len();
    let human = if ready.is_empty() {
        "no ready tasks".to_string()
    } else {
        ready
            .iter()
            .map(|task| format_summary(task, &graph))
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(
        crate::output::CommandOutput::new(json!({ "issues": summaries, "count": count }), human)
            .with_meta(json!({ "count": count })),
    )
}

/// One row of a paginated query result: the stable sort key, the Task,
/// and its projected summary JSON.
type PagedRow<'a> = (SortKey, &'a Task, Value);

/// Applies the keyset cursor and page size to an already-sorted result set.
/// Returns the page, the `next_cursor` token (when more items remain), and
/// the total number of matches before the cursor.
fn paginate<'a>(
    mut items: Vec<PagedRow<'a>>,
    cursor: Option<&str>,
    limit: Option<usize>,
    scope: &str,
) -> Result<(Vec<PagedRow<'a>>, Option<String>, usize)> {
    use sha2::{Digest, Sha256};
    // A cursor is only valid for the query that produced it.
    let scope: String = Sha256::digest(scope.as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if let Some(limit) = limit
        && limit == 0
    {
        return Err(OrbitError::invalid_argument("--limit must be at least 1"));
    }
    let total = items.len();
    if let Some(token) = cursor {
        let payload = crate::search::decode_cursor(token)?;
        if payload.get("s").and_then(Value::as_str) != Some(scope.as_str()) {
            return Err(OrbitError::invalid_argument(
                "this cursor belongs to a different query; pass the `next_cursor` of the same command and filters",
            ));
        }
        let after = key_from_json(payload.get("k").unwrap_or(&Value::Null))?;
        items.retain(|(key, ..)| key > &after);
    }
    let mut next_cursor = None;
    if let Some(limit) = limit
        && items.len() > limit
    {
        items.truncate(limit);
        if let Some((key, ..)) = items.last() {
            next_cursor = Some(crate::search::encode_cursor(
                &json!({ "k": key_to_json(key), "s": scope }),
            ));
        }
    }
    Ok((items, next_cursor, total))
}

/// Validates every `--fields` root against the Issue summary schema.
fn validate_field_roots(fields: &[Vec<String>], known: &[String]) -> Result<()> {
    for root in crate::search::field_root_names(fields) {
        if !known.iter().any(|name| name == &root) {
            let mut names = known.to_vec();
            names.sort();
            return Err(OrbitError::invalid_argument(format!(
                "unknown --fields entry `{root}`; valid fields: {}",
                names.join(", ")
            )));
        }
    }
    Ok(())
}

/// Parsed `--filter`/`--fields` pair shared by `list` and `search`.
type QueryOptions = (Option<Value>, Option<Vec<Vec<String>>>);

/// `--filter` + `--fields` shared by `list` and `search`: parses both
/// options up front so a bad value fails before any Board scan.
fn query_options(filter: Option<&str>, fields: Option<&str>) -> Result<QueryOptions> {
    let filter = filter.map(crate::search::parse_filter).transpose()?;
    let fields = fields.map(crate::search::parse_fields).transpose()?;
    Ok((filter, fields))
}

pub(super) fn cmd_show(board: &Board, args: ShowArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = crate::index::load(board, LoadMode::Strict)?;
    let task = graph.get(&args.id)?;
    let comments = board.read_comments(&task.id)?;
    let remote = crate::remote::details_visible(board, &graph);
    let mut data = graph::task_detail_with_comments(&graph, task, &comments);
    if let Some(fields) = args.fields.as_deref() {
        let fields = crate::search::parse_fields(fields)?;
        crate::search::validate_field_names(&fields, &[&data])?;
        crate::search::project_fields(&mut data, &fields);
    }
    Ok(crate::output::CommandOutput::new(
        data,
        format_detail(&graph, task, &comments, remote),
    ))
}

pub(super) fn cmd_tree(board: &Board, args: TreeArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = crate::index::load(board, LoadMode::Strict)?;
    let root = graph.get(&args.id)?;
    if root.front.parent.is_some() && graph.children(&root.id).is_empty() {
        return Err(OrbitError::invalid_argument(format!(
            "{} is a SubTask; `orbit tree` expects a Parent Task",
            root.id
        )));
    }

    // Depth-first preorder: the root, then every descendant level by level,
    // each SubTask listed before its own SubTasks.
    fn visit<'a>(
        graph: &'a Graph,
        task: &'a Task,
        level: usize,
        seen: &mut std::collections::HashSet<String>,
        entries: &mut Vec<(&'a Task, usize)>,
    ) {
        entries.push((task, level));
        for child in graph.children(&task.id) {
            if seen.insert(child.id.clone()) {
                visit(graph, child, level + 1, seen, entries);
            }
        }
    }
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::from([root.id.clone()]);
    visit(&graph, root, 0, &mut seen, &mut entries);

    let tasks: Vec<&Task> = entries.iter().map(|(task, _)| *task).collect();
    let index = graph.summary_index();
    let summaries: Vec<Value> = tasks
        .iter()
        .map(|task| graph::task_summary_in(&graph, task, &index))
        .collect();
    let count = summaries.len();

    let mut human = String::new();
    for (task, level) in &entries {
        if !human.is_empty() {
            human.push('\n');
        }
        human.push_str(&" ".repeat(level * 2));
        human.push_str(&format_tree_line(task, &graph, *level == 0));
    }

    Ok(crate::output::CommandOutput::new(
        json!({ "root": root.id, "issues": summaries, "count": count }),
        human,
    )
    .with_meta(json!({ "count": count })))
}

/// `orbit search <query>`: free text plus `key:value` qualifiers, ranked
/// title → metadata → body, each hit carrying a `snippet`.
pub(super) fn cmd_search(board: &Board, args: SearchArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = crate::index::load(board, LoadMode::Strict)?;
    let query = crate::search::parse_query(&args.query)?;
    let (json_filter, fields) = query_options(args.filter.as_deref(), args.fields.as_deref())?;
    let scope = repository_scope(board, args.all)?;

    // `snippet` is a valid --fields/--filter root on search results.
    if let Some(any_task) = graph.tasks.values().next() {
        let mut schema_keys =
            crate::search::filter_field_names(&graph::task_summary(&graph, any_task));
        schema_keys.push("snippet".to_string());
        if let Some(filter) = &json_filter {
            crate::search::validate_filter_fields(filter, &schema_keys)?;
        }
        if let Some(fields) = &fields {
            validate_field_roots(fields, &schema_keys)?;
        }
    }

    let mut keyed: Vec<(SortKey, &Task)> = graph
        .tasks
        .values()
        .filter(|task| {
            if !args.include_closed && graph.workflow.is_terminal(&task.front.status) {
                return false;
            }
            if let Some(scope) = scope.as_deref()
                && task
                    .front
                    .repository
                    .as_deref()
                    .is_some_and(|repo| repo != scope)
            {
                return false;
            }
            crate::search::matches(task, &query)
        })
        .map(|task| {
            (
                sort_key(&graph, task, crate::search::rank(task, &query)),
                task,
            )
        })
        .collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));

    let mut filtered: Vec<(SortKey, &Task, Value)> = Vec::with_capacity(keyed.len());
    let index = graph.summary_index();
    let search_summary = |task: &Task| {
        let mut summary = graph::task_summary_in(&graph, task, &index);
        summary["snippet"] = json!(crate::search::snippet(task, &query));
        summary
    };
    for (key, task) in keyed {
        // Summaries are costly; build them up front only for `--filter`.
        let summary = match &json_filter {
            Some(filter) => {
                let summary = search_summary(task);
                if !crate::search::matches_filter(&summary, filter) {
                    continue;
                }
                summary
            }
            None => Value::Null,
        };
        filtered.push((key, task, summary));
    }

    let scope = {
        let mut scoped = args.clone();
        scoped.cursor = None;
        scoped.limit = None;
        format!("search|{scoped:?}")
    };
    let (page, next_cursor, total) =
        paginate(filtered, args.cursor.as_deref(), args.limit, &scope)?;
    let count = page.len();

    let mut summaries: Vec<Value> = page
        .iter()
        .map(|(_, task, summary)| {
            if summary.is_null() {
                search_summary(task)
            } else {
                summary.clone()
            }
        })
        .collect();
    if let Some(fields) = &fields {
        let mut array = Value::Array(summaries);
        crate::search::project_fields(&mut array, fields);
        summaries = match array {
            Value::Array(items) => items,
            _ => unreachable!(),
        };
    }

    let human = if summaries.is_empty() {
        "no matching issues".to_string()
    } else {
        summaries
            .iter()
            .map(|summary| {
                let snippet = summary["snippet"].as_str().unwrap_or("");
                format!(
                    "{}  {}  {}",
                    summary["id"].as_str().unwrap_or("?"),
                    summary["title"].as_str().unwrap_or(""),
                    snippet
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(crate::output::CommandOutput::new(
        json!({
            "results": summaries,
            "count": count,
            "total": total,
            "next_cursor": next_cursor,
        }),
        human,
    )
    .with_meta(json!({ "count": count })))
}

/// `orbit digest <id>`: one Issue's compact state, plus a subtree rollup
/// (`done`/`cancelled`/`blocked`/`open` counts) when it has SubTasks.
pub(super) fn cmd_digest(board: &Board, args: DigestArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = crate::index::load(board, LoadMode::Strict)?;
    let task = graph.get(&args.id)?;
    let comments = board.read_comments(&task.id)?;
    let descendants = graph.descendants(&task.id);

    let blocked_by: Vec<String> = graph.blocked_by(task);
    let blocking: Vec<String> = graph
        .blocking_dependents(&task.id)
        .iter()
        .map(|dependent| graph.display_id(&dependent.id))
        .collect();
    let children: Vec<&Task> = graph.children(&task.id);
    let child_entries: Vec<Value> = children
        .iter()
        .map(|child| {
            json!({
                "id": child.id,
                "key": child.front.key,
                "title": child.front.title,
                "status": child.front.status,
                "blocked": !graph.blocking_ids(child).is_empty(),
            })
        })
        .collect();

    let mut data = json!({
        "id": task.id,
        "key": task.front.key,
        "title": task.front.title,
        "type": task.front.task_type,
        "status": task.front.status,
        "status_category": graph
            .workflow
            .category_of(&task.front.status)
            .map(crate::workflow::StatusCategory::as_str),
        "resolution": task.front.resolution,
        "resolved_at": task.front.resolved_at.map(|at| at.to_rfc3339()),
        "priority": task.front.priority,
        "ready": graph.is_ready(task),
        "assignee": task.assignee_name(),
        "assignee_kind": task.front.assignee.as_ref().map(|a| a.kind.as_str()),
        "lease_until": task.lease_until().map(|until| until.to_rfc3339()),
        "reporter": task.front.reporter,
        "labels": task.front.labels,
        "component": task.front.component,
        "repository": task.front.repository,
        "external_id": task.front.external_id,
        "parent": task.front.parent.as_deref().map(|parent| graph.display_id(parent)),
        "depth": graph.depth(&task.id),
        "blocked_by": blocked_by,
        "blocking": blocking,
        "children": child_entries,
        "comment_count": comments.len(),
        "created_at": task.front.created_at.to_rfc3339(),
        "updated_at": task.front.updated_at.to_rfc3339(),
    });

    if !descendants.is_empty() {
        let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
        let mut done = 0usize;
        let mut cancelled = 0usize;
        let mut blocked = 0usize;
        let mut open = 0usize;
        for id in &descendants {
            let Some(descendant) = graph.tasks.get(id) else {
                continue;
            };
            *by_status
                .entry(descendant.front.status.clone())
                .or_default() += 1;
            match graph.workflow.category_of(&descendant.front.status) {
                Some(crate::workflow::StatusCategory::Done) => done += 1,
                Some(crate::workflow::StatusCategory::Canceled) => cancelled += 1,
                _ => open += 1,
            }
            if !graph.blocking_ids(descendant).is_empty() {
                blocked += 1;
            }
        }
        data["rollup"] = json!({
            "scope": "descendants",
            "total": descendants.len(),
            "done": done,
            "cancelled": cancelled,
            "blocked": blocked,
            "open": open,
            "by_status": by_status,
        });
        let direct_total = children.len();
        let direct_open = children
            .iter()
            .filter(|child| !graph.workflow.is_terminal(&child.front.status))
            .count();
        data["sub_issues"] = json!({
            "total": direct_total,
            "open": direct_open,
            "done": direct_total - direct_open,
        });
    }

    if let Some(checkpoint) = &task.front.checkpoint {
        data["checkpoint"] = json!({
            "by": checkpoint.by,
            "at": checkpoint.at.to_rfc3339(),
            "progress": checkpoint.progress,
            "next": checkpoint.next,
            "blocker": checkpoint.blocker,
        });
    }

    let mut human = format!(
        "{}  {}\nstatus: {} ({})  priority: {}  ready: {}\n",
        task.display_id(),
        task.front.title,
        task.front.status,
        data["status_category"].as_str().unwrap_or("?"),
        task.front.priority,
        graph.is_ready(task),
    );
    if let Some(assignee) = task.assignee_name() {
        human.push_str(&format!("assignee: {assignee}"));
        if let Some(kind) = task.front.assignee.as_ref().map(|a| a.kind.as_str()) {
            human.push_str(&format!(" ({kind})"));
        }
        if let Some(until) = task.lease_until() {
            human.push_str(&format!(", lease until {}", until.to_rfc3339()));
        }
        human.push('\n');
    }
    if let Some(parent) = &task.front.parent {
        human.push_str(&format!("parent: {}\n", graph.display_id(parent)));
    }
    if !blocked_by.is_empty() {
        human.push_str(&format!("blocked_by: {}\n", blocked_by.join(", ")));
    }
    if !children.is_empty() {
        human.push_str(&format!("children: {}\n", child_entries.len()));
    }
    if let Some(rollup) = data.get("rollup") {
        human.push_str(&format!(
            "rollup: {} descendants — {} done, {} cancelled, {} blocked, {} open\n",
            rollup["total"], rollup["done"], rollup["cancelled"], rollup["blocked"], rollup["open"]
        ));
    }
    if let Some(checkpoint) = &task.front.checkpoint {
        human.push_str(&format!("checkpoint: {}\n", checkpoint.progress));
    }
    human.push_str(&format!(
        "updated: {}  comments: {}\n",
        task.front.updated_at.to_rfc3339(),
        comments.len()
    ));

    Ok(crate::output::CommandOutput::new(
        data,
        human.trim_end().to_string(),
    ))
}

pub(super) fn cmd_prime(board: &Board, args: PrimeArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = crate::index::load(board, LoadMode::Strict)?;
    let label_filters = normalize_labels(&args.label)?;
    let repository_filter = normalize_repository_filter(args.repository.as_deref())?;
    let matches = |task: &Task| {
        task.has_labels(&label_filters) && matches_repository(task, repository_filter.as_deref())
    };
    let counts = graph.counts_where(matches);
    let agent = optional_agent(args.by).unwrap_or_else(|| "unknown (set ORBIT_AGENT)".to_string());
    let instructions = format!(
        "Orbit is the task tracker for this repository.\n\
Use the CLI; never edit task files directly.\n\
- `orbit ready --claim --by <agent>` takes one ready task.\n\
- `orbit heartbeat <id> --by <agent>` extends the lease while working.\n\
- `orbit checkpoint <id> --by <agent> --progress \"...\"` leaves a handoff that survives release and takeover.\n\
{COMPLETION_LINE}\n\
- `orbit add \"title\" -p high --depends-on <id>` creates work.\n\
- `orbit depend <id> <dep-id>` links work; cycles and unfinished dependencies are rejected.\n\
- A Parent Task whose SubTasks are all resolved appears under close candidates; close it explicitly when the family is done.\n\
- `orbit show <id>` reads the full body."
    );

    let ready_tasks: Vec<Task> = graph
        .ready_tasks()
        .into_iter()
        .filter(|task| matches(task))
        .take(10)
        .collect();
    let close_candidate_tasks: Vec<&Task> = graph
        .close_candidates()
        .into_iter()
        .filter(|task| matches(task))
        .take(10)
        .collect();
    let stale_tasks: Vec<&Task> = graph
        .stale_tasks()
        .into_iter()
        .filter(|task| matches(task))
        .take(10)
        .collect();

    // `--max-tokens` trims the task listings — stale claims first, then
    // close candidates, then ready — until the rendered context fits the
    // budget; the fixed instructions always stay.
    let (mut ready_len, mut close_len, mut stale_len) = (
        ready_tasks.len(),
        close_candidate_tasks.len(),
        stale_tasks.len(),
    );
    let mut truncated = false;
    // The JSON payload (what MCP returns) carries full summaries, so the budget
    // must hold for it too, not only for the human rendering.
    let index = graph.summary_index();
    let all_ready: Vec<Value> = ready_tasks
        .iter()
        .map(|task| graph::task_summary_in(&graph, task, &index))
        .collect();
    let all_close: Vec<Value> = close_candidate_tasks
        .iter()
        .map(|task| graph::task_summary_in(&graph, task, &index))
        .collect();
    let all_stale: Vec<Value> = stale_tasks
        .iter()
        .map(|task| graph::task_summary_in(&graph, task, &index))
        .collect();
    let json_tokens = |ready: usize, close: usize, stale: usize| {
        estimate_tokens(
            &json!({
                "instructions": instructions,
                "counts": counts,
                "ready": &all_ready[..ready],
                "close_candidates": &all_close[..close],
                "stale": &all_stale[..stale],
            })
            .to_string(),
        )
    };
    let mut human = prime_human(
        &graph,
        &agent,
        &instructions,
        &label_filters,
        repository_filter.as_deref(),
        &counts,
        &ready_tasks,
        &close_candidate_tasks,
        &stale_tasks,
    );
    if let Some(max_tokens) = args.max_tokens {
        if max_tokens == 0 {
            return Err(OrbitError::invalid_argument(
                "--max-tokens must be at least 1",
            ));
        }
        while estimate_tokens(&human).max(json_tokens(ready_len, close_len, stale_len)) > max_tokens
            && (ready_len + close_len + stale_len) > 0
        {
            if stale_len > 0 {
                stale_len -= 1;
            } else if close_len > 0 {
                close_len -= 1;
            } else {
                ready_len -= 1;
            }
            human = prime_human(
                &graph,
                &agent,
                &instructions,
                &label_filters,
                repository_filter.as_deref(),
                &counts,
                &ready_tasks[..ready_len],
                &close_candidate_tasks[..close_len],
                &stale_tasks[..stale_len],
            );
        }
        if estimate_tokens(&human).max(json_tokens(ready_len, close_len, stale_len)) > max_tokens {
            // The fixed instructions alone exceed the budget: hard-truncate
            // the human rendering and report it honestly.
            human = human.chars().take(max_tokens.saturating_mul(4)).collect();
            truncated = true;
        }
    }

    let ready = all_ready[..ready_len].to_vec();
    let close_candidates = all_close[..close_len].to_vec();
    let stale = all_stale[..stale_len].to_vec();

    let mut data = json!({
        "agent": agent,
        "instructions": instructions,
        "labels": label_filters,
        "repository": repository_filter,
        "counts": counts,
        "ready": ready,
        "close_candidates": close_candidates,
        "stale": stale,
    });
    if let Some(max_tokens) = args.max_tokens {
        data["budget"] = json!({
            "max_tokens": max_tokens,
            "estimated_tokens": estimate_tokens(&human)
                .max(json_tokens(ready_len, close_len, stale_len)),
            "truncated": truncated,
        });
    }

    let mut output = crate::output::CommandOutput::new(data, human);

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

/// Rough token estimate for budget checks: ~4 characters per token.
fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// The human rendering of `orbit prime`: instructions, agent, scope, counts,
/// then the ready / close-candidate / stale-claim listings. Extracted so
/// `--max-tokens` can re-render with shorter listings.
#[allow(clippy::too_many_arguments)]
fn prime_human(
    graph: &Graph,
    agent: &str,
    instructions: &str,
    label_filters: &[String],
    repository_filter: Option<&str>,
    counts: &crate::graph::Counts,
    ready_tasks: &[Task],
    close_tasks: &[&Task],
    stale_tasks: &[&Task],
) -> String {
    let mut human = String::new();
    human.push_str(instructions);
    human.push_str("\n\n");
    human.push_str(&format!("agent: {agent}\n"));
    if !label_filters.is_empty() {
        human.push_str(&format!("scope: {}\n", label_filters.join(", ")));
    }
    if let Some(repository) = repository_filter {
        human.push_str(&format!("repository: {repository}\n"));
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
    if ready_tasks.is_empty() {
        human.push_str("  (none)\n");
    } else {
        for task in ready_tasks {
            human.push_str(&format!("  {}\n", format_summary(task, graph)));
        }
    }
    human.push_str("close candidates:\n");
    if close_tasks.is_empty() {
        human.push_str("  (none)\n");
    } else {
        for &task in close_tasks {
            human.push_str(&format!(
                "  {}  (ready to close)\n",
                format_summary(task, graph)
            ));
        }
    }
    human.push_str("stale claims:\n");
    if stale_tasks.is_empty() {
        human.push_str("  (none)\n");
    } else {
        for &task in stale_tasks {
            human.push_str(&format!("  {}\n", format_summary(task, graph)));
            let summary = graph::task_summary(graph, task);
            push_summary_checkpoint(&mut human, &summary);
        }
    }
    human
}

pub(super) fn cmd_export(board: &Board) -> Result<crate::output::CommandOutput> {
    let (graph, _) = crate::index::load(board, LoadMode::Strict)?;
    let mut sorted: Vec<&crate::task::Task> = graph.tasks.values().collect();
    sorted.sort_by(|left, right| left.id.cmp(&right.id));

    let mut tasks = Vec::with_capacity(sorted.len());
    let mut comment_count = 0;
    for (task, detail) in sorted.iter().zip(graph::task_details(&graph, &sorted)) {
        let comments = board.read_comments(&task.id)?;
        comment_count += comments.len();
        tasks.push(graph::with_comments(detail, &comments));
    }

    let task_count = tasks.len();
    let snapshot = json!({
        "schema_version": 5,
        "board": {
            "branch": board.config.board.branch,
            "prefix": board.config.id.prefix,
            "issue_count": task_count,
            "comment_count": comment_count,
        },
        "issues": tasks,
    });
    Ok(crate::output::CommandOutput::new(
        snapshot,
        format!("exported {task_count} tasks and {comment_count} comments (schema 5)"),
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

/// The Repository scope for `list`/`ready`: the member Repository the command
/// runs in, or `None` at the Board's root or with `--all`. A Task without a
/// `repository` is Board-wide and always stays in scope.
fn repository_scope(board: &Board, all: bool) -> Result<Option<String>> {
    if all {
        return Ok(None);
    }
    crate::repository::current_scope(board)
}

fn normalize_type_filters(types: &[String]) -> Vec<String> {
    types
        .iter()
        .map(|ty| crate::hierarchy::normalize_type_name(ty))
        .collect()
}

#[derive(Clone, Copy)]
struct ReadyFilters<'a> {
    parent: Option<&'a str>,
    labels: &'a [String],
    excluded: &'a [String],
    types: &'a [String],
    excluded_types: &'a [String],
    components: &'a [String],
    excluded_components: &'a [String],
    scope: Option<&'a str>,
    repository: Option<&'a str>,
}

fn ready_for_filters(mut ready: Vec<Task>, filters: ReadyFilters<'_>) -> Vec<Task> {
    let ReadyFilters {
        parent,
        labels,
        excluded,
        types,
        excluded_types,
        components,
        excluded_components,
        scope,
        repository,
    } = filters;
    ready.retain(|task| matches_repository(task, repository));
    if let Some(scope) = scope {
        ready.retain(|task| {
            task.front
                .repository
                .as_deref()
                .is_none_or(|repo| repo == scope)
        });
    }
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
    if !components.is_empty() {
        ready.retain(|task| {
            task.front
                .component
                .as_deref()
                .is_some_and(|component| components.iter().any(|candidate| candidate == component))
        });
    }
    if !excluded_components.is_empty() {
        ready.retain(|task| {
            !task.front.component.as_deref().is_some_and(|component| {
                excluded_components
                    .iter()
                    .any(|candidate| candidate == component)
            })
        });
    }
    ready
}
