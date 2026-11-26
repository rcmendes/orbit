pub mod guard;
pub mod http;
pub mod plan;
pub mod window;
mod write;

use std::collections::HashMap;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::cli::BoardViewArgs;
use crate::error::{OrbitError, Result};
use crate::mcp::run_orbit;
use guard::{Denied, Guard, new_token};
use http::{Request, Response, read_request, write_response};

const READ_TIMEOUT: Duration = Duration::from_secs(5);

struct Ctx {
    exe: PathBuf,
    dir: Option<PathBuf>,
    guard: Guard,
    viewer: String,
    workflow: Value,
    closed_window: String,
}

pub fn serve(dir: Option<&Path>, args: &BoardViewArgs, json_mode: bool) -> Result<()> {
    let viewer = crate::commands::resolve_agent(args.by.clone())?;
    let exe = std::env::current_exe()
        .map_err(|error| OrbitError::io("resolve the orbit executable", error))?;
    let dir = dir.map(Path::to_path_buf);
    let workflow = cli_data(&exe, dir.as_deref(), &["capabilities"])
        .map_err(|_| OrbitError::config("could not read the Board workflow"))?["workflow"]
        .clone();

    // Read once at start; a changed team default applies on the next run.
    let closed_window = window::effective(
        crate::board::Board::discover(dir.as_deref())?
            .config
            .board_view
            .closed_window
            .as_deref(),
    );

    let listener = TcpListener::bind(("127.0.0.1", args.port))
        .map_err(|error| OrbitError::io("bind the Board view port", error))?;
    let port = listener
        .local_addr()
        .map_err(|error| OrbitError::io("read the Board view port", error))?
        .port();
    let token = new_token();
    let url = format!("http://127.0.0.1:{port}/#token={token}");
    if json_mode {
        println!("{}", json!({"ok": true, "data": {"url": url}}));
    } else {
        println!("orbit board view: {url}");
    }
    let _ = std::io::stdout().flush();
    if !args.no_open {
        open_browser(&url);
    }

    let ctx = Ctx {
        exe,
        dir,
        guard: Guard { token, port },
        viewer,
        workflow,
        closed_window,
    };
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle(&ctx, stream) {
                    eprintln!("orbit board view: {error}");
                }
            }
            Err(error) => eprintln!("orbit board view: accept: {error}"),
        }
    }
    Ok(())
}

fn handle(ctx: &Ctx, mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(READ_TIMEOUT))?;
    let response = match read_request(&mut stream) {
        Ok(request) => route(ctx, &request),
        Err(error) => Response::text(error.status(), "bad request"),
    };
    write_response(&mut stream, &response)
}

fn denied(d: Denied) -> Response {
    let status = if d == Denied::BadToken { 401 } else { 403 };
    Response::text(status, "forbidden")
}

fn route(ctx: &Ctx, request: &Request) -> Response {
    let path = request.path.split('?').next().unwrap_or("");
    if let Some(api) = path.strip_prefix("/api/") {
        if let Err(d) = ctx.guard.check_api(request) {
            return denied(d);
        }
        return match (request.method.as_str(), api) {
            ("GET", "snapshot") => snapshot(ctx, request),
            ("POST", "move" | "takeover" | "release") => write::write(ctx, api, request),
            ("GET", _) => match api.strip_prefix("show/") {
                Some(id) if !id.is_empty() => show(ctx, id),
                _ => Response::text(404, "not found"),
            },
            _ => Response::text(404, "not found"),
        };
    }
    if request.method != "GET" {
        return Response::text(404, "not found");
    }
    let asset = |content_type, body| match ctx.guard.check_host(request) {
        Ok(()) => Response::asset(content_type, body),
        Err(d) => denied(d),
    };
    match path {
        "/" => asset(
            "text/html; charset=utf-8",
            include_str!("assets/index.html"),
        ),
        "/app.js" => asset(
            "text/javascript; charset=utf-8",
            include_str!("assets/app.js"),
        ),
        "/app.css" => asset("text/css; charset=utf-8", include_str!("assets/app.css")),
        "/favicon.svg" => asset("image/svg+xml", include_str!("assets/favicon.svg")),
        _ => Response::text(404, "not found"),
    }
}

fn snapshot(ctx: &Ctx, request: &Request) -> Response {
    let data = match cli_data(
        &ctx.exe,
        ctx.dir.as_deref(),
        &["list", "--all", "--include-closed"],
    ) {
        Ok(data) => data,
        Err(response) => return response,
    };
    let mut issues = data["issues"].as_array().cloned().unwrap_or_default();
    // `blocking` holds Issue ids; show keys in refusal reasons.
    let keys: HashMap<String, String> = issues
        .iter()
        .filter_map(|i| {
            Some((
                i["id"].as_str()?.to_string(),
                i["key"].as_str()?.to_string(),
            ))
        })
        .collect();
    let now = chrono::Utc::now();
    for issue in &mut issues {
        if let Some(blocking) = issue["blocking"].as_array_mut() {
            for entry in blocking {
                if let Some(key) = entry.as_str().and_then(|id| keys.get(id)) {
                    *entry = json!(key);
                }
            }
        }
        let extra = plan::annotate(issue, &ctx.workflow, &ctx.viewer, now);
        if let (Some(base), Some(extra)) = (issue.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
    }
    let mut response = Response::json(
        200,
        &json!({"viewer": ctx.viewer, "workflow": ctx.workflow, "closed_window": ctx.closed_window, "issues": issues}),
    );
    let etag = format!(
        "\"{}\"",
        Sha256::digest(&response.body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    if request.header("If-None-Match") == Some(etag.as_str()) {
        response = Response::text(304, "");
    }
    response.extra.push(("ETag", etag));
    response
}

fn show(ctx: &Ctx, id: &str) -> Response {
    if !write::valid_id(id) {
        return write::bad_request("Invalid id.");
    }
    match cli_data(&ctx.exe, ctx.dir.as_deref(), &["show", id]) {
        Ok(data) => Response::json(200, &data),
        Err(response) => response,
    }
}

/// Runs `orbit <args> --json` and returns the envelope's `data`, or the response to send instead.
fn cli_data(exe: &Path, dir: Option<&Path>, args: &[&str]) -> std::result::Result<Value, Response> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let run = run_orbit(exe, dir, &args, true)
        .map_err(|e| Response::json(502, &json!({"ok": false, "error": {"message": e}})))?;
    match serde_json::from_str::<Value>(&run.stdout) {
        Ok(env) if env["ok"] == true => Ok(env["data"].clone()),
        Ok(env) => {
            let status = if env["error"]["code"] == "VERSION_CONFLICT" {
                409
            } else {
                422
            };
            Err(Response::json(status, &env))
        }
        Err(_) => Err(Response::json(
            502,
            &json!({"ok": false, "error": {"message": run.stderr.trim()}}),
        )),
    }
}

fn open_browser(url: &str) {
    let mut command = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    if let Err(error) = command.spawn() {
        eprintln!("orbit board view: could not open a browser: {error}");
    }
}
