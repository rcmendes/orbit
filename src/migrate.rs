//! Explicit Task-ID prefix migration.
//!
//! A Board normally has one immutable literal prefix. `orbit slug migrate`
//! rewrites every ID to a new prefix instead of teaching the rest of the CLI to
//! accept a permanent set of legacy namespaces. The migration preserves the
//! random suffix, updates structured references, and runs as one commit built
//! in a disposable Git worktree.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tempfile::Builder;

use crate::board::{Board, Config};
use crate::error::{OrbitError, Result, code};
use crate::git::{self, CommitOutcome};
use crate::graph::{Graph, Issue, LoadMode};
use crate::slug;
use crate::task;

#[derive(Debug, Clone, Serialize)]
pub struct Mapping {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaimMapping {
    pub from: String,
    pub to: String,
    pub holder: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Collision {
    pub id: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub old_slug: String,
    pub old_prefix: String,
    pub new_slug: String,
    pub new_prefix: String,
    pub changed: bool,
    pub task_count: usize,
    pub subtask_count: usize,
    pub comment_files: Vec<String>,
    pub references: usize,
    pub mappings: Vec<Mapping>,
    pub claims: Vec<ClaimMapping>,
    pub collisions: Vec<Collision>,
}

#[derive(Debug, Clone)]
pub struct Applied {
    pub commit: String,
    pub warnings: Vec<String>,
}

/// Builds a deterministic migration plan from a strictly loaded Board.
pub fn plan(board: &Board, graph: &Graph, requested_slug: &str) -> Result<Plan> {
    let requested_prefix = slug::explicit_prefix(requested_slug)?;
    let old_prefix = board.config.id.prefix.clone();
    let old_slug = slug::slug_of_prefix(&old_prefix).to_string();
    let changed = old_slug != requested_slug;
    let new_prefix = if changed {
        requested_prefix
    } else {
        old_prefix.clone()
    };

    let mut old_to_new = BTreeMap::new();
    if changed {
        for task in graph.tasks.values() {
            let to = map_id(&task.id, &old_prefix, &new_prefix)?;
            old_to_new.insert(task.id.clone(), to);
        }
    }

    let mut mappings: Vec<Mapping> = old_to_new
        .iter()
        .map(|(from, to)| Mapping {
            from: from.clone(),
            to: to.clone(),
        })
        .collect();
    mappings.sort_by(|left, right| left.from.cmp(&right.from));

    let collisions = detect_collisions(&old_to_new);
    let comment_files = board.comment_ids()?;
    let references = if changed {
        graph
            .tasks
            .values()
            .map(|task| task.front.depends_on.len() + usize::from(task.front.parent.is_some()))
            .sum()
    } else {
        0
    };

    let now = Utc::now();
    let mut claims = Vec::new();
    if changed {
        for task in graph.tasks.values() {
            let Some(claim) = task.front.claim.as_ref() else {
                continue;
            };
            if !claim.is_live(now) {
                continue;
            }
            let Some(to) = old_to_new.get(&task.id) else {
                continue;
            };
            claims.push(ClaimMapping {
                from: task.id.clone(),
                to: to.clone(),
                holder: claim.holder.clone(),
                lease_until: claim.lease_until,
            });
        }
        claims.sort_by(|left, right| left.from.cmp(&right.from));
    }

    Ok(Plan {
        old_slug,
        old_prefix,
        new_slug: requested_slug.to_string(),
        new_prefix,
        changed,
        task_count: graph.tasks.len(),
        subtask_count: graph
            .tasks
            .values()
            .filter(|task| task.front.parent.is_some())
            .count(),
        comment_files,
        references,
        mappings,
        claims,
        collisions,
    })
}

fn map_id(id: &str, old_prefix: &str, new_prefix: &str) -> Result<String> {
    let rest = id.strip_prefix(old_prefix).ok_or_else(|| {
        OrbitError::new(
            code::MIGRATION_BLOCKED,
            format!("task id {id} does not start with the configured prefix `{old_prefix}`"),
        )
    })?;
    Ok(format!("{new_prefix}{rest}"))
}

/// Detects two source IDs that would collapse onto one target ID. The normal
/// one-prefix board cannot trigger this, but the check keeps the rewrite safe if
/// the accepted ID set ever grows.
fn detect_collisions(old_to_new: &BTreeMap<String, String>) -> Vec<Collision> {
    let mut targets: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, to) in old_to_new {
        targets.entry(to).or_default().push(from);
    }
    targets
        .into_iter()
        .filter(|(_, sources)| sources.len() > 1)
        .map(|(to, mut sources)| {
            sources.sort();
            Collision {
                id: to.to_string(),
                sources: sources.into_iter().map(str::to_string).collect(),
            }
        })
        .collect()
}

/// Doctor-equivalent issues that make a migration unsafe. Strict loading already
/// covers task parsing and graph structure; this adds the checks doctor performs
/// over comments, parent/ID agreement, and the checked-out branch.
pub fn health_issues(board: &Board, graph: &Graph) -> Result<Vec<Issue>> {
    health_issues_inner(board, graph, true)
}

fn health_issues_inner(board: &Board, graph: &Graph, check_branch: bool) -> Result<Vec<Issue>> {
    let mut issues = Vec::new();

    for id in board.comment_ids()? {
        if !graph.tasks.contains_key(&id) {
            issues.push(Issue::new(
                code::ORPHAN_COMMENT,
                Some(&id),
                format!("comments/{id}.md has no matching task"),
            ));
            continue;
        }
        if let Err(error) = board.read_comments(&id) {
            issues.push(Issue::new(error.code, Some(&id), error.message));
            continue;
        }
        match comment_header_matches(&board.comment_path(&id), &id) {
            Ok(true) => {}
            Ok(false) => issues.push(Issue::new(
                code::CORRUPT_COMMENT,
                Some(&id),
                format!(
                    "comments/{id}.md does not start with `# Comments: {id}`; repair the header before migrating"
                ),
            )),
            Err(error) => issues.push(Issue::new(error.code, Some(&id), error.message)),
        }
    }

    for task in graph.tasks.values() {
        let encoded_parent = task::parent_id_from_id(&task.id, &board.config.id.prefix);
        match (&task.front.parent, encoded_parent) {
            (Some(parent), Some(encoded)) if parent != &encoded => {
                issues.push(Issue::new(
                    code::PARENT_MISMATCH,
                    Some(&task.id),
                    format!(
                        "{} has parent {parent} but its ID encodes {encoded}",
                        task.id
                    ),
                ));
            }
            (None, Some(encoded)) => {
                issues.push(Issue::new(
                    code::PARENT_MISMATCH,
                    Some(&task.id),
                    format!(
                        "{} is a SubTask ID but has no parent; expected {encoded}",
                        task.id
                    ),
                ));
            }
            (Some(parent), None) => {
                issues.push(Issue::new(
                    code::PARENT_MISMATCH,
                    Some(&task.id),
                    format!(
                        "{} has parent {parent} but its ID is not a SubTask ID",
                        task.id
                    ),
                ));
            }
            _ => {}
        }
    }

    if check_branch {
        match git::current_branch(&board.root) {
            Ok(branch) if branch != board.config.board.branch => issues.push(Issue::new(
                code::BOARD_CORRUPT,
                None,
                format!(
                    "board is on branch {branch}, expected {}",
                    board.config.board.branch
                ),
            )),
            Ok(_) => {}
            Err(error) => issues.push(Issue::new(code::BOARD_CORRUPT, None, error.message)),
        }
    }

    Ok(issues)
}

fn comment_header_matches(path: &Path, id: &str) -> Result<bool> {
    let content = std::fs::read_to_string(path)
        .map_err(|error| OrbitError::io(format!("read {}", path.display()), error))?;
    let first = content.lines().next().unwrap_or_default();
    Ok(first == format!("# Comments: {id}"))
}

pub fn blocked_by_issues(issues: Vec<Issue>) -> OrbitError {
    OrbitError::new(
        code::MIGRATION_BLOCKED,
        "the Board is not healthy enough to migrate; run `orbit doctor` and fix the reported issues",
    )
    .with_details(serde_json::json!({
        "reason": "unhealthy",
        "issues": issues,
    }))
}

pub fn blocked_by_claims(claims: &[ClaimMapping]) -> OrbitError {
    OrbitError::new(
        code::MIGRATION_BLOCKED,
        format!(
            "{} task(s) have live claims; re-run with --force after telling the holders to re-read the Board",
            claims.len()
        ),
    )
    .with_details(serde_json::json!({
        "reason": "live_claims",
        "claims": claims,
    }))
}

pub fn blocked_by_collisions(collisions: &[Collision], target_prefix: &str) -> OrbitError {
    OrbitError::new(
        code::PREFIX_COLLISION,
        format!(
            "{} ID(s) would collide under prefix `{target_prefix}`; no files were changed",
            collisions.len()
        ),
    )
    .with_details(serde_json::json!({
        "target": target_prefix,
        "collisions": collisions,
    }))
}

pub fn blocked_by_dirty_board() -> OrbitError {
    OrbitError::new(
        code::MIGRATION_BLOCKED,
        "the Board still has uncommitted changes after syncing; commit or discard them before migrating",
    )
    .with_details(serde_json::json!({ "reason": "dirty_board" }))
}

/// Builds, validates, and commits the migration in a temporary worktree, then
/// fast-forwards the real Board to that commit. The Board must be clean and the
/// caller must hold the exclusive lock.
pub fn apply(board: &Board, graph: &Graph, plan: &Plan) -> Result<Applied> {
    if !plan.changed {
        return Err(OrbitError::invalid_argument(
            "the Board already uses this slug; there is nothing to migrate",
        ));
    }
    if !plan.collisions.is_empty() {
        return Err(blocked_by_collisions(&plan.collisions, &plan.new_prefix));
    }

    // Heartbeat writes are deliberately not committed. Flush them first so the
    // migration commit contains only the rewrite and is the single rollback point.
    match board.commit("orbit: sync", true)? {
        CommitOutcome::Committed | CommitOutcome::NoChanges | CommitOutcome::Skipped => {}
        CommitOutcome::Failed(message) => return Err(OrbitError::git(message)),
    }
    if !git::status_porcelain(&board.root)?.trim().is_empty() {
        return Err(blocked_by_dirty_board());
    }
    let base = git::head_commit(&board.root)?;

    let temp = Builder::new()
        .prefix("orbit-slug-migrate-")
        .tempdir()
        .map_err(|error| OrbitError::io("create temporary migration directory", error))?;
    let temp_path = temp.path().join("board");
    git::worktree_add_detached(&board.root, &temp_path)?;

    match build_and_commit(board, graph, plan, &temp_path) {
        Ok(commit) => {
            let current = match git::head_commit(&board.root) {
                Ok(current) => current,
                Err(error) => {
                    let _ = git::worktree_remove(&board.root, &temp_path);
                    return Err(error);
                }
            };
            if current != base {
                let _ = git::worktree_remove(&board.root, &temp_path);
                return Err(OrbitError::new(
                    code::MIGRATION_BLOCKED,
                    "the Board branch moved while the migration was being built; no files were changed, retry the migration",
                )
                .with_details(serde_json::json!({ "reason": "board_moved" })));
            }
            if let Err(error) = git::reset_hard(&board.root, &commit) {
                let path = temp_path.clone();
                std::mem::forget(temp);
                return Err(OrbitError::new(
                    code::GIT_ERROR,
                    format!(
                        "{}; the migrated commit {commit} is in the temporary worktree {}; inspect it before retrying",
                        error.message,
                        path.display()
                    ),
                ));
            }

            let mut warnings = Vec::new();
            if let Err(error) = git::worktree_remove(&board.root, &temp_path) {
                warnings.push(format!(
                    "could not remove the temporary worktree {}: {}; run `git worktree prune`",
                    temp_path.display(),
                    error.message
                ));
            }
            Ok(Applied { commit, warnings })
        }
        Err(error) => {
            let _ = git::worktree_remove(&board.root, &temp_path);
            Err(error)
        }
    }
}

fn build_and_commit(board: &Board, graph: &Graph, plan: &Plan, root: &Path) -> Result<String> {
    let mut config: Config = board.config.clone();
    config.id.prefix = plan.new_prefix.clone();
    config.validate()?;
    std::fs::write(root.join("config.toml"), config.to_toml()?.as_bytes())
        .map_err(|error| OrbitError::io("write migrated config.toml", error))?;

    let old_to_new: BTreeMap<&str, &str> = plan
        .mappings
        .iter()
        .map(|mapping| (mapping.from.as_str(), mapping.to.as_str()))
        .collect();

    for mapping in &plan.mappings {
        let original = graph.tasks.get(&mapping.from).ok_or_else(|| {
            OrbitError::new(
                code::MIGRATION_BLOCKED,
                format!("task {} disappeared while planning", mapping.from),
            )
        })?;
        let mut migrated = original.clone();
        migrated.id = mapping.to.clone();
        migrated.front.parent = match original.front.parent.as_deref() {
            Some(parent) => Some(mapped(&old_to_new, parent)?.to_string()),
            None => None,
        };
        migrated.front.depends_on = original
            .front
            .depends_on
            .iter()
            .map(|dependency| mapped(&old_to_new, dependency).map(str::to_string))
            .collect::<Result<Vec<_>>>()?;

        let rendered = task::render_task(&migrated)?;
        std::fs::write(
            root.join("tasks").join(format!("{}.md", migrated.id)),
            rendered.as_bytes(),
        )
        .map_err(|error| OrbitError::io(format!("write migrated task {}", migrated.id), error))?;
        if mapping.from != mapping.to {
            std::fs::remove_file(root.join("tasks").join(format!("{}.md", mapping.from))).map_err(
                |error| OrbitError::io(format!("remove old task {}", mapping.from), error),
            )?;
        }
    }

    for old_id in &plan.comment_files {
        let new_id = mapped(&old_to_new, old_id)?;
        let content = std::fs::read_to_string(board.comment_path(old_id))
            .map_err(|error| OrbitError::io(format!("read comments/{old_id}.md"), error))?;
        let updated = rewrite_comment_header(&content, old_id, new_id)?;
        std::fs::create_dir_all(root.join("comments"))
            .map_err(|error| OrbitError::io("create comments directory", error))?;
        std::fs::write(
            root.join("comments").join(format!("{new_id}.md")),
            updated.as_bytes(),
        )
        .map_err(|error| OrbitError::io(format!("write migrated comments/{new_id}.md"), error))?;
        if old_id != new_id {
            std::fs::remove_file(root.join("comments").join(format!("{old_id}.md"))).map_err(
                |error| OrbitError::io(format!("remove old comments/{old_id}.md"), error),
            )?;
        }
    }

    // Validate the complete result before the real Board moves. A bug here
    // leaves the original Board untouched and only discards the temp worktree.
    let temp_board = Board {
        root: root.to_path_buf(),
        lock_path: root.join(".lock"),
        config,
    };
    let (migrated_graph, _) = Graph::load(&temp_board, LoadMode::Strict)?;
    for mapping in &plan.mappings {
        migrated_graph.get(&mapping.to)?;
        if migrated_graph.tasks.contains_key(&mapping.from) {
            return Err(OrbitError::board_corrupt(
                "migration left an old task ID in the temporary Board",
                serde_json::json!({ "task": mapping.from }),
            ));
        }
    }
    let issues = health_issues_inner(&temp_board, &migrated_graph, false)?;
    if !issues.is_empty() {
        return Err(OrbitError::board_corrupt(
            "the migrated Board failed validation; the original Board was not changed",
            serde_json::json!({ "issues": issues }),
        ));
    }

    let message = format!(
        "orbit: migrate task prefix from {} to {}",
        plan.old_prefix, plan.new_prefix
    );
    match git::commit(root, &message)? {
        CommitOutcome::Committed => {}
        other => {
            return Err(OrbitError::git(format!(
                "migration produced no commit ({other:?})"
            )));
        }
    }
    git::head_commit(root)
}

fn mapped<'a>(old_to_new: &'a BTreeMap<&str, &str>, id: &str) -> Result<&'a str> {
    old_to_new.get(id).copied().ok_or_else(|| {
        OrbitError::new(
            code::MIGRATION_BLOCKED,
            format!("task {id} has a reference that is not in the migration plan"),
        )
    })
}

