//! Declarative transition policy (ADR 0022): `[policy]` rules in
//! `config.toml` guard the Board's workflow transitions — required fields,
//! evidence properties, WIP limits, attempt limits, reviewer/assignee
//! distinctness, and a readiness lint — plus post-functions that run after
//! a governed transition succeeds.
//!
//! Enforcement lives in [`crate::ops`], the shared operation layer every
//! surface funnels through: the CLI, `orbit batch`, and the MCP tools (which
//! spawn the CLI) all hit the same checks. A rule matches when its
//! conditions hold — the transition name (or `"*"`), an optional target
//! status (`to`), the Issue type (`types`), every listed Label (`labels`),
//! and the Board's applied profiles (`profiles`). Every set condition must
//! match; an empty scope matches everything.
//!
//! Validators run against the Task as it will be stored (post-transition);
//! a violation fails the whole operation with `POLICY_VIOLATION` and rolls
//! the in-memory change back. Post-functions (`add_labels`,
//! `set_properties`) then mutate the stored Task. Rules that declare
//! `max_attempts` keep a per-rule counter in the reserved `orbit`
//! property's `attempts` map, incremented only when the governed
//! transition succeeds.
//!
//! `lint = true` marks a rule as the Board's readiness lint: its field and
//! property checks also gate `Graph::is_ready` (so lint-failing Tasks leave
//! the `orbit ready` queue) and `orbit claim`, on top of guarding the
//! transition into the Ready status the rule's `to` names. This replaces
//! the hard-coded "ready brief" notion: readiness requirements are
//! declarative per Board.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::board::Config;
use crate::error::{OrbitError, Result, code};
use crate::graph::Graph;
use crate::hierarchy::{self, Hierarchy};
use crate::task::{self, Task};
use crate::workflow::{StatusCategory, TransitionDef, Workflow};

/// The `[policy]` section of `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PolicyConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<PolicyRule>,
}

impl PolicyConfig {
    /// True when no `[policy]` section was configured.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// A WIP limit: at most `max` Tasks may sit in `status` (exact status name)
/// or `category` (a workflow status category) when a governed transition
/// would add one more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WipLimit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<StatusCategory>,
    pub max: u32,
}

/// One declarative rule. See the module docs for the condition/validator/
/// post-function split.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Unique rule name; appears in violation details and attempt counters.
    pub name: String,
    /// The workflow transition this rule guards, or `"*"` for every
    /// transition. Lifecycle commands map to their workflow transitions:
    /// `claim`/`assign` → `claim`, `release` → `release`, `done`/`cancel` →
    /// `done`/`cancel`, `reopen` → `reopen`.
    pub transition: String,
    /// Optional target-status filter: the rule applies only when the
    /// transition's `to` equals this status name. Required when `lint` is
    /// set — it must name the workflow's Ready status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Issue types the rule applies to; empty matches every type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
    /// Labels the Task must carry (all of them); empty matches everything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Applied profile names (`[[profiles]]`) that scope the rule; empty
    /// matches every Board. A profile contributes rules through `orbit
    /// setup`, which records the profile before the rules take effect.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,
    /// Frontmatter fields the Task must have once the transition lands —
    /// the same vocabulary `[[board.types]].required` uses (`title`,
    /// `body`, `verification`, `properties`, `checkpoint`, `parent`, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub require_fields: Vec<String>,
    /// Property owners (`orbit property set <id> <owner> '<json>'`) that
    /// must be present — the evidence hook (`evidence`, `proof`, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub require_properties: Vec<String>,
    /// Refuse the transition when the target status or category already
    /// holds `max` other Tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wip_limit: Option<WipLimit>,
    /// Refuse the transition once this rule has governed `max` successful
    /// transitions of this Task; attempts are counted per rule in the
    /// `orbit` property's `attempts` map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    /// The post-transition assignee must not equal any of these names.
    /// `reporter` resolves to the Task's reporter, `author` to the last
    /// `orbit submit`ter (the reporter when never submitted), and any other
    /// entry is a literal name — use it for reviewer-not-author rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub distinct_from: Vec<String>,
    /// Readiness lint: the rule's field and property checks also gate
    /// `Graph::is_ready` and `orbit claim`, on top of guarding the
    /// transition into its `to` status. `to` must name the Ready status.
    #[serde(default)]
    pub lint: bool,
    /// Post-function: labels added to the Task after a governed transition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_labels: Vec<String>,
    /// Post-function: Properties set on the Task after a governed
    /// transition, keyed by owner; each value must be a JSON object.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub set_properties: BTreeMap<String, serde_json::Value>,
}

