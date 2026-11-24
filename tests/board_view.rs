mod common;

use common::TestRepo;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

struct Server {
    child: Child,
    base: String,
    token: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(repo: &TestRepo) -> Server {
    let mut command = repo.orbit_command(&[
        "board",
        "view",
        "--no-open",
        "--port",
        "0",
        "--by",
        "tester",
    ]);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = command.spawn().unwrap();
    // Guard first: a panic below must not leak the server.
    let mut guard = Server {
        child,
        base: String::new(),
        token: String::new(),
    };
    let stdout = guard.child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| panic!("orbit board view printed no startup line within 30 s"));
    let url = line
        .trim()
        .strip_prefix("orbit board view: ")
        .unwrap_or_else(|| panic!("unexpected startup line: {line:?}"));
    let (base, token) = url.split_once("/#token=").unwrap();
    guard.base = base.to_string();
    guard.token = token.to_string();
    guard
}

fn http(
    base: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> (u16, Vec<(String, String)>, String) {
    http_body(base, method, path, headers, "")
}

fn http_body(
    base: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, Vec<(String, String)>, String) {
    let addr = base.trim_start_matches("http://");
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut request = format!("{method} {path} HTTP/1.1\r\n");
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        request.push_str(&format!("Host: {addr}\r\n"));
    }
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    (status, headers, body.to_string())
}

