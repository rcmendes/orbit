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
    let mut child = command.spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let url = line
        .trim()
        .strip_prefix("orbit board view: ")
        .unwrap_or_else(|| panic!("unexpected startup line: {line:?}"));
    let (base, token) = url.split_once("/#token=").unwrap();
    Server {
        child,
        base: base.to_string(),
        token: token.to_string(),
    }
}

fn http(
    base: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
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
    request.push_str("Connection: close\r\n\r\n");
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
    assert_eq!(http(&server.base, "GET", "/nope", &[]).0, 404);
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