impl PolicyRule {
    /// Syntactic validation that needs no Board context: a usable name,
    /// a non-empty transition, valid label/type-ish scope entries, and a
    /// well-formed WIP limit. Semantic checks (transition/status/profile
    /// references) run in [`Policy::from_config`].
    pub(crate) fn validate_shape(&self) -> Result<()> {
        let invalid =
            |message: String| OrbitError::config(format!("policy rule `{}`: {message}", self.name));
        if !task::is_valid_label(&self.name) {
            return Err(invalid(format!(
                "name `{}` must be a lowercase slug",
                self.name
            )));
        }
        if self.transition.trim().is_empty() {
            return Err(invalid("transition must not be empty".to_string()));
        }
        if let Some(limit) = &self.wip_limit {
            if limit.status.is_some() == limit.category.is_some() {
                return Err(invalid(
                    "wip_limit needs exactly one of `status` or `category`".to_string(),
                ));
            }
            if limit.max == 0 {
                return Err(invalid("wip_limit.max must be positive".to_string()));
            }
        }
        if self.max_attempts == Some(0) {
            return Err(invalid("max_attempts must be positive".to_string()));
        }
        for label in &self.labels {
            if !task::is_valid_label(label) {
                return Err(invalid(format!("invalid label `{label}`")));
            }
        }
        for owner in self
            .require_properties
            .iter()
            .chain(self.set_properties.keys())
        {
            task::validate_property(owner, &json!({}))
                .map_err(|_| invalid(format!("invalid property owner `{owner}`")))?;
        }
        for (owner, value) in &self.set_properties {
            task::validate_property(owner, value)
                .map_err(|error| invalid(format!("set_properties.{owner}: {}", error.message)))?;
        }
        for label in &self.add_labels {
            if !task::is_valid_label(label) {
                return Err(invalid(format!("invalid add_labels entry `{label}`")));
            }
        }
        Ok(())
    }
}

/// The resolved policy for a Board: its rules plus the applied profile
/// names rules may scope to. Carried on [`Graph`] like the workflow and
/// hierarchy so every surface agrees.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Policy {
    pub rules: Vec<PolicyRule>,
    /// Names of the `[[profiles]]` records applied to the Board.
    applied_profiles: Vec<String>,
}

/// One failed check, phrased for the `POLICY_VIOLATION` details payload.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub rule: String,
    pub check: &'static str,
    pub message: String,
    pub hint: String,
}

