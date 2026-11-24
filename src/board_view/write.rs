//! Write routes (ADR 0023): `POST /api/move`, `/api/takeover`, `/api/release`.
//! Each re-plans on a fresh `orbit show` and runs the chosen action's argv.

use serde_json::{Value, json};

use super::http::{Request, Response};
use super::plan::{self, Plan};
use super::{Ctx, cli_data};

/// `^[A-Za-z0-9][A-Za-z0-9._-]{0,80}$`: an id can never read as an option.
pub(super) fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 81
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

pub(super) fn bad_request(message: &str) -> Response {
    Response::json(
        400,
        &json!({"ok": false, "error": {"code": "BAD_REQUEST", "message": message}}),
    )
}

fn conflict(code: &str, extra: Value) -> Response {
    let mut error = json!({ "code": code });
    if let (Some(e), Some(x)) = (error.as_object_mut(), extra.as_object()) {
        e.extend(x.clone());
    }
    Response::json(409, &json!({"ok": false, "error": error}))
}

/// `route` is `move`, `takeover`, or `release`.
pub(super) fn write(ctx: &Ctx, route: &str, request: &Request) -> Response {
    let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
        return bad_request("The body must be JSON.");
    };
    let field = |k: &str| body.get(k).and_then(Value::as_str);
    let Some(id) = field("id").filter(|id| valid_id(id)) else {
        return bad_request("Invalid id.");
    };
    let Some(version) = field("version") else {
        return bad_request("Missing version.");
    };
    let wf = &ctx.workflow;
    let named = |key: &str, name: &str| {
        wf[key]
            .as_array()
            .is_some_and(|a| a.iter().any(|d| d["name"] == name))
    };
    let transition = field("transition");
    if transition.is_some_and(|t| !named("transitions", t)) {
        return bad_request("Unknown transition.");
    }
    let (column, status) = (field("column"), field("status"));
    if route == "move" {
        let ok = match (column, status) {
            (Some(c), None) => plan::columns(wf).iter().any(|col| col.id == c),
            (None, Some(s)) => named("statuses", s),
            _ => false,
        };
        if !ok {
            return bad_request("Send exactly one known column or status.");
        }
    }

    let issue = match cli_data(&ctx.exe, ctx.dir.as_deref(), &["show", id]) {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    // The card changed since the viewer saw it (e.g. its agent came back):
    // the plan the viewer confirmed no longer holds, so fail like the CLI would.
    let current = issue["updated_at"].as_str().unwrap_or_default();
    // An Issue without a version cannot be checked, and `--if-version ""` is no guard.
    if current.is_empty() || current != version {
        let detail =
            format!("{id} changed since version {version}; its current version is {current}");
        return conflict(
            "VERSION_CONFLICT",
            json!({
                "message": detail,
                "detail": detail,
                "retryable": true,
                "details": {"issue": id, "expected_version": version, "actual_version": current},
            }),
        );
    }

    let now = chrono::Utc::now();
    let plan = match (route, column, status) {
        ("move", Some(c), _) => plan::plan_column(&issue, c, wf, &ctx.viewer, now),
        ("move", _, Some(s)) => plan::plan_move(&issue, s, wf, &ctx.viewer, now),
        _ => {
            let want = if route == "takeover" {
                "claim"
            } else {
                "release"
            };
            let actions: Vec<_> = plan::stale_actions(&issue, wf, &ctx.viewer, now)
                .into_iter()
                .filter(|a| a.transition == want)
                .collect();
            if actions.is_empty() {
                Plan::Refused {
                    reason: "Only a Stale claim can be taken over or released here.".into(),
                }
            } else {
                Plan::Run(actions)
            }
        }
    };
    let mut actions = match plan {
        Plan::Refused { reason } => return conflict("MOVE_REFUSED", json!({"detail": reason})),
        Plan::Run(actions) => actions,
    };
    let action = match transition {
        Some(t) => match actions.iter().position(|a| a.transition == t) {
            Some(i) => actions.swap_remove(i),
            None => return bad_request("That transition does not apply to this move."),
        },
        None if actions.len() > 1 => {
            let choices: Vec<Value> = actions
                .iter()
                .map(|a| json!({"transition": a.transition, "label": a.label}))
                .collect();
            return conflict("CHOOSE_TRANSITION", json!({"choices": choices}));
        }
        None => actions.swap_remove(0),
    };
    if let Some(c) = &action.confirm
        && body["confirmed"] != true
    {
        return conflict(
            "CONFIRM_REQUIRED",
            json!({"confirm": {"title": c.title, "body": c.body, "ok": c.ok}}),
        );
    }
    let mut args = action.args;
    // Race window between the check above and the write: the CLI catches it.
    if let Some(i) = args.iter().position(|a| a == "--if-version") {
        args[i + 1] = version.to_string();
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match cli_data(&ctx.exe, ctx.dir.as_deref(), &args) {
        Ok(data) => Response::json(200, &data),
        Err(response) => response,
    }
}
