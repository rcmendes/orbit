//! Move planner for the Board view (ADR 0023): which CLI command a drop or a
//! menu pick runs, or why it is refused. Pure logic over the JSON that
//! `orbit show --json` and `orbit capabilities --json` (`data.workflow`) emit.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimState {
    None,
    Live,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub title: String,
    pub body: String,
    pub ok: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub label: String,
    pub transition: String,
    pub args: Vec<String>,
    pub confirm: Option<Confirm>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Refused { reason: String },
    Run(Vec<Action>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub id: String,
    pub name: String,
    pub statuses: Vec<String>,
    pub home: Option<String>,
}

/// Input transitions the board can't run: each needs text only the CLI collects.
const NEEDS_INPUT: [(&str, &str); 6] = [
    (
        "submit",
        "Submitting for review needs evidence. Use orbit submit.",
    ),
    (
        "approve",
        "Approving needs a review verdict. Use orbit review.",
    ),
    (
        "request_changes",
        "Requesting changes needs a review note. Use orbit review.",
    ),
    ("ask", "Asking needs a question. Use orbit ask."),
    ("escalate", "Escalating needs a reason. Use the CLI."),
    (
        "answer",
        "Answering needs the answer text. Use orbit answer.",
    ),
];

fn needs_input(transition: &str) -> Option<&'static str> {
    NEEDS_INPUT
        .iter()
        .find(|(n, _)| *n == transition)
        .map(|(_, m)| *m)
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn list<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn cat_of<'a>(wf: &'a Value, status: &str) -> &'a str {
    list(wf, "statuses")
        .iter()
        .find(|d| s(d, "name") == status)
        .map_or("", |d| s(d, "category"))
}

fn is_ready(wf: &Value, status: &str) -> bool {
    list(wf, "statuses")
        .iter()
        .any(|d| s(d, "name") == status && d["ready"] == true)
}

fn closed(cat: &str) -> bool {
    cat == "done" || cat == "canceled"
}

/// Mirrors `TransitionDef::allows_from`: `"*"` matches every status.
fn allows(t: &Value, status: &str) -> bool {
    list(t, "from")
        .iter()
        .any(|f| f == "*" || f.as_str() == Some(status))
}

/// The statuses some `names` transition lands on.
fn targets_of<'a>(wf: &'a Value, names: &[&str]) -> Vec<&'a str> {
    list(wf, "transitions")
        .iter()
        .filter(|t| names.contains(&s(t, "name")))
        .map(|t| s(t, "to"))
        .collect()
}

/// `in_progress` -> `In progress`.
fn label(name: &str) -> String {
    let text = name.replace('_', " ");
    let mut chars = text.chars();
    chars
        .next()
        .map_or(String::new(), |c| c.to_uppercase().chain(chars).collect())
}

// shortcut: `blocking` holds Issue ids in real output; resolving them to keys needs the snapshot.
fn blocking(issue: &Value) -> Vec<&str> {
    list(issue, "blocking")
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

fn lease(issue: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s(issue, "lease_until"))
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn ago(d: chrono::Duration) -> String {
    let m = (d.num_seconds().abs() as f64 / 60.0).round() as i64;
    if m < 60 {
        return format!("{m} min");
    }
    let h = (m as f64 / 60.0).round() as i64;
    if h < 48 {
        format!("{h} h")
    } else {
        format!("{} days", (h as f64 / 24.0).round() as i64)
    }
}

/// `key` is null on Issues without a project key; the CLI also takes the id.
fn key_of(issue: &Value) -> &str {
    Some(s(issue, "key"))
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| s(issue, "id"))
}

/// An Ask without `answered_at` (review::open_ask); answering keeps the object.
fn open_ask(issue: &Value) -> Option<&Value> {
    let ask = &issue["properties"]["orbit"]["ask"];
    (ask.is_object() && ask.get("answered_at").is_none()).then_some(ask)
}