impl Policy {
    /// Resolves `[policy]` against the Board config, validating rule
    /// references against the workflow, hierarchy, and applied profiles.
    pub fn from_config(config: &Config) -> Result<Self> {
        let policy = &config.policy;
        if policy.is_empty() {
            return Ok(Self::default());
        }
        let workflow = Workflow::from_config(config)?;
        let hierarchy = Hierarchy::from_config(&config.board)?;
        let applied_profiles: Vec<String> = config
            .profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect();
        let mut policy_set = Self {
            rules: Vec::new(),
            applied_profiles,
        };
        let mut names = std::collections::BTreeSet::new();
        for rule in &policy.rules {
            rule.validate_shape()?;
            if !names.insert(rule.name.as_str()) {
                return Err(OrbitError::config(format!(
                    "policy rule `{}` is defined more than once",
                    rule.name
                )));
            }
            if rule.transition != "*" && workflow.transition_named(&rule.transition).is_none() {
                return Err(OrbitError::config(format!(
                    "policy rule `{}` guards unknown transition `{}`; the workflow defines: {}",
                    rule.name,
                    rule.transition,
                    workflow.transition_names().join(", ")
                )));
            }
            if let Some(to) = &rule.to
                && workflow.status(to).is_none()
            {
                return Err(OrbitError::config(format!(
                    "policy rule `{}` targets unknown status `{to}`; the workflow defines: {}",
                    rule.name,
                    workflow.status_names().join(", ")
                )));
            }
            if rule.lint {
                match (rule.to.as_deref(), workflow.ready_status()) {
                    (Some(to), Some(ready)) if to == ready => {}
                    (Some(to), _) => {
                        return Err(OrbitError::config(format!(
                            "policy rule `{}` is a readiness lint but `to = \"{to}\"` is not the Ready status `{}`",
                            rule.name,
                            workflow.ready_status().unwrap_or("<none>")
                        )));
                    }
                    (None, _) => {
                        return Err(OrbitError::config(format!(
                            "policy rule `{}` is a readiness lint and must set `to` to the Ready status",
                            rule.name
                        )));
                    }
                }
            }
            for task_type in &rule.types {
                let normalized = hierarchy::normalize_type_name(task_type);
                if !hierarchy.is_known_type(&normalized) {
                    return Err(OrbitError::config(format!(
                        "policy rule `{}` scopes to unknown type `{task_type}`; the Board types are: {}",
                        rule.name,
                        hierarchy.type_names().join(", ")
                    )));
                }
            }
            for profile in &rule.profiles {
                if !policy_set.applied_profiles.contains(profile) {
                    return Err(OrbitError::config(format!(
                        "policy rule `{}` scopes to profile `{profile}`, which is not applied to this Board",
                        rule.name
                    )));
                }
            }
            if let Some(limit) = &rule.wip_limit
                && let Some(status) = &limit.status
                && workflow.status(status).is_none()
            {
                return Err(OrbitError::config(format!(
                    "policy rule `{}` wip_limit targets unknown status `{status}`",
                    rule.name
                )));
            }
        }
        policy_set.rules = policy.rules.clone();
        Ok(policy_set)
    }

