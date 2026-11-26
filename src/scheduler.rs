//! Opt-in ready-queue scheduler strategies (ADR 0022).
//!
//! The kernel's `Graph::ready_tasks` order — Board priority, then creation
//! time, then Task ID — is the `default` strategy and is unchanged. A Board
//! opts in through `[scheduler]` in `config.toml`; `orbit ready --strategy`
//! overrides the Board's choice for one invocation. The value is a
//! comma-separated list of strategy names applied in order.
//!
//! Strategies:
//!
//! - `default` — keep the kernel order (a no-op).
//! - `critical-path` — stable-sort by the length of the longest chain of
//!   non-terminal Tasks that transitively block-depend on each candidate, so
//!   work that unblocks the most downstream effort is claimed first.
//! - `touches` — avoid file-touch overlap. An Issue declares the paths it
//!   expects to modify with the `touches` Property
//!   (`orbit property set <id> touches --value '{"paths": ["src/auth"]}'`).
//!   On `ready --claim` a candidate is refused when its paths overlap —
//!   equal, or one a `/`-separated prefix of the other — the paths of a
//!   non-terminal Task assigned to a *different* assignee, so two agents
//!   never receive overlapping touches. On a plain `ready` listing the same
//!   candidates sort first instead of being hidden.
//! - `capability` — stable-sort Tasks whose declared capability
//!   requirements (`capabilities` Property, `{"requires": [...]}`) are a
//!   subset of the claiming agent's configured capabilities
//!   (`[scheduler.capabilities]`) first. Tasks requiring nothing match
//!   everyone. This is a preference, not a refusal.
//! - `wip` — on `ready --claim`, refuse the claim when the agent already
//!   holds `scheduler.wip_limit` non-terminal assigned Tasks.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::Value;

use crate::error::{OrbitError, Result};
use crate::graph::Graph;
use crate::task::{self, Task};

/// Every strategy name `scheduler.strategy` / `--strategy` accepts.
pub const STRATEGIES: &[&str] = &["default", "critical-path", "touches", "capability", "wip"];

/// One named scheduler strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Keep the kernel `(priority, created_at, id)` order.
    Default,
    /// Unblock the longest downstream dependency chain first.
    CriticalPath,
    /// Never hand two agents overlapping `touches` paths.
    Touches,
    /// Prefer Tasks whose required capabilities the agent has.
    Capability,
    /// Refuse a claim past `scheduler.wip_limit` held Tasks.
    Wip,
}

impl Strategy {
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::CriticalPath => "critical-path",
            Self::Touches => "touches",
            Self::Capability => "capability",
            Self::Wip => "wip",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "default" => Some(Self::Default),
            "critical-path" => Some(Self::CriticalPath),
            "touches" => Some(Self::Touches),
            "capability" => Some(Self::Capability),
            "wip" => Some(Self::Wip),
            _ => None,
        }
        .filter(|strategy| STRATEGIES.contains(&strategy.name()))
    }
}

// The `[scheduler]` config schema lives in `crate::board` (kernel-owned
// config.toml schema); this module interprets and validates it.
pub use crate::board::SchedulerConfig;