fn api(
    server: &Server,
    path: &str,
    extra: &[(&str, &str)],
) -> (u16, Vec<(String, String)>, String) {
    let mut headers = vec![("X-Orbit-Token", server.token.as_str())];
    headers.extend_from_slice(extra);
    http(&server.base, "GET", path, &headers)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn repo_with_issue() -> TestRepo {
    let repo = TestRepo::new();
    repo.init();
    assert_eq!(repo.json(&["add", "first", "--json"]).0, 0);
    repo
}

#[test]
fn capabilities_lists_board_view() {
    let repo = repo_with_issue();
    let (_, value) = repo.json(&["capabilities", "--json"]);
    assert!(
        value["data"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "board-view")
    );
}

#[test]
fn serves_page_and_assets_with_csp() {
    let repo = repo_with_issue();
    let server = start(&repo);
    let (status, headers, body) = http(&server.base, "GET", "/", &[]);
    assert_eq!(status, 200);
    assert!(
        header(&headers, "content-security-policy")
            .unwrap()
            .starts_with("default-src 'self'")
    );
    assert!(!body.contains("style="));
    assert!(
        body.split("<script")
            .skip(1)
            .all(|t| t.split('>').next().unwrap().contains("src=")),
        "inline script: {body}"
    );
    assert_eq!(http(&server.base, "GET", "/app.js", &[]).0, 200);
    assert_eq!(http(&server.base, "GET", "/app.css", &[]).0, 200);
    assert_eq!(http(&server.base, "GET", "/favicon.svg", &[]).0, 200);
    assert_eq!(http(&server.base, "GET", "/nope", &[]).0, 404);
}

#[test]
fn assets_have_no_inline_code_and_no_external_urls() {
    let repo = repo_with_issue();
    let server = start(&repo);
    let (status, _, page) = http(&server.base, "GET", "/", &[]);
    assert_eq!(status, 200);
    let (js_status, _, js) = http(&server.base, "GET", "/app.js", &[]);
    let (css_status, _, css) = http(&server.base, "GET", "/app.css", &[]);
    assert_eq!((js_status, css_status), (200, 200));
    assert!(page.contains(r#"src="/app.js""#), "{page}");
    for banned in ["<style", " style=", "innerHTML", "https://", "http://"] {
        assert!(!page.contains(banned), "page contains {banned}");
    }
    // An `on[a-z]+=` attribute inside any tag.
    let inline_handler = page.split('<').skip(1).any(|tag| {
        let tag = tag.split('>').next().unwrap_or("");
        tag.split_whitespace().skip(1).any(|attr| {
            attr.strip_prefix("on").is_some_and(|rest| {
                let name = rest.split('=').next().unwrap_or("");
                rest.contains('=')
                    && !name.is_empty()
                    && name.bytes().all(|b| b.is_ascii_lowercase())
            })
        })
    });
    assert!(!inline_handler, "inline event handler: {page}");
    assert!(
        !js.contains("innerHTML") && !js.contains("eval("),
        "unsafe js"
    );
    assert!(js.contains("X-Orbit-Token"), "page never sends the token");
    assert!(
        js.contains("sessionStorage"),
        "token does not survive a reload"
    );
    assert!(
        js.lines()
            .filter(|l| l.contains("localStorage"))
            .all(|l| !l.to_ascii_lowercase().contains("token")),
        "token in localStorage"
    );
    for asset in [&js, &css] {
        assert!(!asset.contains("https://") && !asset.contains("http://"));
    }
}

#[test]
fn snapshot_requires_token() {
    let repo = repo_with_issue();
    let server = start(&repo);
    assert_eq!(http(&server.base, "GET", "/api/snapshot", &[]).0, 401);
    let (status, _, body) = api(&server, "/api/snapshot", &[]);
    assert_eq!(status, 200);
    let value: Value = serde_json::from_str(&body).unwrap();
    assert!(value["issues"][0].get("moves").is_some(), "{body}");
    assert_eq!(value["viewer"], "tester");
}

#[test]
fn snapshot_rejects_rebinding_host() {
    let repo = repo_with_issue();
    let server = start(&repo);
    let port = server.base.rsplit(':').next().unwrap();
    let host = format!("evil.example:{port}");
    assert_eq!(api(&server, "/api/snapshot", &[("Host", &host)]).0, 403);
}

#[test]
fn snapshot_etag_round_trip() {
    let repo = repo_with_issue();
    let server = start(&repo);
    let (_, headers, _) = api(&server, "/api/snapshot", &[]);
    let etag = header(&headers, "etag").unwrap().to_string();
    let (status, _, body) = api(&server, "/api/snapshot", &[("If-None-Match", &etag)]);
    assert_eq!(status, 304);
    assert!(body.is_empty());
}

#[test]
fn show_returns_issue() {
    let repo = repo_with_issue();
    let server = start(&repo);
    let (_, _, body) = api(&server, "/api/snapshot", &[]);
    let id = serde_json::from_str::<Value>(&body).unwrap()["issues"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _, body) = api(&server, &format!("/api/show/{id}"), &[]);
    assert_eq!(status, 200, "{body}");
    assert_eq!(api(&server, "/api/show/nope-nope", &[]).0, 422);
}

#[test]
fn silent_client_does_not_block_server() {
    let repo = repo_with_issue();
    let server = start(&repo);
    let _silent = TcpStream::connect(server.base.trim_start_matches("http://")).unwrap();
    let started = Instant::now();
    assert_eq!(api(&server, "/api/snapshot", &[]).0, 200);
    assert!(started.elapsed() < Duration::from_secs(7));
}

/// POSTs `body` as JSON with the token and, when `origin`, a same-origin `Origin`.
fn post(server: &Server, path: &str, body: &Value, origin: bool) -> (u16, Value) {
    let origin_value = server.base.clone();
    let mut headers = vec![
        ("X-Orbit-Token", server.token.as_str()),
        ("Content-Type", "application/json"),
    ];
    if origin {
        headers.push(("Origin", origin_value.as_str()));
    }
    let (status, _, body) = http_body(&server.base, "POST", path, &headers, &body.to_string());
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}

fn shown(repo: &TestRepo, id: &str) -> Value {
    repo.ok_json(&["show", id, "--json"])["data"].clone()
}

fn version(repo: &TestRepo, id: &str) -> String {
    shown(repo, id)["updated_at"].as_str().unwrap().to_string()
}

#[test]
fn move_ready_to_in_progress_assigns_viewer() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("work", &[]);
    let server = start(&repo);
    let v = version(&repo, &id);
    let (status, body) = post(
        &server,
        "/api/move",
        &serde_json::json!({"id": id, "status": "in_progress", "version": v}),
        true,
    );
    assert_eq!(status, 200, "{body}");
    let issue = shown(&repo, &id);
    assert_eq!(issue["assignee"], "tester");
    assert_eq!(issue["assignee_kind"], "human");
    assert!(issue["lease_until"].is_null(), "{issue}");
}

#[test]
fn move_by_column_and_bad_shapes() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("work", &[]);
    let server = start(&repo);
    let v = version(&repo, &id);
    for bad in [
        serde_json::json!({"id": id, "version": v}),
        serde_json::json!({"id": id, "version": v, "status": "in_progress", "column": "in_progress"}),
        serde_json::json!({"id": id, "version": v, "status": "nope"}),
        serde_json::json!({"id": id, "version": v, "column": "nope"}),
        serde_json::json!({"id": id, "version": v, "column": "in_progress", "transition": "nope"}),
        serde_json::json!({"id": id, "version": v, "column": "in_progress", "transition": "done"}),
        serde_json::json!({"id": id, "column": "in_progress"}),
    ] {
        assert_eq!(post(&server, "/api/move", &bad, true).0, 400, "{bad}");
    }
    let (status, body) = post(
        &server,
        "/api/move",
        &serde_json::json!({"id": id, "column": "in_progress", "version": v}),
        true,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(shown(&repo, &id)["status"], "in_progress");
}

#[test]
fn move_refuses_live_claim() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("work", &[]);
    repo.ok_json(&["claim", &id, "--by", "agent-1", "--json"]);
    let before = shown(&repo, &id);
    let server = start(&repo);
    let (status, body) = post(
        &server,
        "/api/move",
        &serde_json::json!({"id": id, "status": "ready", "version": before["updated_at"]}),
        true,
    );
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "MOVE_REFUSED");
    assert_eq!(shown(&repo, &id), before);
}