    /// Rules whose conditions match `task` for the transition `name`
    /// landing on `to` — the definition-order subset enforcement runs.
    fn matching<'a>(&'a self, name: &str, task: &Task, to: Option<&str>) -> Vec<&'a PolicyRule> {
        self.rules
            .iter()
            .filter(|rule| {
                (rule.transition == "*" || rule.transition == name)
                    && rule
                        .to
                        .as_deref()
                        .is_none_or(|to_status| Some(to_status) == to)
                    && (rule.types.is_empty()
                        || rule
                            .types
                            .iter()
                            .any(|ty| hierarchy::normalize_type_name(ty) == task.front.task_type))
                    && task.has_labels(&rule.labels)
                    && (rule.profiles.is_empty()
                        || rule
                            .profiles
                            .iter()
                            .all(|profile| self.applied_profiles.contains(profile)))
            })
            .collect()
    }

    /// Field and property checks shared by transition validators and the
    /// readiness lint.
    fn field_violations(rule: &PolicyRule, task: &Task) -> Vec<Violation> {
        let mut violations = Vec::new();
        let missing: Vec<String> = rule
            .require_fields
            .iter()
            .filter(|field| !hierarchy::required_field_present(task, field))
            .cloned()
            .collect();
        if !missing.is_empty() {
            violations.push(Violation {
                rule: rule.name.clone(),
                check: "require_fields",
                message: format!(
                    "rule `{}` requires field(s) {}",
                    rule.name,
                    missing.join(", ")
                ),
                hint: format!(
                    "set them with `orbit update {} --verification ...` / `orbit property set {} <owner> '<json>'` or edit the Task file",
                    task.id, task.id
                ),
            });
        }
        let missing_properties: Vec<&String> = rule
            .require_properties
            .iter()
            .filter(|owner| {
                // Evidence only counts when `submit` recorded at least one item.
                let value = task.front.properties.get(*owner);
                if owner.as_str() == crate::review::EVIDENCE_OWNER {
                    !value
                        .and_then(|v| v.get("items"))
                        .and_then(|items| items.as_array())
                        .is_some_and(|items| !items.is_empty())
                } else {
                    value.is_none()
                }
            })
            .collect();
        if !missing_properties.is_empty() {
            let owners: Vec<&str> = missing_properties
                .iter()
                .map(|owner| owner.as_str())
                .collect();
            violations.push(Violation {
                rule: rule.name.clone(),
                check: "require_properties",
                message: format!(
                    "rule `{}` requires propertie(s) {}",
                    rule.name,
                    owners.join(", ")
                ),
                hint: format!(
                    "record evidence with `orbit property set {} <owner> '{{\"...\"}}'`",
                    task.id
                ),
            });
        }
        violations
    }

    /// Readiness-lint failures for `task`: field and property checks of
    /// every `lint` rule scoped to the Task's current status. Empty when
    /// the Task passes (or no lint rules apply).
    pub fn lint_failures(&self, task: &Task) -> Vec<Violation> {
        let status = task.front.status.as_str();
        self.rules
            .iter()
            .filter(|rule| {
                rule.lint
                    && rule.to.as_deref() == Some(status)
                    && (rule.types.is_empty()
                        || rule
                            .types
                            .iter()
                            .any(|ty| hierarchy::normalize_type_name(ty) == task.front.task_type))
                    && task.has_labels(&rule.labels)
                    && (rule.profiles.is_empty()
                        || rule
                            .profiles
                            .iter()
                            .all(|profile| self.applied_profiles.contains(profile)))
            })
            .flat_map(|rule| Self::field_violations(rule, task))
            .collect()
    }

    /// Whether `task` passes every readiness-lint rule that applies to its
    /// current status — the policy half of [`Graph::is_ready`].
    pub fn passes_lint(&self, task: &Task) -> bool {
        self.lint_failures(task).is_empty()
    }

    /// Validators for `rule` against the post-transition `task`. WIP counts
    /// exclude the Task itself; attempts read the `orbit` property's
    /// per-rule counter.
    fn validator_violations(
        &self,
        rule: &PolicyRule,
        graph: &Graph,
        task: &Task,
        before: Option<&Task>,
        workflow: &Workflow,
    ) -> Vec<Violation> {
        let mut violations = Self::field_violations(rule, task);

        // A WIP cap only gates a Task *entering* the limited scope; moving or
        // closing an unrelated Task must not trip it.
        let in_scope = |candidate: &Task| match (
            rule.wip_limit.as_ref().and_then(|l| l.status.as_ref()),
            rule.wip_limit.as_ref().and_then(|l| l.category),
        ) {
            (Some(status), _) => candidate.front.status == *status,
            (None, Some(category)) => {
                workflow.category_of(&candidate.front.status) == Some(category)
            }
            _ => false,
        };
        let entering = in_scope(task) && !before.is_some_and(in_scope);
        if let Some(limit) = rule.wip_limit.as_ref().filter(|_| entering) {
            let (count, scope) = match (&limit.status, limit.category) {
                (Some(status), None) => (
                    graph
                        .tasks
                        .values()
                        .filter(|other| other.id != task.id && other.front.status == *status)
                        .count(),
                    format!("status `{status}`"),
                ),
                (None, Some(category)) => (
                    graph
                        .tasks
                        .values()
                        .filter(|other| {
                            other.id != task.id
                                && workflow.category_of(&other.front.status) == Some(category)
                        })
                        .count(),
                    format!("category `{}`", category.as_str()),
                ),
                _ => unreachable!("wip_limit shape is validated"),
            };
            if count >= limit.max as usize {
                violations.push(Violation {
                    rule: rule.name.clone(),
                    check: "wip_limit",
                    message: format!(
                        "rule `{}` caps {} at {} Tasks; it already holds {count}",
                        rule.name, scope, limit.max
                    ),
                    hint: format!("finish or move Tasks out of {} before retrying", scope),
                });
            }
        }

        if let Some(max) = rule.max_attempts {
            let attempts = attempt_count(task, &rule.name);
            if attempts >= max {
                violations.push(Violation {
                    rule: rule.name.clone(),
                    check: "max_attempts",
                    message: format!(
                        "rule `{}` allows at most {max} governed transition(s); this Task has {attempts}",
                        rule.name
                    ),
                    hint: "the limit counts successful transitions; adjust the rule or the Task's `orbit` property".to_string(),
                });
            }
        }

        // The acting identity is the assignee after the transition (review sets it
        // temporarily) or the claim holder before it: closing transitions clear
        // the assignee, so the pre-transition holder is who is acting.
        let actors: Vec<&str> = task
            .front
            .assignee
            .iter()
            .chain(before.and_then(|b| b.front.assignee.as_ref()))
            .map(|assignee| assignee.name.as_str())
            .collect();
        if !rule.distinct_from.is_empty() && !actors.is_empty() {
            let forbidden: Vec<String> = rule
                .distinct_from
                .iter()
                .map(|entry| match entry.as_str() {
                    "reporter" => task.front.reporter.clone().unwrap_or_default(),
                    // The author is the last `orbit submit`ter, falling back
                    // to the reporter when the work was never submitted.
                    "author" => crate::review::author_name(task).unwrap_or_default(),
                    literal => literal.to_string(),
                })
                .filter(|name| !name.is_empty())
                .collect();
            if let Some(actor) = actors.iter().find(|actor| {
                forbidden
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(actor))
            }) {
                violations.push(Violation {
                    rule: rule.name.clone(),
                    check: "distinct_from",
                    message: format!(
                        "rule `{}` requires the assignee to differ from {}; `{}` is both",
                        rule.name,
                        forbidden.join(", "),
                        actor
                    ),
                    hint: "assign the transition to a different identity".to_string(),
                });
            }
        }

        violations
    }

    /// Runs every matching rule's validators for `transition_name` landing
    /// on `to_status`. Returns all violations across rules; an empty Vec
    /// means the transition may proceed.
    pub fn check(
        &self,
        graph: &Graph,
        task: &Task,
        before: Option<&Task>,
        transition_name: &str,
        to_status: Option<&str>,
    ) -> Vec<Violation> {
        let workflow = &graph.workflow;
        self.matching(transition_name, task, to_status)
            .into_iter()
            .flat_map(|rule| self.validator_violations(rule, graph, task, before, workflow))
            .collect()
    }

    /// Post-functions for every rule that matched and passed validation:
    /// labels, properties, and the per-rule attempt counter. Mutates `task`
    /// in place; the caller persists it.
    pub fn apply_post_functions(&self, task: &mut Task, transition_name: &str, now: DateTime<Utc>) {
        let to_status = Some(task.front.status.as_str());
        for rule in self.matching(transition_name, task, to_status) {
            if !rule.add_labels.is_empty() {
                let labels = crate::task::normalize_labels(&rule.add_labels)
                    .unwrap_or_else(|_| rule.add_labels.clone());
                task.add_labels(&labels, now);
            }
            for (owner, value) in &rule.set_properties {
                if task::validate_property(owner, value).is_ok() {
                    task.front.properties.insert(owner.clone(), value.clone());
                }
            }
            if rule.max_attempts.is_some() {
                increment_attempt(task, &rule.name);
            }
        }
        task.touch(now);
    }
}