/// `orbit assign` only travels the `claim` transition and lands on its `to`.
fn on_claim_route(wf: &Value, from: &str, to: &str) -> bool {
    list(wf, "transitions")
        .iter()
        .any(|t| s(t, "name") == "claim" && allows(t, from) && s(t, "to") == to)
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

fn refuse(reason: impl Into<String>) -> Plan {
    Plan::Refused {
        reason: reason.into(),
    }
}

pub fn claim_state(issue: &Value, now: DateTime<Utc>) -> ClaimState {
    match lease(issue) {
        Some(until) if !s(issue, "assignee").is_empty() => {
            if until > now {
                ClaimState::Live
            } else {
                ClaimState::Stale
            }
        }
        _ => ClaimState::None,
    }
}

pub fn columns(wf: &Value) -> Vec<Column> {
    let st = list(wf, "statuses");
    let names = |pred: &dyn Fn(&Value) -> bool| -> Vec<String> {
        st.iter()
            .filter(|d| pred(d))
            .map(|d| s(d, "name").to_string())
            .collect()
    };
    let todo =
        names(&|d| s(d, "category") == "todo" && d["ready"] != true && s(d, "name") != "blocked");
    let ready = names(&|d| d["ready"] == true);
    let closed_st = names(&|d| closed(s(d, "category")));
    let mut out = vec![
        Column {
            id: "blocked".into(),
            name: "Blocked".into(),
            statuses: vec![],
            home: None,
        },
        Column {
            id: "todo".into(),
            name: "Todo".into(),
            home: todo.iter().any(|n| n == "todo").then(|| "todo".into()),
            statuses: todo,
        },
        Column {
            id: "ready".into(),
            name: label(ready.first().map_or("ready", String::as_str)),
            home: ready.first().cloned(),
            statuses: ready,
        },
    ];
    out.extend(
        names(&|d| s(d, "category") == "active")
            .into_iter()
            .map(|n| Column {
                id: n.clone(),
                name: label(&n),
                statuses: vec![n.clone()],
                home: Some(n),
            }),
    );
    out.push(Column {
        id: "done".into(),
        name: "Done".into(),
        home: closed_st.iter().any(|n| n == "done").then(|| "done".into()),
        statuses: closed_st,
    });
    out
}

pub fn column_of(issue: &Value, wf: &Value) -> String {
    let status = s(issue, "status");
    let cat = cat_of(wf, status);
    if closed(cat) {
        "done".into()
    } else if cat == "todo" && (!blocking(issue).is_empty() || status == "blocked") {
        "blocked".into()
    } else if is_ready(wf, status) {
        "ready".into()
    } else if cat == "todo" {
        "todo".into()
    } else {
        status.into()
    }
}

pub fn plan_move(
    issue: &Value,
    target: &str,
    wf: &Value,
    viewer: &str,
    now: DateTime<Utc>,
) -> Plan {
    let status = s(issue, "status");
    if status == target {
        return refuse("Already in this column.");
    }
    // The CLI refuses every transition but `answer` with ASK_OPEN.
    if open_ask(issue).is_some() {
        return refuse("Answer the open Ask first. Use orbit answer.");
    }
    let target_cat = cat_of(wf, target);
    if column_of(issue, wf) == "blocked" && target_cat != "canceled" {
        let by = blocking(issue);
        return refuse(if by.is_empty() {
            "In a blocked Status. Unblock it from the CLI.".to_string()
        } else {
            let when = if by.len() > 1 {
                "they close"
            } else {
                "that closes"
            };
            format!("Blocked by {}. It moves on when {when}.", by.join(", "))
        });
    }
    let who = s(issue, "assignee");
    match (claim_state(issue, now), lease(issue)) {
        (ClaimState::Live, Some(until)) => {
            let at = until.with_timezone(&chrono::Local).format("%H:%M");
            return refuse(format!(
                "Claimed by {who} until {at}. A claimed Issue can't be moved."
            ));
        }
        (ClaimState::Stale, Some(until)) => {
            return refuse(format!(
                "{who}'s lease expired {} ago. Take over or release it first.",
                ago(now - until)
            ));
        }
        _ => {}
    }
    let ts: Vec<&Value> = list(wf, "transitions")
        .iter()
        .filter(|t| allows(t, status) && s(t, "to") == target)
        .collect();
    let Some(first) = ts.first() else {
        return refuse(format!(
            "No transition goes from {} to {}.",
            label(status),
            label(target)
        ));
    };
    let usable: Vec<&Value> = ts
        .iter()
        .copied()
        .filter(|t| needs_input(s(t, "name")).is_none())
        .collect();
    if usable.is_empty() {
        return refuse(needs_input(s(first, "name")).unwrap_or_default());
    }
    // Review guard: a submitted Issue only closes through `orbit review`.
    if target_cat == "done" && targets_of(wf, &["submit"]).contains(&status) {
        return refuse(needs_input("approve").unwrap_or_default());
    }
    if closed(target_cat) && !usable.iter().any(|t| s(t, "name") == "reopen") {
        let by = blocking(issue);
        // The CLI lets `cancel` through an open blocker but never `done`.
        if target_cat == "done" && !by.is_empty() {
            return refuse(format!("Still blocked by {}.", by.join(", ")));
        }
        match issue["open_sub_issue_count"].as_u64().unwrap_or(0) {
            0 => {}
            1 => return refuse("1 sub-issue is still open."),
            n => return refuse(format!("{n} sub-issues are still open.")),
        }
    }
    Plan::Run(
        usable
            .into_iter()
            .map(|t| action_for(issue, t, wf, viewer))
            .collect(),
    )
}

fn action_for(issue: &Value, t: &Value, wf: &Value, viewer: &str) -> Action {
    let (name, to) = (s(t, "name"), s(t, "to"));
    let (key, ver) = (key_of(issue), s(issue, "updated_at"));
    let mut confirm = None;
    let args = match name {
        "done" | "cancel" => argv(&[name, key, "--by", viewer, "--if-version", ver]),
        "reopen" => argv(&["reopen", key, "--if-version", ver]),
        _ if on_claim_route(wf, s(issue, "status"), to) => {
            let other = s(issue, "assignee");
            let mut a = argv(&["assign", key, "--to", viewer, "--kind", "human"]);
            if !other.is_empty() && other != viewer {
                a.push("--force".into());
                confirm = Some(Confirm {
                    title: format!("Replace {other} as Assignee?"),
                    body: format!(
                        "{key} is assigned to {other} without a lease. Moving it to {} makes you the Assignee instead.",
                        label(to)
                    ),
                    ok: "Replace and move".into(),
                });
            }
            a.extend(argv(&["--if-version", ver]));
            a
        }
        _ => argv(&["transition", key, name, "--by", viewer, "--if-version", ver]),
    };
    Action {
        label: label(name),
        transition: name.into(),
        args,
        confirm,
    }
}

pub fn plan_column(issue: &Value, col: &str, wf: &Value, viewer: &str, now: DateTime<Utc>) -> Plan {
    if column_of(issue, wf) == col {
        return refuse("Already in this column.");
    }
    if col == "blocked" {
        return refuse(
            "Nothing is dropped onto Blocked. Issues land there when they gain an open blocker.",
        );
    }
    let cols = columns(wf);
    let statuses = cols
        .iter()
        .find(|c| c.id == col)
        .map_or(&[][..], |c| &c.statuses[..]);
    let mut run = Vec::new();
    let mut refusals = Vec::new();
    // Cancel is menu-only: dropping on Done never cancels.
    for st in statuses.iter().filter(|st| cat_of(wf, st) != "canceled") {
        match plan_move(issue, st, wf, viewer, now) {
            Plan::Run(a) => run.extend(a),
            Plan::Refused { reason } => refusals.push(reason),
        }
    }
    if !run.is_empty() {
        return Plan::Run(run);
    }
    let reason = refusals
        .iter()
        .find(|r| !r.starts_with("No transition"))
        .or(refusals.first())
        .map_or("Nothing can move here.", String::as_str);
    refuse(reason)
}

pub fn stale_actions(issue: &Value, wf: &Value, viewer: &str, now: DateTime<Utc>) -> Vec<Action> {
    let (ClaimState::Stale, Some(until)) = (claim_state(issue, now), lease(issue)) else {
        return vec![];
    };
    let (key, ver, agent) = (key_of(issue), s(issue, "updated_at"), s(issue, "assignee"));
    let status = s(issue, "status");
    let since = ago(now - until);
    let allowed = |name: &str| {
        list(wf, "transitions")
            .iter()
            .find(|t| s(t, "name") == name && allows(t, status))
    };
    let mut out = vec![];
    if allowed("claim").is_some() {
        out.push(Action {
            label: "Take over".into(),
            transition: "claim".into(),
            args: argv(&["assign", key, "--to", viewer, "--kind", "human", "--force", "--if-version", ver]),
            confirm: Some(Confirm {
                title: format!("Take over {key}?"),
                body: format!(
                    "{agent}'s lease expired {since} ago. You become the Assignee, without a lease. The Issue stays in {} and keeps its checkpoint. If {agent} comes back, its next heartbeat fails.",
                    label(status)
                ),
                ok: "Take over".into(),
            }),
        });
    }
    if let Some(t) = allowed("release") {
        out.push(Action {
            label: "Release".into(),
            transition: "release".into(),
            args: argv(&["release", key, "--by", viewer, "--force", "--if-version", ver]),
            confirm: Some(Confirm {
                title: format!("Release {key}?"),
                body: format!(
                    "Clears {agent}'s claim and moves the Issue to {}, where any agent can claim it. The checkpoint stays.",
                    label(s(t, "to"))
                ),
                ok: "Release".into(),
            }),
        });
    }
    out
}

pub fn needs_human(issue: &Value, wf: &Value, now: DateTime<Utc>) -> Option<String> {
    let status = s(issue, "status");
    if closed(cat_of(wf, status)) {
        return None;
    }
    let orbit = &issue["properties"]["orbit"];
    if let Some(ask) = open_ask(issue) {
        return Some(format!("Answer: {}", s(ask, "question")));
    }
    if targets_of(wf, &["ask", "escalate"]).contains(&status) {
        let by = s(&orbit["escalation"], "by");
        return Some(if by.is_empty() {
            "Decide: escalated".into()
        } else {
            format!("Decide: escalated by {by}")
        });
    }
    if claim_state(issue, now) == ClaimState::Stale {
        return Some("Take over or release".into());
    }
    let labels = list(issue, "labels");
    let has = |l: &str| labels.iter().any(|x| x == l);
    if has("needs-triage") {
        Some("Triage this Issue".into())
    } else if has("needs-info") {
        Some("Add the missing information".into())
    } else {
        None
    }
}

pub fn ready_for_human(issue: &Value, wf: &Value) -> bool {
    is_ready(wf, s(issue, "status"))
        && list(issue, "labels").iter().any(|l| l == "ready-for-human")
        && s(issue, "assignee").is_empty()
        && blocking(issue).is_empty()
}

fn confirm_json(c: &Option<Confirm>) -> Option<Value> {
    c.as_ref()
        .map(|c| json!({ "title": c.title, "body": c.body, "ok": c.ok }))
}

/// The page-facing shape of a Plan: labels and confirms, never argv.
fn plan_json(p: Plan) -> Value {
    match p {
        Plan::Refused { reason } => json!({ "ok": false, "reason": reason }),
        Plan::Run(actions) => {
            let actions: Vec<Value> = actions
                .iter()
                .map(|a| {
                    let mut v = json!({ "label": a.label, "transition": a.transition });
                    if let Some(c) = confirm_json(&a.confirm) {
                        v["confirm"] = c;
                    }
                    v
                })
                .collect();
            json!({ "ok": true, "actions": actions })
        }
    }
}

pub fn annotate(issue: &Value, wf: &Value, viewer: &str, now: DateTime<Utc>) -> Value {
    let moves: Map<String, Value> = columns(wf)
        .into_iter()
        .map(|c| {
            let p = plan_column(issue, &c.id, wf, viewer, now);
            (c.id, plan_json(p))
        })
        .collect();
    let menu: Map<String, Value> = list(wf, "statuses")
        .iter()
        .map(|d| {
            let st = s(d, "name");
            (
                st.to_string(),
                plan_json(plan_move(issue, st, wf, viewer, now)),
            )
        })
        .collect();
    let stale: Vec<Value> = stale_actions(issue, wf, viewer, now)
        .iter()
        .map(|a| json!({ "label": a.label, "confirm": confirm_json(&a.confirm) }))
        .collect();
    let claim = match claim_state(issue, now) {
        ClaimState::None => "none",
        ClaimState::Live => "live",
        ClaimState::Stale => "stale",
    };
    json!({
        "column": column_of(issue, wf),
        "needs_human": needs_human(issue, wf, now),
        "ready_for_human": ready_for_human(issue, wf),
        "claim": claim,
        "moves": moves,
        "menu": menu,
        "stale_actions": stale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::Workflow;
    use chrono::Duration;
    use serde_json::{Value, json};

    fn wf_of(w: Workflow) -> Value {
        json!({ "statuses": w.statuses, "transitions": w.transitions })
    }
    fn std_wf() -> Value {
        wf_of(Workflow::standard())
    }
    fn agentic_wf() -> Value {
        wf_of(Workflow::agentic())
    }
    fn now() -> DateTime<Utc> {
        "2026-11-23T12:00:00Z".parse().unwrap()
    }
    fn issue(status: &str, patch: Value) -> Value {
        let mut v = json!({
            "id": "orb-a1b2c3", "key": "ORB-1", "status": status, "assignee": null,
            "assignee_kind": null, "lease_until": null, "blocking": [], "labels": [],
            "open_sub_issue_count": 0, "properties": {}, "updated_at": "V",
        });
        for (k, val) in patch.as_object().unwrap() {
            v[k] = val.clone();
        }
        v
    }
    fn lease(mins: i64) -> Value {
        json!((now() + Duration::minutes(mins)).to_rfc3339())
    }
    fn actions(p: Plan) -> Vec<Action> {
        match p {
            Plan::Run(a) => a,
            Plan::Refused { reason } => panic!("refused: {reason}"),
        }
    }
    fn reason(p: Plan) -> String {
        match p {
            Plan::Refused { reason } => reason,
            Plan::Run(a) => panic!("ran: {a:?}"),
        }
    }
    fn mv(i: &Value, target: &str) -> Plan {
        plan_move(i, target, &std_wf(), "me", now())
    }

    #[test]
    fn live_claim_is_refused() {
        let i = issue(
            "in_progress",
            json!({"assignee": "bot", "lease_until": lease(10)}),
        );
        assert_eq!(claim_state(&i, now()), ClaimState::Live);
        let r = reason(mv(&i, "ready"));
        assert!(r.starts_with("Claimed by bot until "), "{r}");
        assert!(r.ends_with(". A claimed Issue can't be moved."), "{r}");
    }

    #[test]
    fn stale_claim_is_refused_with_takeover_hint() {
        let i = issue(
            "in_progress",
            json!({"assignee": "bot", "lease_until": lease(-12)}),
        );
        assert_eq!(claim_state(&i, now()), ClaimState::Stale);
        assert_eq!(
            reason(mv(&i, "ready")),
            "bot's lease expired 12 min ago. Take over or release it first."
        );
    }

    #[test]
    fn ready_to_in_progress_assigns_viewer_without_force() {
        let a = actions(mv(&issue("ready", json!({})), "in_progress"));
        assert_eq!(a.len(), 1);
        assert_eq!(
            a[0].args,
            [
                "assign",
                "ORB-1",
                "--to",
                "me",
                "--kind",
                "human",
                "--if-version",
                "V"
            ]
        );
        assert!(a[0].confirm.is_none());
    }

    #[test]
    fn in_progress_over_lease_free_assignee_forces_and_confirms() {
        let i = issue("ready", json!({"assignee": "maria"}));
        let a = actions(mv(&i, "in_progress"));
        assert!(a[0].args.contains(&"--force".to_string()));
        assert_eq!(
            a[0].confirm.as_ref().unwrap().title,
            "Replace maria as Assignee?"
        );
    }

    #[test]
    fn drop_on_done_runs_orbit_done() {
        let a = actions(mv(&issue("in_progress", json!({})), "done"));
        assert_eq!(a.len(), 1);
        assert_eq!(
            a[0].args,
            ["done", "ORB-1", "--by", "me", "--if-version", "V"]
        );
    }

    #[test]
    fn drop_on_done_refused_while_blocked() {
        let i = issue("in_progress", json!({"blocking": ["ORB-9"]}));
        let r = reason(mv(&i, "done"));
        assert!(r.contains("ORB-9"), "{r}");
    }

    #[test]
    fn drop_on_done_refused_with_open_sub_issues() {
        let i = issue("in_progress", json!({"open_sub_issue_count": 2}));
        assert_eq!(reason(mv(&i, "done")), "2 sub-issues are still open.");
        let i = issue("ready", json!({"open_sub_issue_count": 1}));
        assert_eq!(reason(mv(&i, "cancelled")), "1 sub-issue is still open.");
    }

    #[test]
    fn in_review_to_done_is_refused() {
        assert_eq!(
            reason(mv(&issue("in_review", json!({})), "done")),
            "Approving needs a review verdict. Use orbit review."
        );
    }

    #[test]
    fn in_progress_to_in_review_needs_input() {
        let r = reason(mv(&issue("in_progress", json!({})), "in_review"));
        assert!(r.contains("orbit submit"), "{r}");
    }

    #[test]
    fn todo_to_ready_runs_promote_transition() {
        let a = actions(mv(&issue("todo", json!({})), "ready"));
        assert_eq!(
            a[0].args,
            [
                "transition",
                "ORB-1",
                "promote",
                "--by",
                "me",
                "--if-version",
                "V"
            ]
        );
    }

    #[test]
    fn done_to_ready_runs_reopen() {
        let a = actions(mv(&issue("done", json!({})), "ready"));
        assert_eq!(a[0].args, ["reopen", "ORB-1", "--if-version", "V"]);
    }

    #[test]
    fn unknown_route_is_refused() {
        assert_eq!(
            reason(mv(&issue("todo", json!({})), "in_review")),
            "No transition goes from Todo to In review."
        );
        assert_eq!(
            reason(mv(&issue("todo", json!({})), "todo")),
            "Already in this column."
        );
    }

    #[test]
    fn stale_actions_offer_takeover_and_release_with_force() {
        let i = issue(
            "in_progress",
            json!({"assignee": "bot", "lease_until": lease(-5)}),
        );
        let a = stale_actions(&i, &std_wf(), "me", now());
        let labels: Vec<_> = a.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["Take over", "Release"]);
        for x in &a {
            assert!(x.args.contains(&"--force".to_string()));
            assert!(x.args.contains(&"--if-version".to_string()));
            assert!(x.confirm.is_some());
        }
        assert_eq!(
            a[0].args,
            [
                "assign",
                "ORB-1",
                "--to",
                "me",
                "--kind",
                "human",
                "--force",
                "--if-version",
                "V"
            ]
        );
        assert_eq!(
            a[1].args,
            [
                "release",
                "ORB-1",
                "--by",
                "me",
                "--force",
                "--if-version",
                "V"
            ]
        );
    }

    #[test]
    fn stale_actions_empty_when_live() {
        let i = issue(
            "in_progress",
            json!({"assignee": "bot", "lease_until": lease(5)}),
        );
        assert!(stale_actions(&i, &std_wf(), "me", now()).is_empty());
    }

    #[test]
    fn annotate_never_exposes_argv() {
        for i in [
            issue("ready", json!({"assignee": "maria"})),
            issue(
                "in_progress",
                json!({"assignee": "bot", "lease_until": lease(-5)}),
            ),
        ] {
            let out = annotate(&i, &std_wf(), "me", now());
            assert!(!out.to_string().contains("\"args\""), "{out}");
            assert!(out["moves"].get("done").is_some());
            assert!(out["menu"].get("cancelled").is_some());
        }
        let out = annotate(&issue("ready", json!({})), &std_wf(), "me", now());
        assert_eq!(out["column"], "ready");
        assert_eq!(out["claim"], "none");
        assert_eq!(out["moves"]["in_progress"]["ok"], true);
        assert_eq!(out["moves"]["ready"]["ok"], false);
    }

    #[test]
    fn columns_follow_fixed_order_on_standard() {
        let cols = columns(&std_wf());
        let ids: Vec<_> = cols.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "blocked",
                "todo",
                "ready",
                "in_progress",
                "in_review",
                "done"
            ]
        );
        assert!(cols[0].statuses.is_empty());
        assert_eq!(cols[1].home.as_deref(), Some("todo"));
        assert_eq!(cols[5].statuses, ["done", "cancelled"]);
    }

    #[test]
    fn columns_on_agentic_put_needs_decision_in_todo() {
        let cols = columns(&agentic_wf());
        let todo = &cols.iter().find(|c| c.id == "todo").unwrap().statuses;
        assert!(todo.contains(&"needs_decision".to_string()));
        assert!(todo.contains(&"needs_human".to_string()));
        assert!(!todo.contains(&"blocked".to_string()));
        let i = issue("blocked", json!({}));
        assert_eq!(column_of(&i, &agentic_wf()), "blocked");
        assert_eq!(
            reason(plan_move(&i, "in_progress", &agentic_wf(), "me", now())),
            "In a blocked Status. Unblock it from the CLI."
        );
    }

    #[test]
    fn ready_issue_with_open_blocker_sits_in_blocked() {
        let i = issue("ready", json!({"blocking": ["ORB-9"]}));
        assert_eq!(column_of(&i, &std_wf()), "blocked");
        assert_eq!(column_of(&issue("ready", json!({})), &std_wf()), "ready");
        assert_eq!(
            column_of(&issue("needs_human", json!({})), &std_wf()),
            "todo"
        );
        assert_eq!(column_of(&issue("cancelled", json!({})), &std_wf()), "done");
    }

    #[test]
    fn in_progress_issue_with_blocker_stays_in_progress() {
        let i = issue("in_progress", json!({"blocking": ["ORB-9"]}));
        assert_eq!(column_of(&i, &std_wf()), "in_progress");
    }

    #[test]
    fn blocked_card_can_only_be_cancelled() {
        let i = issue("todo", json!({"blocking": ["ORB-9", "ORB-10"]}));
        let r = reason(plan_column(&i, "ready", &std_wf(), "me", now()));
        assert_eq!(r, "Blocked by ORB-9, ORB-10. It moves on when they close.");
        let a = actions(mv(&i, "cancelled"));
        assert_eq!(a[0].transition, "cancel");
        assert_eq!(
            reason(plan_column(&i, "blocked", &std_wf(), "me", now())),
            "Already in this column."
        );
        let r = reason(plan_column(
            &issue("ready", json!({})),
            "blocked",
            &std_wf(),
            "me",
            now(),
        ));
        assert!(r.starts_with("Nothing is dropped onto Blocked."), "{r}");
    }

    #[test]
    fn drop_on_done_column_never_cancels() {
        let a = actions(plan_column(
            &issue("ready", json!({})),
            "done",
            &std_wf(),
            "me",
            now(),
        ));
        let names: Vec<_> = a.iter().map(|a| a.transition.as_str()).collect();
        assert_eq!(names, ["done"]);
    }

    #[test]
    fn plan_column_prefers_a_meaningful_refusal() {
        // in_progress -> todo column: no route to `todo`, `needs_human` needs input.
        let r = reason(plan_column(
            &issue("in_progress", json!({})),
            "todo",
            &std_wf(),
            "me",
            now(),
        ));
        assert_eq!(r, "Asking needs a question. Use orbit ask.");
    }

    #[test]
    fn needs_human_reasons_in_priority_order() {
        let w = std_wf();
        let ask = json!({"orbit": {
            "ask": {"question": "Which DB?", "by": "bot", "at": "x", "return_to": "in_progress"},
            "escalation": {"reason": "attempt-limit", "by": "rev"},
        }});
        let i = issue("needs_human", json!({"properties": ask}));
        assert_eq!(
            needs_human(&i, &w, now()).as_deref(),
            Some("Answer: Which DB?")
        );

        let mut answered = ask.clone();
        answered["orbit"]["ask"]["answered_at"] = json!("y");
        let i = issue("needs_human", json!({"properties": answered}));
        assert_eq!(
            needs_human(&i, &w, now()).as_deref(),
            Some("Decide: escalated by rev")
        );
        let i = issue("needs_human", json!({}));
        assert_eq!(
            needs_human(&i, &w, now()).as_deref(),
            Some("Decide: escalated")
        );

        let i = issue(
            "in_progress",
            json!({"assignee": "bot", "lease_until": lease(-1), "labels": ["needs-triage"]}),
        );
        assert_eq!(
            needs_human(&i, &w, now()).as_deref(),
            Some("Take over or release")
        );
        let i = issue("ready", json!({"labels": ["x", "needs-triage"]}));
        assert_eq!(
            needs_human(&i, &w, now()).as_deref(),
            Some("Triage this Issue")
        );
        let i = issue("ready", json!({"labels": ["needs-info"]}));
        assert_eq!(
            needs_human(&i, &w, now()).as_deref(),
            Some("Add the missing information")
        );
        let i = issue("done", json!({"labels": ["needs-info"]}));
        assert_eq!(needs_human(&i, &w, now()), None);
    }

    #[test]
    fn in_review_is_not_needs_human() {
        assert_eq!(
            needs_human(&issue("in_review", json!({})), &std_wf(), now()),
            None
        );
    }

    #[test]
    fn waiting_on_link_is_not_needs_human() {
        let i = issue("ready", json!({"blocking": ["ORB-9"]}));
        assert_eq!(needs_human(&i, &std_wf(), now()), None);
    }

    #[test]
    fn ready_for_human_requires_label_ready_status_and_no_blocker() {
        let w = std_wf();
        let lbl = json!(["ready-for-human"]);
        assert!(ready_for_human(&issue("ready", json!({"labels": lbl})), &w));
        assert!(!ready_for_human(&issue("ready", json!({})), &w));
        assert!(!ready_for_human(&issue("todo", json!({"labels": lbl})), &w));
        assert!(!ready_for_human(
            &issue("ready", json!({"labels": lbl, "blocking": ["ORB-9"]})),
            &w
        ));
        assert!(!ready_for_human(
            &issue("ready", json!({"labels": lbl, "assignee": "maria"})),
            &w
        ));
    }

    #[test]
    fn move_into_active_off_the_claim_route_runs_the_transition() {
        let w = agentic_wf();
        let i = issue("needs_decision", json!({"assignee": "maria"}));
        let a = actions(plan_move(&i, "in_progress", &w, "me", now()));
        assert_eq!(
            a[0].args,
            [
                "transition",
                "ORB-1",
                "resume",
                "--by",
                "me",
                "--if-version",
                "V"
            ]
        );
        assert!(a[0].confirm.is_none());
        let a = actions(plan_move(
            &issue("ready", json!({})),
            "in_progress",
            &w,
            "me",
            now(),
        ));
        assert_eq!(a[0].args[0], "assign");
    }

    #[test]
    fn null_key_falls_back_to_id() {
        let i = issue("in_progress", json!({"key": null}));
        let a = actions(mv(&i, "done"));
        assert_eq!(
            a[0].args,
            ["done", "orb-a1b2c3", "--by", "me", "--if-version", "V"]
        );
        let i = issue(
            "in_progress",
            json!({"key": "", "assignee": "bot", "lease_until": lease(-5)}),
        );
        let a = stale_actions(&i, &std_wf(), "me", now());
        assert_eq!(a[0].args[1], "orb-a1b2c3");
        assert_eq!(
            a[0].confirm.as_ref().unwrap().title,
            "Take over orb-a1b2c3?"
        );
    }

    #[test]
    fn open_ask_refuses_every_move() {
        let props = json!({"orbit": {"ask": {"question": "Which DB?"}}});
        let i = issue("needs_human", json!({"properties": props}));
        let want = "Answer the open Ask first. Use orbit answer.";
        assert_eq!(reason(mv(&i, "done")), want);
        assert_eq!(reason(mv(&i, "cancelled")), want);
        assert_eq!(
            reason(plan_column(&i, "ready", &std_wf(), "me", now())),
            want
        );
        let mut answered = props.clone();
        answered["orbit"]["ask"]["answered_at"] = json!("y");
        let i = issue("needs_human", json!({"properties": answered}));
        assert_eq!(actions(mv(&i, "done"))[0].transition, "done");
    }
}