#[test]
fn takeover_requires_confirmation_then_succeeds() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("work", &[]);
    repo.ok_json(&["claim", &id, "--by", "agent-1", "--json"]);
    repo.make_stale(&id);
    let server = start(&repo);
    let v = version(&repo, &id);
    let (status, body) = post(
        &server,
        "/api/takeover",
        &serde_json::json!({"id": id, "version": v}),
        true,
    );
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "CONFIRM_REQUIRED");
    assert!(body["error"]["confirm"]["title"].is_string(), "{body}");
    let (status, body) = post(
        &server,
        "/api/takeover",
        &serde_json::json!({"id": id, "version": v, "confirmed": true}),
        true,
    );
    assert_eq!(status, 200, "{body}");
    let issue = shown(&repo, &id);
    assert_eq!(issue["assignee"], "tester");
    assert_eq!(issue["status"], "in_progress");
}

#[test]
fn release_stale_claim_returns_issue_to_ready() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("work", &[]);
    repo.ok_json(&["claim", &id, "--by", "agent-1", "--json"]);
    repo.make_stale(&id);
    let server = start(&repo);
    let v = version(&repo, &id);
    let (status, body) = post(
        &server,
        "/api/release",
        &serde_json::json!({"id": id, "version": v, "confirmed": true}),
        true,
    );
    assert_eq!(status, 200, "{body}");
    let issue = shown(&repo, &id);
    assert_eq!(issue["status"], "ready");
    assert!(issue["assignee"].is_null(), "{issue}");
}

#[test]
fn live_claim_refuses_takeover_and_release_even_confirmed() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("work", &[]);
    repo.ok_json(&["claim", &id, "--by", "agent-1", "--json"]);
    let server = start(&repo);
    let before = shown(&repo, &id);
    let v = version(&repo, &id);
    for path in ["/api/takeover", "/api/release"] {
        let (status, body) = post(
            &server,
            path,
            &serde_json::json!({"id": id, "version": v, "confirmed": true}),
            true,
        );
        assert_eq!(status, 409, "{path} {body}");
        assert_eq!(body["error"]["code"], "MOVE_REFUSED", "{path} {body}");
    }
    assert_eq!(shown(&repo, &id), before);
}

#[test]
fn takeover_loses_to_returning_agent() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("work", &[]);
    repo.ok_json(&["claim", &id, "--by", "agent-1", "--json"]);
    repo.make_stale(&id);
    let v = version(&repo, &id);
    repo.ok_json(&["claim", &id, "--by", "agent-1", "--json"]);
    assert_ne!(version(&repo, &id), v);
    let server = start(&repo);
    let (status, body) = post(
        &server,
        "/api/takeover",
        &serde_json::json!({"id": id, "version": v, "confirmed": true}),
        true,
    );
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "VERSION_CONFLICT");
    assert_eq!(shown(&repo, &id)["assignee"], "agent-1");
}

#[test]
fn move_rejects_option_like_id() {
    let repo = repo_with_issue();
    let before = repo.ok_json(&["list", "--all", "--json"])["data"].clone();
    let server = start(&repo);
    let (status, _) = post(
        &server,
        "/api/move",
        &serde_json::json!({"id": "--force", "status": "in_progress", "version": "x"}),
        true,
    );
    assert_eq!(status, 400);
    for path in ["/api/takeover", "/api/release"] {
        let (status, _) = post(
            &server,
            path,
            &serde_json::json!({"id": "-h", "version": "x", "confirmed": true}),
            true,
        );
        assert_eq!(status, 400, "{path}");
    }
    assert_eq!(repo.ok_json(&["list", "--all", "--json"])["data"], before);
}

#[test]
fn show_rejects_option_like_id() {
    let repo = repo_with_issue();
    let server = start(&repo);
    assert_eq!(api(&server, "/api/show/--help", &[]).0, 400);
    assert_eq!(api(&server, "/api/show/-h", &[]).0, 400);
}

#[test]
fn post_without_origin_is_forbidden() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("work", &[]);
    let server = start(&repo);
    let v = version(&repo, &id);
    let (status, _) = post(
        &server,
        "/api/move",
        &serde_json::json!({"id": id, "status": "in_progress", "version": v}),
        false,
    );
    assert_eq!(status, 403);
    assert_eq!(shown(&repo, &id)["status"], "ready");
}

#[test]
fn in_review_to_done_is_refused() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("work", &[]);
    repo.ok_json(&["claim", &id, "--by", "agent-1", "--json"]);
    repo.ok_json(&["submit", &id, "--by", "agent-1", "--note", "done", "--json"]);
    let before = shown(&repo, &id);
    assert_eq!(before["status"], "in_review");
    let server = start(&repo);
    let (status, body) = post(
        &server,
        "/api/move",
        &serde_json::json!({"id": id, "status": "done", "version": before["updated_at"]}),
        true,
    );
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "MOVE_REFUSED");
    assert!(
        body["error"]["detail"]
            .as_str()
            .unwrap()
            .contains("orbit review"),
        "{body}"
    );
    assert_eq!(shown(&repo, &id), before);
}