/// The attempt counter for `rule` on `task`: reads the reserved `orbit`
/// property's `attempts` map.
fn attempt_count(task: &Task, rule: &str) -> u32 {
    task.front
        .properties
        .get("orbit")
        .and_then(|value| value.get("attempts"))
        .and_then(|attempts| attempts.get(rule))
        .and_then(|count| count.as_u64())
        .unwrap_or(0) as u32
}

/// Increments the per-rule attempt counter inside the `orbit` property,
/// preserving any other keys the property already carries.
fn increment_attempt(task: &mut Task, rule: &str) {
    let mut orbit = task
        .front
        .properties
        .get("orbit")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut attempts = orbit
        .get("attempts")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let next = attempts
        .get(rule)
        .and_then(|count| count.as_u64())
        .unwrap_or(0)
        + 1;
    attempts.insert(rule.to_string(), json!(next));
    orbit.insert("attempts".to_string(), serde_json::Value::Object(attempts));
    task.front
        .properties
        .insert("orbit".to_string(), serde_json::Value::Object(orbit));
}

/// The `POLICY_VIOLATION` error for a failed check set: one message line
/// per violation plus remediation hints, with the full list in `details`.
pub fn violation_error(id: &str, transition: &str, violations: Vec<Violation>) -> OrbitError {
    let mut lines: Vec<String> = violations
        .iter()
        .map(|violation| format!("{}: {}", violation.rule, violation.message))
        .collect();
    for violation in &violations {
        lines.push(format!("  hint: {}", violation.hint));
    }
    OrbitError::new(
        code::POLICY_VIOLATION,
        format!(
            "{id}: policy violation on `{transition}`\n{}",
            lines.join("\n")
        ),
    )
    .with_details(json!({
        "issue": id,
        "transition": transition,
        "violations": violations.iter().map(|violation| json!({
            "rule": violation.rule,
            "check": violation.check,
            "message": violation.message,
            "hint": violation.hint,
        })).collect::<Vec<_>>(),
    }))
}