fn rewrite_comment_header(content: &str, old_id: &str, new_id: &str) -> Result<String> {
    let expected = format!("# Comments: {old_id}");
    let Some(newline) = content.find('\n') else {
        return Err(OrbitError::new(
            code::CORRUPT_COMMENT,
            format!("comments/{old_id}.md is missing the `{expected}` header"),
        ));
    };
    let first = &content[..newline];
    let first_without_cr = first.strip_suffix('\r').unwrap_or(first);
    if first_without_cr != expected {
        return Err(OrbitError::new(
            code::CORRUPT_COMMENT,
            format!("comments/{old_id}.md does not start with `{expected}`"),
        ));
    }
    let carriage_return = if first.ends_with('\r') { "\r" } else { "" };
    Ok(format!(
        "# Comments: {new_id}{carriage_return}{}",
        &content[newline..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_top_level_and_subtask_suffixes() {
        assert_eq!(
            map_id("orb-a7b2c9", "orb-", "atlas-").unwrap(),
            "atlas-a7b2c9"
        );
        assert_eq!(
            map_id("orb-a7b2c9.d4b3df", "orb-", "atlas-").unwrap(),
            "atlas-a7b2c9.d4b3df"
        );
        assert_eq!(
            map_id("orbabc123", "orb", "atlas-").unwrap(),
            "atlas-abc123"
        );
    }

    #[test]
    fn collision_detection_groups_sources_by_target() {
        let mappings = BTreeMap::from([
            ("a-one".to_string(), "b-one".to_string()),
            ("c-one".to_string(), "b-one".to_string()),
            ("d-two".to_string(), "b-two".to_string()),
        ]);
        let collisions = detect_collisions(&mappings);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].id, "b-one");
        assert_eq!(collisions[0].sources, vec!["a-one", "c-one"]);
    }

    #[test]
    fn comment_header_rewrite_preserves_body_and_line_endings() {
        let content = "# Comments: orb-a7b2c9\n\n<!-- orbit:comment x -->\nbody\n";
        let updated = rewrite_comment_header(content, "orb-a7b2c9", "atlas-a7b2c9").unwrap();
        assert_eq!(
            updated,
            "# Comments: atlas-a7b2c9\n\n<!-- orbit:comment x -->\nbody\n"
        );

        let crlf = "# Comments: orb-a7b2c9\r\n\r\nbody\r\n";
        let updated = rewrite_comment_header(crlf, "orb-a7b2c9", "atlas-a7b2c9").unwrap();
        assert_eq!(updated, "# Comments: atlas-a7b2c9\r\n\r\nbody\r\n");
    }

    #[test]
    fn comment_header_rewrite_refuses_a_mismatch() {
        let error =
            rewrite_comment_header("# Notes: orb-a7b2c9\nbody\n", "orb-a7b2c9", "atlas-a7b2c9")
                .unwrap_err();
        assert_eq!(error.code, code::CORRUPT_COMMENT);
    }
}