impl SchedulerConfig {
    /// Semantic checks run by `crate::modules::validate` in the application
    /// layer: known strategy names, a positive WIP limit, a limit whenever
    /// `wip` is configured, and label-shaped agent/capability names. The
    /// kernel never calls this.
    pub fn validate(&self) -> Result<()> {
        if let Some(raw) = &self.strategy {
            for name in raw.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                if Strategy::parse(name).is_none() {
                    return Err(OrbitError::config(format!(
                        "scheduler.strategy contains unknown strategy `{name}`; expected one of: {}",
                        STRATEGIES.join(", ")
                    )));
                }
            }
        }
        if self.wip_limit == Some(0) {
            return Err(OrbitError::config("scheduler.wip_limit must be positive"));
        }
        let uses_wip = self
            .strategy
            .as_deref()
            .is_some_and(|raw| raw.split(',').any(|name| name.trim() == "wip"));
        if uses_wip && self.wip_limit.is_none() {
            return Err(OrbitError::config(
                "scheduler strategy `wip` requires scheduler.wip_limit",
            ));
        }
        for (agent, capabilities) in &self.capabilities {
            if !task::is_valid_label(agent) {
                return Err(OrbitError::config(format!(
                    "scheduler.capabilities contains invalid agent name `{agent}`"
                )));
            }
            for capability in capabilities {
                if !task::is_valid_label(capability) {
                    return Err(OrbitError::config(format!(
                        "scheduler.capabilities.{agent} contains invalid capability `{capability}`"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Parses a comma-separated strategy list (`--strategy` or
/// `scheduler.strategy`). An empty or absent value is `default`.
pub fn parse_strategies(raw: &str) -> Result<Vec<Strategy>> {
    let mut strategies = Vec::new();
    for name in raw.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let Some(strategy) = Strategy::parse(name) else {
            return Err(OrbitError::invalid_argument(format!(
                "unknown scheduler strategy `{name}`; expected one of: {}",
                STRATEGIES.join(", ")
            )));
        };
        if !strategies.contains(&strategy) {
            strategies.push(strategy);
        }
    }
    if strategies.is_empty() {
        strategies.push(Strategy::Default);
    }
    Ok(strategies)
}

/// Resolves the active strategies: the `--strategy` flag wins over
/// `scheduler.strategy`, which wins over the kernel default.
pub fn resolve(flag: Option<&str>, config: &SchedulerConfig) -> Result<Vec<Strategy>> {
    match flag.or(config.strategy.as_deref()) {
        Some(raw) => parse_strategies(raw),
        None => Ok(vec![Strategy::Default]),
    }
}

/// Why a strategy emptied the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The agent already holds `scheduler.wip_limit` Tasks.
    WipLimit,
}

/// The queue after strategies ran, plus why it may be empty.
pub struct Outcome {
    pub tasks: Vec<Task>,
    pub refusal: Option<Refusal>,
}

/// Applies `strategies` to an already filtered, kernel-ordered ready queue.
/// `agent` is the claiming agent's name (`None` for a plain listing);
/// `claim` selects the `--claim` semantics of the constraint strategies.
pub fn apply(
    graph: &Graph,
    tasks: Vec<Task>,
    strategies: &[Strategy],
    agent: Option<&str>,
    claim: bool,
    config: &SchedulerConfig,
) -> Result<Outcome> {
    if strategies.contains(&Strategy::Wip) && config.wip_limit.is_none() {
        return Err(OrbitError::invalid_argument(
            "scheduler strategy `wip` requires scheduler.wip_limit",
        ));
    }

    let mut tasks = tasks;
    for strategy in strategies {
        match strategy {
            Strategy::Default => {}
            Strategy::CriticalPath => {
                let scores = critical_scores(graph);
                // Stable sort: equal scores keep the kernel's
                // (priority, created_at, id) order.
                tasks.sort_by(|left, right| scores.get(&right.id).cmp(&scores.get(&left.id)));
            }
            Strategy::Capability => {
                let agent_capabilities: BTreeSet<String> = agent
                    .and_then(|name| config.capabilities.get(name))
                    .map(|caps| caps.iter().cloned().collect())
                    .unwrap_or_default();
                tasks.sort_by_cached_key(|task| {
                    usize::from(!required_capabilities(task).is_subset(&agent_capabilities))
                });
            }
            Strategy::Touches => {
                let held = held_touches(graph);
                if claim {
                    let mut others = BTreeSet::new();
                    for (holder, paths) in &held {
                        if agent.is_none_or(|agent| holder != agent) {
                            others.extend(paths.iter().cloned());
                        }
                    }
                    tasks.retain(|task| touch_paths(task).touches_disjoint(&others));
                } else {
                    let all: BTreeSet<String> = held.values().flatten().cloned().collect();
                    tasks.sort_by_cached_key(|task| {
                        usize::from(!touch_paths(task).touches_disjoint(&all))
                    });
                }
            }
            Strategy::Wip => {
                if !claim {
                    continue;
                }
                let Some(agent) = agent else {
                    continue;
                };
                let held = graph
                    .tasks
                    .values()
                    .filter(|task| {
                        !graph.workflow.is_terminal(&task.front.status)
                            && task.assignee_name() == Some(agent)
                    })
                    .count();
                if held >= config.wip_limit.unwrap_or(0) as usize {
                    return Ok(Outcome {
                        tasks: Vec::new(),
                        refusal: Some(Refusal::WipLimit),
                    });
                }
            }
        }
    }
    Ok(Outcome {
        tasks,
        refusal: None,
    })
}

/// The `touches` Property's declared paths: `{"paths": [...]}` on the
/// `touches` owner. A bare string counts as one path.
fn touch_paths(task: &Task) -> BTreeSet<String> {
    string_entries(task.front.properties.get("touches"), "paths")
}

/// The `capabilities` Property's required capabilities:
/// `{"requires": [...]}` on the `capabilities` owner.
fn required_capabilities(task: &Task) -> BTreeSet<String> {
    string_entries(task.front.properties.get("capabilities"), "requires")
}

fn string_entries(property: Option<&Value>, key: &str) -> BTreeSet<String> {
    let Some(value) = property.and_then(|property| property.get(key)) else {
        return BTreeSet::new();
    };
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(normalize_path)
            .collect(),
        Value::String(single) => {
            let path = normalize_path(single);
            (!single.trim().is_empty())
                .then_some(path)
                .into_iter()
                .collect()
        }
        _ => BTreeSet::new(),
    }
}

/// Lexically normalizes a declared path: drops `.`/empty segments and resolves
/// `..`, so `./src/` and `src` compare equal. The repository root (`.`, `/`,
/// or a path that climbs out) normalizes to `.`, which overlaps everything.
fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.trim().split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return ".".to_string();
                }
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// Whether two declared paths can touch the same file: equal, or one a
/// `/`-separated prefix of the other (`src` overlaps `src/a.rs`, not
/// `src2`).
fn paths_overlap(left: &str, right: &str) -> bool {
    left == "."
        || right == "."
        || left == right
        || left
            .strip_prefix(right)
            .is_some_and(|rest| rest.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Declared `touches` paths per assignee for every non-terminal assigned
/// Task; callers union the entries they care about.
fn held_touches(graph: &Graph) -> BTreeMap<String, BTreeSet<String>> {
    let mut held: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for task in graph.tasks.values() {
        if graph.workflow.is_terminal(&task.front.status) {
            continue;
        }
        // Work parked in review holds no assignee but is still in flight: its
        // author keeps the paths until it closes.
        let in_flight_author = (graph.workflow.category_of(&task.front.status)
            == Some(crate::workflow::StatusCategory::Active))
        .then(|| crate::review::author_name(task))
        .flatten();
        let Some(holder) = task
            .assignee_name()
            .map(str::to_string)
            .or(in_flight_author)
        else {
            continue;
        };
        let holder = holder.as_str();
        let paths = touch_paths(task);
        if paths.is_empty() {
            continue;
        }
        held.entry(holder.to_string()).or_default().extend(paths);
    }
    held
}

/// Overlap-aware disjointness: `touch_paths` returns normalized paths, so a
/// candidate overlaps held work when any pair `paths_overlap`s. Named apart
/// from `BTreeSet::is_disjoint`, which only compares exact members.
trait TouchesDisjoint {
    fn touches_disjoint(&self, other: &BTreeSet<String>) -> bool;
}

impl TouchesDisjoint for BTreeSet<String> {
    fn touches_disjoint(&self, other: &BTreeSet<String>) -> bool {
        !self
            .iter()
            .any(|path| other.iter().any(|held| paths_overlap(path, held)))
    }
}

/// Critical-path score per Task ID: the length of the longest chain of
/// non-terminal Tasks that transitively block-depend on it. Terminal
/// dependents contribute nothing; a dependency cycle stops the walk.
fn critical_scores(graph: &Graph) -> HashMap<String, usize> {
    // Prebuilt reverse index: link target → the non-terminal Tasks whose
    // blocking links point at it, sorted by Task ID like `dependents`.
    // One pass over every Task's links replaces the per-Task
    // `blocking_dependents` scan, which made scoring O(N²).
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    for task in graph.tasks.values() {
        if graph.workflow.is_terminal(&task.front.status) {
            continue;
        }
        let mut targets: Vec<&str> = task
            .front
            .links
            .iter()
            .filter(|link| link.is_blocking())
            .filter_map(|link| link.target_task())
            .collect();
        targets.sort_unstable();
        targets.dedup();
        for target in targets {
            dependents
                .entry(target.to_string())
                .or_default()
                .push(task.id.clone());
        }
    }
    for ids in dependents.values_mut() {
        ids.sort_unstable();
    }

    fn score(
        id: &str,
        dependents: &HashMap<String, Vec<String>>,
        memo: &mut HashMap<String, usize>,
        visiting: &mut HashSet<String>,
    ) -> usize {
        if let Some(score) = memo.get(id) {
            return *score;
        }
        if !visiting.insert(id.to_string()) {
            return 0;
        }
        let mut best = 0;
        if let Some(next) = dependents.get(id) {
            for dependent in next {
                best = best.max(1 + score(dependent, dependents, memo, visiting));
            }
        }
        visiting.remove(id);
        memo.insert(id.to_string(), best);
        best
    }

    let mut memo = HashMap::new();
    let mut visiting = HashSet::new();
    for task in graph.tasks.values() {
        score(&task.id, &dependents, &mut memo, &mut visiting);
    }
    memo
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hierarchy::Hierarchy;
    use crate::policy::Policy;
    use crate::task::{Identity, IdentityKind, Lease, Link, TaskFrontmatter};
    use crate::workflow::Workflow;
    use chrono::{TimeZone, Utc};

    fn task(id: &str, status: &str, priority: &str) -> Task {
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap();
        Task {
            id: id.to_string(),
            front: TaskFrontmatter {
                title: format!("task {id}"),
                status: status.to_string(),
                key: None,
                key_aliases: Vec::new(),
                resolution: None,
                resolved_at: None,
                task_type: "task".to_string(),
                priority: priority.to_string(),
                labels: Vec::new(),
                links: Vec::new(),
                parent: None,
                checkpoint: None,
                component: None,
                verification: None,
                github: None,
                remote: None,
                repository: None,
                external_id: None,
                assignee: None,
                lease: None,
                reporter: None,
                created_at: now,
                updated_at: now,
                source_key: None,
                properties: BTreeMap::new(),
                extra: BTreeMap::new(),
            },
            body: String::new(),
        }
    }

    fn with_property(mut task: Task, owner: &str, value: Value) -> Task {
        task.front.properties.insert(owner.to_string(), value);
        task
    }

    fn assigned(mut task: Task, name: &str) -> Task {
        task.front.assignee = Some(Identity {
            name: name.to_string(),
            kind: IdentityKind::Agent,
        });
        task.front.lease = Some(Lease {
            claimed_at: Some(Utc::now()),
            lease_until: Some(Utc::now() + chrono::Duration::hours(1)),
        });
        task
    }

    fn depends(mut task: Task, dep: &str) -> Task {
        task.front.links.push(Link::dependency(dep));
        task
    }

    fn graph(tasks: Vec<Task>) -> Graph {
        let mut graph = Graph::from_tasks_with_workflow(tasks, Vec::new(), Workflow::standard());
        graph.hierarchy = Hierarchy::default();
        graph.policy = Policy::default();
        graph
    }

    fn ids(tasks: &[Task]) -> Vec<String> {
        tasks.iter().map(|task| task.id.clone()).collect()
    }

    #[test]
    fn parse_strategies_accepts_comma_lists_and_default() {
        assert_eq!(
            parse_strategies("critical-path, touches").unwrap(),
            vec![Strategy::CriticalPath, Strategy::Touches]
        );
        assert_eq!(parse_strategies("").unwrap(), vec![Strategy::Default]);
        assert!(parse_strategies("bogus").is_err());
    }

    #[test]
    fn resolve_prefers_flag_over_config() {
        let config = SchedulerConfig {
            strategy: Some("touches".to_string()),
            ..SchedulerConfig::default()
        };
        assert_eq!(
            resolve(Some("default"), &config).unwrap(),
            vec![Strategy::Default]
        );
        assert_eq!(resolve(None, &config).unwrap(), vec![Strategy::Touches]);
        assert_eq!(
            resolve(None, &SchedulerConfig::default()).unwrap(),
            vec![Strategy::Default]
        );
    }

    #[test]
    fn critical_path_puts_the_task_that_unblocks_the_chain_first() {
        // c depends on b, d depends on c: b's critical score is 2 (c then d),
        // a's is 0. Both ready candidates share priority, so the kernel order
        // would list orb-aaaaaa first; critical-path promotes orb-bbbbbb.
        let a = task("orb-aaaaaa", "ready", "medium");
        let b = task("orb-bbbbbb", "ready", "medium");
        let c = depends(task("orb-cccccc", "ready", "medium"), "orb-bbbbbb");
        let d = depends(task("orb-dddddd", "ready", "medium"), "orb-cccccc");
        let graph = graph(vec![a, b.clone(), c, d]);

        let scores = critical_scores(&graph);
        assert_eq!(scores["orb-bbbbbb"], 2);
        assert_eq!(scores["orb-aaaaaa"], 0);

        let mut ready = graph.ready_tasks();
        assert_eq!(ids(&ready), vec!["orb-aaaaaa", "orb-bbbbbb"]);
        let outcome = apply(
            &graph,
            std::mem::take(&mut ready),
            &[Strategy::CriticalPath],
            None,
            false,
            &SchedulerConfig::default(),
        )
        .unwrap();
        assert_eq!(ids(&outcome.tasks), vec!["orb-bbbbbb", "orb-aaaaaa"]);
    }

    #[test]
    fn critical_scores_skip_terminal_dependents_and_survive_cycles() {
        // Diamond: a is blocked by b and c, both blocked by tail. The done
        // dependent of a contributes nothing: b and c each score 1 (tail
        // scores 0), so a scores 2.
        let a = task("orb-aaaaaa", "ready", "medium");
        let b = depends(task("orb-bbbbbb", "ready", "medium"), "orb-aaaaaa");
        let c = depends(task("orb-cccccc", "ready", "medium"), "orb-aaaaaa");
        let mut tail = depends(task("orb-dddddd", "ready", "medium"), "orb-bbbbbb");
        tail.front.links.push(Link::dependency("orb-cccccc"));
        let done_dependent = depends(task("orb-eeeeee", "done", "medium"), "orb-aaaaaa");
        // A dependency cycle must not hang the walk; both sides score > 0.
        let f = depends(task("orb-ffffff", "ready", "medium"), "orb-999999");
        let g = depends(task("orb-999999", "ready", "medium"), "orb-ffffff");
        let graph = graph(vec![a, b, c, tail, done_dependent, f, g]);

        let scores = critical_scores(&graph);
        assert_eq!(scores["orb-aaaaaa"], 2);
        assert_eq!(scores["orb-bbbbbb"], 1);
        assert_eq!(scores["orb-cccccc"], 1);
        assert_eq!(scores["orb-dddddd"], 0);
        assert!(scores["orb-ffffff"] >= 1);
        assert!(scores["orb-999999"] >= 1);
    }

    #[test]
    fn touches_claim_never_overlaps_another_assignee() {
        let held = assigned(
            with_property(
                task("orb-111111", "in_progress", "high"),
                "touches",
                serde_json::json!({"paths": ["src/shared", "docs"]}),
            ),
            "alice",
        );
        let overlap = with_property(
            task("orb-222222", "ready", "high"),
            "touches",
            serde_json::json!({"paths": ["src/shared/deep.rs"]}),
        );
        let free = with_property(
            task("orb-333333", "ready", "high"),
            "touches",
            serde_json::json!({"paths": ["src/other"]}),
        );
        let graph = graph(vec![held, overlap.clone(), free.clone()]);
        let ready = vec![overlap.clone(), free.clone()];
        let outcome = apply(
            &graph,
            ready,
            &[Strategy::Touches],
            Some("bob"),
            true,
            &SchedulerConfig::default(),
        )
        .unwrap();
        assert_eq!(ids(&outcome.tasks), vec!["orb-333333"]);

        // The same agent may take overlapping work.
        let outcome = apply(
            &graph,
            vec![overlap],
            &[Strategy::Touches],
            Some("alice"),
            true,
            &SchedulerConfig::default(),
        )
        .unwrap();
        assert_eq!(ids(&outcome.tasks), vec!["orb-222222"]);
    }

    #[test]
    fn touches_listing_sorts_safe_candidates_first() {
        let held = assigned(
            with_property(
                task("orb-111111", "in_progress", "high"),
                "touches",
                serde_json::json!({"paths": ["src/shared"]}),
            ),
            "alice",
        );
        let risky = with_property(
            task("orb-222222", "ready", "high"),
            "touches",
            serde_json::json!({"paths": ["src/shared/x.rs"]}),
        );
        let safe = task("orb-333333", "ready", "high");
        let graph = graph(vec![held, risky, safe.clone()]);
        let outcome = apply(
            &graph,
            graph.ready_tasks(),
            &[Strategy::Touches],
            None,
            false,
            &SchedulerConfig::default(),
        )
        .unwrap();
        assert_eq!(ids(&outcome.tasks), vec!["orb-333333", "orb-222222"]);
    }

    #[test]
    fn capability_prefers_tasks_the_agent_can_do() {
        let config = SchedulerConfig {
            capabilities: BTreeMap::from([("bob".to_string(), vec!["rust".to_string()])]),
            ..SchedulerConfig::default()
        };
        let needs_kotlin = with_property(
            task("orb-111111", "ready", "high"),
            "capabilities",
            serde_json::json!({"requires": ["kotlin"]}),
        );
        let needs_rust = with_property(
            task("orb-222222", "ready", "high"),
            "capabilities",
            serde_json::json!({"requires": ["rust"]}),
        );
        let open = task("orb-333333", "ready", "high");
        let graph = graph(vec![needs_kotlin, needs_rust.clone(), open.clone()]);
        let outcome = apply(
            &graph,
            graph.ready_tasks(),
            &[Strategy::Capability],
            Some("bob"),
            true,
            &config,
        )
        .unwrap();
        // rust and no-requirement tasks match bob; kotlin sorts last. Stable
        // order keeps orb-222222 before orb-333333.
        assert_eq!(
            ids(&outcome.tasks),
            vec!["orb-222222", "orb-333333", "orb-111111"]
        );
    }

    #[test]
    fn wip_refuses_a_claim_at_the_limit() {
        let config = SchedulerConfig {
            wip_limit: Some(1),
            ..SchedulerConfig::default()
        };
        let held = assigned(task("orb-111111", "in_progress", "high"), "bob");
        let next = task("orb-222222", "ready", "high");
        let graph = graph(vec![held, next]);
        let outcome = apply(
            &graph,
            graph.ready_tasks(),
            &[Strategy::Wip],
            Some("bob"),
            true,
            &config,
        )
        .unwrap();
        assert!(outcome.tasks.is_empty());
        assert_eq!(outcome.refusal, Some(Refusal::WipLimit));

        // Another agent under the limit still claims.
        let outcome = apply(
            &graph,
            graph.ready_tasks(),
            &[Strategy::Wip],
            Some("alice"),
            true,
            &config,
        )
        .unwrap();
        assert_eq!(ids(&outcome.tasks), vec!["orb-222222"]);
        assert_eq!(outcome.refusal, None);
    }

    #[test]
    fn wip_requires_a_configured_limit() {
        let graph = graph(vec![task("orb-111111", "ready", "high")]);
        assert!(
            apply(
                &graph,
                graph.ready_tasks(),
                &[Strategy::Wip],
                Some("bob"),
                true,
                &SchedulerConfig::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn config_validation_rejects_bad_strategies_and_wip() {
        let bad = SchedulerConfig {
            strategy: Some("bogus".to_string()),
            ..SchedulerConfig::default()
        };
        assert!(bad.validate().is_err());
        let no_limit = SchedulerConfig {
            strategy: Some("wip".to_string()),
            ..SchedulerConfig::default()
        };
        assert!(no_limit.validate().is_err());
        let zero = SchedulerConfig {
            strategy: Some("wip".to_string()),
            wip_limit: Some(0),
            ..SchedulerConfig::default()
        };
        assert!(zero.validate().is_err());
        let good = SchedulerConfig {
            strategy: Some("critical-path,touches".to_string()),
            wip_limit: Some(2),
            capabilities: BTreeMap::from([("devin".to_string(), vec!["rust".to_string()])]),
        };
        assert!(good.validate().is_ok());
    }
}