/// Enforces policy for `transition_name` on the Task `id` after its
/// transition was applied to the in-memory Graph. On violation the Task is
/// restored to `before` and the `POLICY_VIOLATION` error is returned, so
/// callers (CLI, batch, MCP) share one enforcement point. `to_status` is
/// the transition's target status when known (used for `to`-scoped rules).
pub fn enforce(
    graph: &mut Graph,
    id: &str,
    transition_name: &str,
    to_status: Option<&str>,
    before: &Task,
    now: DateTime<Utc>,
) -> Result<()> {
    if graph.policy.rules.is_empty() {
        return Ok(());
    }
    let resolved = graph.resolve(id)?;
    let task = graph
        .tasks
        .get(&resolved)
        .cloned()
        .unwrap_or_else(|| before.clone());
    let violations = graph
        .policy
        .check(graph, &task, Some(before), transition_name, to_status);
    if !violations.is_empty() {
        graph.tasks.insert(resolved.clone(), before.clone());
        return Err(violation_error(&resolved, transition_name, violations));
    }
    let task = graph.tasks.get_mut(&resolved).expect("task present");
    graph
        .policy
        .apply_post_functions(task, transition_name, now);
    Ok(())
}

/// Readiness-lint gate for `orbit claim` and [`Graph::is_ready`]: returns
/// the `POLICY_VIOLATION` error when lint rules fail, `Ok` otherwise.
pub fn lint_error(graph: &Graph, id: &str) -> Result<()> {
    let task = graph.get(id)?;
    let violations = graph.policy.lint_failures(task);
    if violations.is_empty() {
        return Ok(());
    }
    Err(violation_error(id, "ready-lint", violations))
}

/// The transition definition `name` will run from `status`, when the
/// workflow defines it — used to learn the target status for `to`-scoped
/// rules before enforcement.
pub fn target_status<'w>(workflow: &'w Workflow, name: &str, status: &str) -> Option<&'w str> {
    let transition: Option<&TransitionDef> = workflow
        .transitions
        .iter()
        .find(|transition| transition.name == name && transition.allows_from(status));
    transition.map(|transition| transition.to.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::BoardConfig;
    use crate::graph::Graph;
    use crate::task::TaskFrontmatter;
    use chrono::TimeZone;
    use std::collections::BTreeMap;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap()
    }

    fn task(id: &str, status: &str) -> Task {
        Task {
            id: id.to_string(),
            front: TaskFrontmatter {
                title: "t".to_string(),
                status: status.to_string(),
                key: None,
                key_aliases: Vec::new(),
                resolution: None,
                task_type: "task".to_string(),
                priority: "medium".to_string(),
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
                reporter: Some("rodrigo".to_string()),
                created_at: now(),
                updated_at: now(),
                source_key: None,
                properties: BTreeMap::new(),
                extra: BTreeMap::new(),
            },
            body: "body".to_string(),
        }
    }

    fn config_with_policy(rules: &str) -> Config {
        let text = format!(
            r#"
[board]

[policy]
{rules}
"#
        );
        let mut config: Config = toml::from_str(&text).unwrap();
        config.board = BoardConfig::default();
        config
    }

    #[test]
    fn empty_policy_is_default() {
        let config = Config::default();
        assert!(Policy::from_config(&config).unwrap().rules.is_empty());
    }

    #[test]
    fn rule_guards_unknown_transition_and_status() {
        let config = config_with_policy(
            r#"
[[policy.rules]]
name = "bad"
transition = "nope"
"#,
        );
        let error = Policy::from_config(&config).unwrap_err();
        assert!(error.message.contains("unknown transition"));

        let config = config_with_policy(
            r#"
[[policy.rules]]
name = "bad2"
transition = "done"
to = "nope"
"#,
        );
        assert!(Policy::from_config(&config).is_err());
    }

    #[test]
    fn lint_rule_must_target_the_ready_status() {
        let config = config_with_policy(
            r#"
[[policy.rules]]
name = "lint"
transition = "*"
to = "done"
lint = true
"#,
        );
        let error = Policy::from_config(&config).unwrap_err();
        assert!(error.message.contains("Ready status"));
    }

    #[test]
    fn matching_rules_validate_and_post_functions_run() {
        let config = config_with_policy(
            r#"
[[policy.rules]]
name = "gate"
transition = "done"
require_fields = ["verification"]
require_properties = ["proof"]
max_attempts = 2
add_labels = ["reviewed"]
set_properties = { verdict = { ok = true } }
"#,
        );
        let policy = Policy::from_config(&config).unwrap();
        assert_eq!(policy.rules.len(), 1);

        let mut t = task("orb-aaaaaa", "ready");
        t.front.status = "done".to_string();
        let graph = Graph::from_tasks(vec![task("orb-bbbbbb", "in_progress")]);
        // Post-state check: missing verification + evidence fails.
        let violations = policy.check(&graph, &t, None, "done", Some("done"));
        assert_eq!(violations.len(), 2);
        assert!(violations.iter().any(|v| v.check == "require_fields"));
        assert!(violations.iter().any(|v| v.check == "require_properties"));

        t.front.verification = Some("cargo test".to_string());
        t.front
            .properties
            .insert("proof".to_string(), json!({"command": "cargo test"}));
        assert!(
            policy
                .check(&graph, &t, None, "done", Some("done"))
                .is_empty()
        );

        policy.apply_post_functions(&mut t, "done", now());
        assert!(t.front.labels.contains(&"reviewed".to_string()));
        assert_eq!(t.front.properties["verdict"], json!({"ok": true}));
        // First governed transition counts as attempt 1.
        assert_eq!(attempt_count(&t, "gate"), 1);
        policy.apply_post_functions(&mut t, "done", now());
        assert_eq!(attempt_count(&t, "gate"), 2);
        // Third application would violate max_attempts = 2.
        let violations = policy.check(&graph, &t, None, "done", Some("done"));
        assert!(violations.iter().any(|v| v.check == "max_attempts"));
    }

    #[test]
    fn scoped_rules_match_on_type_label_and_profile() {
        let config: Config = toml::from_str(
            r#"
[[profiles]]
name = "acme"
requires = []

[policy]

[[policy.rules]]
name = "bug-evidence"
transition = "*"
types = ["bug"]
require_properties = ["proof"]

[[policy.rules]]
name = "labeled"
transition = "*"
labels = ["needs-evidence"]
require_fields = ["verification"]
"#,
        )
        .unwrap();
        let policy = Policy::from_config(&config).unwrap();
        let graph = Graph::from_tasks(vec![]);

        let mut bug = task("orb-1", "ready");
        bug.front.task_type = "bug".to_string();
        let violations = policy.check(&graph, &bug, None, "promote", Some("ready"));
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "bug-evidence");

        let mut plain = task("orb-2", "ready");
        plain.front.labels = vec!["needs-evidence".to_string()];
        plain.front.verification = Some("cargo test".to_string());
        assert!(
            policy
                .check(&graph, &plain, None, "promote", Some("ready"))
                .is_empty()
        );

        // Profile scope: a rule scoped to an unapplied profile never fires.
        let config: Config = toml::from_str(
            r#"
[policy]

[[policy.rules]]
name = "acme-only"
transition = "*"
profiles = ["acme"]
require_fields = ["verification"]
"#,
        )
        .unwrap();
        assert!(Policy::from_config(&config).is_err());
    }

    #[test]
    fn wip_limit_counts_other_tasks_in_scope() {
        let config = config_with_policy(
            r#"
[[policy.rules]]
name = "wip"
transition = "claim"
wip_limit = { status = "in_progress", max = 1 }
"#,
        );
        let policy = Policy::from_config(&config).unwrap();
        let busy = task("orb-bbbbbb", "in_progress");
        let mut claiming = task("orb-aaaaaa", "in_progress");
        let graph = Graph::from_tasks(vec![busy, claiming.clone()]);
        let violations = policy.check(&graph, &claiming, None, "claim", Some("in_progress"));
        assert!(violations.iter().any(|v| v.check == "wip_limit"));
        // The Task itself is not counted against its own limit.
        let graph = Graph::from_tasks(vec![claiming.clone()]);
        assert!(
            policy
                .check(&graph, &claiming, None, "claim", Some("in_progress"))
                .is_empty()
        );
        claiming.front.status = "ready".to_string();
    }

    #[test]
    fn lint_failures_gate_readiness() {
        let config = config_with_policy(
            r#"
[[policy.rules]]
name = "ready-lint"
transition = "*"
to = "ready"
lint = true
require_fields = ["verification"]
"#,
        );
        let policy = Policy::from_config(&config).unwrap();
        let vague = task("orb-aaaaaa", "ready");
        assert_eq!(policy.lint_failures(&vague).len(), 1);

        let mut briefed = task("orb-bbbbbb", "ready");
        briefed.front.verification = Some("cargo test".to_string());
        assert!(policy.lint_failures(&briefed).is_empty());

        // Lint only applies to the Ready status the rule names.
        let backlog = task("orb-cccccc", "todo");
        assert!(policy.lint_failures(&backlog).is_empty());
    }

    #[test]
    fn distinct_from_checks_assignee_against_reporter() {
        let config = config_with_policy(
            r#"
[[policy.rules]]
name = "review"
transition = "done"
distinct_from = ["reporter"]
"#,
        );
        let policy = Policy::from_config(&config).unwrap();
        let graph = Graph::from_tasks(vec![]);
        let mut t = task("orb-aaaaaa", "in_review");
        t.front.assignee = Some(crate::task::Identity {
            name: "rodrigo".to_string(),
            kind: crate::task::IdentityKind::Human,
        });
        let violations = policy.check(&graph, &t, None, "done", Some("done"));
        assert!(violations.iter().any(|v| v.check == "distinct_from"));
    }
}
