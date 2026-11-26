# Board view Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `orbit board view` serves a kanban of the Board (Columns and Queue layouts) on `127.0.0.1` while the command runs, with drag-to-Transition and Stale-card Take over / Release.

**Architecture:** A new `src/board_view/` module holds a single-threaded HTTP/1.1 server on `std::net`. Like `orbit mcp`, every read and write spawns the `orbit` CLI with `--json` (ADR 0004). A pure planner decides which column each Issue sits in, whether it needs a human, and, per Issue and target column, which `orbit` command a move runs or why it is refused. The planner runs on the server twice: once to annotate the snapshot so the page knows valid drop targets, and again on a fresh `orbit show` before every write. The page is plain HTML, JS and CSS embedded with `include_str!`, ported from the prototype.

**Tech Stack:** Rust 2024 (MSRV 1.89), std only plus existing crates (`serde_json`, `sha2`, `rand`, `chrono`). No new dependency.

**Spec:** [docs/adr/0023-board-view-served-by-a-foreground-local-server.md](../adr/0023-board-view-served-by-a-foreground-local-server.md). Validated UI: `scripts/board-view-prototype/index.html` (throwaway; port from it, do not ship it).

## Global Constraints

- No new crate. No Tokio. No CDN, no web font, no build step. Fonts use the system stack.
- Bind `127.0.0.1` only. `--port` defaults to `0` (random free port).
- The token is 32 random bytes as hex, sent only in the URL fragment `#token=…` and the `X-Orbit-Token` header.
- Every response sends `Content-Security-Policy: default-src 'self'`, `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff` and `Connection: close`.
- Every API route checks the token, `Host` ∈ {`127.0.0.1:PORT`, `localhost:PORT`}, and `Origin` (when present; required on POST) ∈ {`http://127.0.0.1:PORT`, `http://localhost:PORT`}.
- Issue text is rendered with `textContent` only, never `innerHTML`.
- The server passes `--force` only for Take over, Release, and an active drop over a lease-free Assignee. Each needs a server-side precondition check, `confirmed: true` in the request, and the client's `--if-version`.
- Every write passes the card's `updated_at` as `--if-version`.
- Identity: `--by`, else `ORBIT_AGENT`, else `USER`, else `USERNAME`; the kind is always `human`.
- Snapshot: `orbit list --json --all --include-closed` with no limit (a cap would silently hide Issues on big Boards), polled every 3000 ms, with a `sha256` ETag.
- Closed columns show Issues closed within the Closed window (default 14 days; `[board_view] closed_window`, ADR 0023 amendment); the page's "Closed:" dropdown (24 hours to All, or a custom range) overrides it. The age is `resolved_at`, falling back to `updated_at` for Issues closed before it was recorded (ADR 0024). This replaces the original `updated_at` 14-day shortcut.
- Gates: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all`.
- Docs rule (AGENTS.md): every behavior change updates `docs/commands.md`, README, and the affected docs in the same task or an immediate follow-up commit.

## Review Focus

1. **Request smuggling and oversized input**: `Transfer-Encoding`, two `Content-Length` headers, headers over 8 KiB, or a body over 16 KiB must be rejected (400/413/501), never buffered or partially parsed. Tested in Task 1.
2. **DNS rebinding and cross-site POST**: a request with `Host: evil.example:PORT` or `Origin: http://evil.example` gets 403, even with a valid token. Tested in Task 2.
3. **An agent comes back mid-action**: a heartbeat between snapshot and Take over must make the write fail with `VERSION_CONFLICT`, and the agent keeps its claim. Tested in Task 5.
4. **Issue IDs from the request reach a spawned CLI**: an id like `--force` or `../x` must be rejected before spawning. Tested in Task 5.
5. **A slow or silent client**: a connection that sends nothing must time out (5 s read timeout) and not block the next request forever. Tested in Task 4.

---

### Task 1: HTTP request parser and response writer

**Files:**
- Create: `src/board_view/mod.rs` (module root; `pub mod http;` for now)
- Create: `src/board_view/http.rs`
- Modify: `src/lib.rs` (add `pub mod board_view;`)

**Interfaces:**
- Produces:
  - `pub struct Request { pub method: String, pub path: String, pub headers: Vec<(String, String)>, pub body: Vec<u8> }` with `pub fn header(&self, name: &str) -> Option<&str>` (case-insensitive).
  - `pub enum ParseError { BadRequest(&'static str), TooLarge, Unsupported }`, mapping to status 400, 413 and 501.
  - `pub fn read_request(stream: &mut impl std::io::Read) -> Result<Request, ParseError>`
  - `pub struct Response { pub status: u16, pub content_type: &'static str, pub body: Vec<u8>, pub extra: Vec<(&'static str, String)> }`, with constructors `Response::json(status, &serde_json::Value)`, `Response::text(status, &str)` and `Response::asset(content_type, &'static str)`.
  - `pub fn write_response(stream: &mut impl std::io::Write, r: &Response) -> std::io::Result<()>`. It always adds the four security headers from Global Constraints, plus `Content-Length`.
  - Constants: `MAX_HEAD: usize = 8 * 1024`, `MAX_BODY: usize = 16 * 1024`, `MAX_HEADERS: usize = 64`.

- [ ] **Step 1: Write failing unit tests** in `src/board_view/http.rs` `#[cfg(test)]`, reading from `std::io::Cursor`:
  - `parses_get_with_headers`: `"GET /api/snapshot HTTP/1.1\r\nHost: 127.0.0.1:9\r\nX-Orbit-Token: t\r\n\r\n"` → method `GET`, path `/api/snapshot`, `header("x-orbit-token") == Some("t")`.
  - `reads_body_by_content_length`: a POST with `Content-Length: 7` and body `{"a":1}` gives `body == b"{\"a\":1}"`.
  - `rejects_transfer_encoding`: `Transfer-Encoding: chunked` → `Unsupported`.
  - `rejects_duplicate_content_length` → `BadRequest`.
  - `rejects_head_over_limit`: 9000 bytes of headers → `TooLarge`.
  - `rejects_body_over_limit`: `Content-Length: 16385` → `TooLarge` (without reading the body).
  - `rejects_too_many_headers`: 65 headers → `TooLarge`.
  - `rejects_missing_http_version` → `BadRequest`.
  - `response_carries_security_headers`: `write_response` output contains `Content-Security-Policy: default-src 'self'`, `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`, `Connection: close` and the right `Content-Length`.
- [ ] **Step 2:** Run `cargo test --lib board_view::http`. Expected: FAIL (not defined).
- [ ] **Step 3:** Implement. Read byte by byte up to `\r\n\r\n` (cap `MAX_HEAD`), split lines, and accept only `HTTP/1.1` and `HTTP/1.0`. Strip any query string from `path` into nothing (the API takes none).
- [ ] **Step 4:** Run `cargo test --lib board_view::http`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(board-view): add a bounded HTTP/1.1 request parser`.

---

### Task 2: Local-only security gate and token

**Files:**
- Create: `src/board_view/guard.rs`
- Modify: `src/board_view/mod.rs` (`pub mod guard;`)

**Interfaces:**
- Consumes: `http::Request`.
- Produces:
  - `pub struct Guard { pub token: String, pub port: u16 }`
  - `pub fn new_token() -> String` (32 bytes from `rand`, lowercase hex, 64 chars).
  - `pub enum Denied { WrongHost, WrongOrigin, MissingOrigin, BadToken }`. All map to 403 except `BadToken`, which maps to 401.
  - `impl Guard { pub fn check_host(&self, r: &Request) -> Result<(), Denied>; pub fn check_api(&self, r: &Request) -> Result<(), Denied> }`. `check_api` does the host check, the origin check (required when `method == "POST"`), and a constant-time token compare.

- [ ] **Step 1: Write failing tests**:
  - `token_is_64_hex_chars_and_random`: two calls differ, both match `^[0-9a-f]{64}$`.
  - `accepts_localhost_and_loopback_hosts`: `Host: 127.0.0.1:4747` and `Host: localhost:4747` pass for `port: 4747`.
  - `rejects_rebinding_host`: `Host: evil.example:4747` → `WrongHost`.
  - `rejects_foreign_origin_even_with_token`: POST with a good token and `Origin: http://evil.example` → `WrongOrigin`.
  - `post_requires_origin`: POST with a good token and no `Origin` → `MissingOrigin`.
  - `get_without_origin_is_allowed`: GET with a good token and no Origin → Ok.
  - `rejects_wrong_or_missing_token` → `BadToken`.
- [ ] **Step 2:** Run `cargo test --lib board_view::guard`. Expected: FAIL.
- [ ] **Step 3:** Implement. The constant-time compare XOR-folds bytes after a length check.
- [ ] **Step 4:** Run `cargo test --lib board_view::guard`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(board-view): add token and host/origin guard`.

---

### Task 3: Move planner (pure)

The rules from ADR 0023, ported from the prototype's `planMove` / `actionFor` / `staleActions`, operating on the JSON Issue shape that `orbit list --json` and `orbit show --json` emit.

**Files:**
- Create: `src/board_view/plan.rs`
- Modify: `src/board_view/mod.rs` (`pub mod plan;`)

**Interfaces:**
- Consumes: the workflow as the JSON `orbit capabilities --json` returns under `data.workflow` (`statuses: [{name, category, ready}]`, `transitions: [{name, from: [..], to, resolution?}]`), passed as `wf: &Value`. The server only ever holds this JSON, so the planner does not touch `crate::workflow`.
- Produces:
  - `pub enum ClaimState { None, Live, Stale }` and `pub fn claim_state(issue: &Value, now: DateTime<Utc>) -> ClaimState` (assignee and `lease_until` both present; live when `lease_until > now`).
  - `pub struct Confirm { pub title: String, pub body: String, pub ok: String }`
  - `pub struct Action { pub label: String, pub transition: String, pub args: Vec<String>, pub confirm: Option<Confirm> }`. `args` is the full CLI argv after `orbit` (without `--json`).
  - `pub enum Plan { Refused { reason: String }, Run(Vec<Action>) }`. Several actions mean the viewer must pick one.
  - `pub fn plan_move(issue: &Value, target: &str, wf: &Value, viewer: &str, now: DateTime<Utc>) -> Plan`
  - `pub fn stale_actions(issue: &Value, wf: &Value, viewer: &str, now: DateTime<Utc>) -> Vec<Action>` (labels `"Take over"` and `"Release"`).
  - `pub struct Column { pub id: String, pub name: String, pub statuses: Vec<String>, pub home: Option<String> }` and `pub fn columns(wf: &Value) -> Vec<Column>`, in this fixed order:
    1. `blocked` ("Blocked", no statuses: nothing drops here)
    2. `todo` ("Todo": every `todo`-category Status except the Ready one and one named `blocked`; `home` = `todo` if present)
    3. `ready` (the Ready Status)
    4. one column per `active` Status, in Workflow order (`id` = Status name)
    5. `done` ("Done": the `done` and `canceled` Statuses)
  - `pub fn column_of(issue: &Value, wf: &Value) -> String` (a column id). A closed category → `done`. A `todo` category with a non-empty `blocking`, or the Status `blocked` → `blocked`. The Ready Status → `ready`. Any other `todo` → `todo`. Otherwise the Status name. An `active` Issue with a blocker stays in its column.
  - `pub fn plan_column(issue: &Value, col: &str, wf: &Value, viewer: &str, now: DateTime<Utc>) -> Plan`. Same column → `Refused "Already in this column."`. `blocked` → `Refused "Nothing is dropped onto Blocked. Issues land there when they gain an open blocker."`. Otherwise `plan_move` runs on each of the column's Statuses except `canceled`-category ones (Cancel is menu-only), and the `Run` actions are merged. If none run, return the first refusal whose reason isn't "No transition…".
  - `pub fn needs_human(issue: &Value, wf: &Value, now: DateTime<Utc>) -> Option<String>`. Closed → `None`. Otherwise the first cause that matches, as the band text:
    1. an open Ask (`properties.orbit.ask`) → `"Answer: {question}"`
    2. the Status is the `to` of an `ask` or `escalate` Transition → `"Decide: escalated"` (append ` by {who}` when `properties.orbit.escalation` names one)
    3. Stale → `"Take over or release"`
    4. label `needs-triage` → `"Triage this Issue"`
    5. label `needs-info` → `"Add the missing information"`
  - `pub fn ready_for_human(issue: &Value, wf: &Value) -> bool`: the Ready Status, label `ready-for-human`, no assignee, empty `blocking`.
  - `pub fn annotate(issue: &Value, wf: &Value, viewer: &str, now: DateTime<Utc>) -> Value` returns `{ "column": str, "needs_human": str|null, "ready_for_human": bool, "claim": "none"|"live"|"stale", "moves": { "<column id>": {"ok": bool, "reason"?: str, "actions"?: [{label, transition, confirm?}]} }, "menu": { "<status>": {same shape} }, "stale_actions": [{label, confirm}] }`. `moves` drives drag highlighting and `menu` drives the per-Status Move menu. Argv is never sent to the page.
- Exact rules (in order) for `plan_move`:
  1. Same status → `Refused "Already in this column."`
  1a. The Issue's column is `blocked` and the target is not `canceled`-category → `Refused "Blocked by {KEYS}. It moves on when that closes."` (or `"…when they close."`)
  2. Live → `Refused "Claimed by {assignee} until {HH:MM}. A claimed Issue can't be moved."`. Stale → `Refused "{assignee}'s lease expired {N} min ago. Take over or release it first."`
  3. No Workflow transition from the current status to the target → `Refused "No transition goes from {From} to {To}."`
  4. Drop the input transitions `submit`, `approve`, `request_changes`, `ask`, `escalate`, `answer`. If none remain, refuse with the prototype's `NEEDS_INPUT` message for the first one.
  5. **Review guard:** from any Status that is the `to` of a `submit` Transition, no move to a `done`-category status is offered. Refuse with `"Approving needs a review verdict. Use orbit review."` This closes the bypass the prototype found.
  6. For a closed-category target (except `reopen`): a non-empty `blocking` → `Refused "Still blocked by {KEYS}."`; `open_sub_issue_count > 0` → `Refused "{n} sub-issue(s) still open."`
  7. Argv per transition, with `V = updated_at`:
     - `done` / `cancel`: `[name, KEY, --by, viewer, --if-version, V]`
     - `reopen`: `[reopen, KEY, --if-version, V]`
     - Into an `active` status: `[assign, KEY, --to, viewer, --kind, human, --if-version, V]`. If another assignee is set, add `--force` and a `Confirm { title: "Replace {a} as Assignee?", ok: "Replace and move" }`.
     - Anything else: `[transition, KEY, name, --by, viewer, --if-version, V]`
- `stale_actions` (only when Stale): Take over when `claim` allows the current status, with `[assign, KEY, --to, viewer, --kind, human, --force, --if-version, V]`. Release when `release` allows it, with `[release, KEY, --by, viewer, --force, --if-version, V]`. Both carry a Confirm whose body copy is the prototype's.

- [ ] **Step 1: Write failing tests**, each building issue JSON with `serde_json::json!` and the `standard` workflow template:
  - `live_claim_is_refused`, `stale_claim_is_refused_with_takeover_hint`
  - `ready_to_in_progress_assigns_viewer_without_force`: args `== ["assign","ORB-1","--to","me","--kind","human","--if-version","V"]`, `confirm.is_none()`
  - `in_progress_over_lease_free_assignee_forces_and_confirms`: args contain `--force`, and the confirm title is `"Replace maria as Assignee?"`
  - `drop_on_done_runs_orbit_done`, `drop_on_done_refused_while_blocked` (reason contains `ORB-9`), `drop_on_done_refused_with_open_sub_issues`
  - `in_review_to_done_is_refused` (review guard)
  - `in_progress_to_in_review_needs_input` (reason contains `orbit submit`)
  - `todo_to_ready_runs_promote_transition`, `done_to_ready_runs_reopen`
  - `stale_actions_offer_takeover_and_release_with_force` (both args contain `--force` and `--if-version`), `stale_actions_empty_when_live`
  - `annotate_never_exposes_argv`: the serialized output contains no `"args"` key
  - `columns_follow_fixed_order_on_standard`: ids `== ["blocked","todo","ready","in_progress","in_review","done"]`
  - `columns_on_agentic_put_needs_decision_in_todo`: on the `agentic` template the `todo` column's statuses contain `needs_decision` and `needs_human` but not `blocked`
  - `ready_issue_with_open_blocker_sits_in_blocked`, `in_progress_issue_with_blocker_stays_in_progress`
  - `blocked_card_can_only_be_cancelled`: `plan_column(.., "ready")` is refused with a reason naming the blocker; `plan_move(.., "cancelled")` runs
  - `drop_on_done_column_never_cancels`: actions from `plan_column(.., "done")` are only `done`
  - `needs_human_reasons_in_priority_order`: Ask beats escalation, and the label cases return their exact text
  - `in_review_is_not_needs_human`, `waiting_on_link_is_not_needs_human`
  - `ready_for_human_requires_label_ready_status_and_no_blocker`
- [ ] **Step 2:** Run `cargo test --lib board_view::plan`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --lib board_view::plan`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(board-view): add the move planner`.

---

### Task 4: `orbit board view` command, server loop, read routes

**Files:**
- Modify: `src/cli.rs`: add `BoardCommand::View(BoardViewArgs)`, with `BoardViewArgs { #[arg(long, default_value_t = 0)] port: u16, #[arg(long)] by: Option<String>, #[arg(long)] no_open: bool }` and help from `help::BOARD_VIEW_LONG` / `BOARD_VIEW_EXAMPLES`.
- Modify: `src/help.rs` (those two constants).
- Modify: `src/commands/mod.rs`: dispatch `BoardCommand::View` → `concurrency.reject_idempotency("orbit board view")?; crate::board_view::serve(dir.as_deref(), args)` returning a silent `CommandOutput`, like `Command::Mcp`.
- Modify: `src/registry.rs`: `exempt(&["board", "view"], "Serve the Board view in a browser.", "is a local web server, not a Board operation")`.
- Modify: `src/capabilities.rs`: add `"board-view"` to `SUPPORTED`.
- Modify: `src/mcp.rs`: make `run_orbit` and `OrbitRun` `pub(crate)` for reuse (no behavior change).
- Modify: `src/board_view/mod.rs`: `pub fn serve(dir: Option<&Path>, args: &BoardViewArgs) -> Result<()>`.
- Create: `src/board_view/assets/index.html`, `app.js`, `app.css` (Task 6 fills these; for now, placeholders whose `index.html` links `/app.css` and `/app.js`).
- Modify: `docs/commands.md`: run `orbit schema --docs` and paste the generated section; add an `orbit board view` subsection under "Maintain the board", next to `board move`.
- Test: `tests/board_view.rs`.

**Interfaces:**
- Consumes: `http::{read_request, write_response, Response}`, `guard::{Guard, new_token}`, `plan::annotate`, `mcp::run_orbit`.
- Produces:
  - Startup line on stdout (flushed): `orbit board view: http://127.0.0.1:{port}/#token={token}`. In `--json` mode: `{"ok":true,"data":{"url":"…"}}`.
  - Routes: `GET /` → `index.html`, `GET /app.js`, `GET /app.css` (host check only); `GET /api/snapshot` (guarded) → `{viewer, workflow, issues: [issue + annotate(issue)]}` with `ETag: "<sha256 hex>"` and `304` on matching `If-None-Match`; `GET /api/show/{id}` (guarded) → `orbit show {id} --json` data. Anything else → 404.
  - Errors from a spawned CLI pass through as `{ok:false, error}` with status 502 when the envelope is unparseable, and the envelope's own status (409 for `VERSION_CONFLICT`, 422 otherwise) when it parses.
  - `fn open_browser(url: &str)`: `open` on macOS, `xdg-open` on other Unix, `cmd /C start "" <url>` on Windows. Failures are printed as a warning and never abort.
  - Per-connection read timeout: `Duration::from_secs(5)`.

- [ ] **Step 1: Write failing integration tests** in `tests/board_view.rs`. A helper `start(repo) -> (Child, String /*base*/, String /*token*/)` spawns `orbit board view --no-open --port 0 --by tester`, reads the first stdout line and parses the URL. A tiny `fn http(base, method, path, headers, body) -> (u16, Vec<(String,String)>, String)` uses `std::net::TcpStream`.
  - `capabilities_lists_board_view`: `capabilities --json` → `data.capabilities` contains `"board-view"`.
  - `serves_page_and_assets_with_csp`: `GET /` → 200, `content-security-policy: default-src 'self'`, and the body has no `<script>` without `src` and no `style=`.
  - `snapshot_requires_token` → 401 without a token, 200 with one; `data.issues[0].moves` exists.
  - `snapshot_rejects_rebinding_host` → 403 with `Host: evil.example:{port}`.
  - `snapshot_etag_round_trip`: the second request with `If-None-Match` → 304, empty body.
  - `silent_client_does_not_block_server`: open a socket and send nothing, then `GET /api/snapshot` on a second socket gets 200 within 7 s.
- [ ] **Step 2:** Run `cargo test --test board_view`. Expected: FAIL.
- [ ] **Step 3:** Implement `serve`: resolve the viewer, bind `127.0.0.1:{port}`, read back the real port, create the `Guard`, print the line, call `open_browser` unless `--no-open`, then loop `accept → set_read_timeout → read_request → route → write_response`. Errors on one connection are logged to stderr and the loop continues. The workflow comes from `orbit capabilities --json`, read once at startup.
- [ ] **Step 4:** Run `cargo test --test board_view` and `cargo test --lib registry`. Expected: PASS, including the registry/docs parity tests.
- [ ] **Step 5:** Commit: `feat(board-view): serve the Board snapshot over a local guarded server`.

---

### Task 5: Write routes: move, Take over, Release

**Files:**
- Modify: `src/board_view/mod.rs` (routes) and, if it grows, create `src/board_view/write.rs`.
- Test: `tests/board_view.rs`.

**Interfaces:**
- Consumes: `plan::{plan_move, stale_actions, Plan, Action}`, `run_orbit`.
- Produces:
  - `POST /api/move` with body `{"id": str, "column"?: str, "status"?: str, "version": str, "transition"?: str, "confirmed"?: bool}`. A drag sends `column` (planned with `plan_column`). The Move menu sends `status` (planned with `plan_move`). Exactly one must be present.
  - `POST /api/takeover` and `POST /api/release` with body `{"id": str, "version": str, "confirmed"?: bool}`.
  - Flow for each: validate `id` against `^[A-Za-z0-9][A-Za-z0-9._-]{0,80}$` (else 400) and `column`/`status`/`transition` against `columns(wf)` and the Workflow's names (else 400). Then run a fresh `orbit show {id} --json` and re-plan against it with `now`. Outcomes:
    - `Refused` → 409 `{ok:false, error:{code:"MOVE_REFUSED", detail: reason}}`.
    - Several actions and no `transition` → 409 `{code:"CHOOSE_TRANSITION", choices:[{transition,label}]}`.
    - `confirm` set and `confirmed != true` → 409 `{code:"CONFIRM_REQUIRED", confirm:{title,body,ok}}`.
    - Otherwise run the action's argv, with **the request's `version` replacing `V`** in `--if-version`, and pass through the CLI envelope.

- [ ] **Step 1: Write failing integration tests** (each with `TestRepo`, `add`, then the HTTP helper with token and `Origin`):
  - `move_ready_to_in_progress_assigns_viewer`: then `show` has `assignee == "tester"`, `assignee_kind == "human"`, no lease.
  - `move_refuses_live_claim`: `claim --by agent-1`, then move → 409 `MOVE_REFUSED`; the Issue is unchanged.
  - `takeover_requires_confirmation_then_succeeds`: claim + `make_stale`; the first POST → 409 `CONFIRM_REQUIRED`; the POST with `confirmed:true` → ok; `assignee == "tester"` and the status is still `in_progress`.
  - `release_stale_claim_returns_issue_to_ready`.
  - `takeover_loses_to_returning_agent`: claim + `make_stale`, read `version`; then agent-1 runs `claim` again (it renews and bumps `updated_at`); takeover with the old version and `confirmed:true` → 409 with code `VERSION_CONFLICT`; `assignee == "agent-1"`.
  - `move_rejects_option_like_id`: id `--force` → 400, and no CLI is spawned (the Issue list is unchanged).
  - `post_without_origin_is_forbidden` → 403.
  - `in_review_to_done_is_refused` (Board using the `standard` template, Issue submitted).
- [ ] **Step 2:** Run `cargo test --test board_view`. Expected: new tests FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --test board_view`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(board-view): run moves, Take over, and Release through the CLI`.

---

### Task 6: The page, and the docs that ship with it

Port the prototype into the three embedded assets. Keep: Columns and Queue layouts, the header layout switch (saved in `localStorage` as `orbit.layout`, plus `?layout=`), filters (search, Type, Assignee with Me / Unassigned / Agents / People, Repository, Epic; the Closed window dropdown replaces the old Show all closed checkbox), the six fixed columns (Blocked cards not draggable, a Status chip when a card is not in its column's home Status, cancelled cards muted in Done), the Needs a human tint and band and the For a human tag, pills (ready for agents, ready for a human, need a human, in progress, claimed, stale, ready to close; mutually exclusive), Queue sections (Needs a human, In progress, Ready, Blocked, Todo, Closed recently), claim bars, lease rings, epic sub-issue rollups and Parent keys on cards, dependency chips and hover edges, the drawer / detail pane with description, Links, Checkpoint and Move list, the keyboard "Move to…" menu (`m`), and `j`/`k` in Queue.

Drop: demo data, the "What the server would run" log, the variant switcher, the conflict-simulation toggle, and the Google Font. Drop the client-side rule engine: drop targets and reasons come from each Issue's `moves` and `stale_actions` in the snapshot.

**Files:**
- Modify: `src/board_view/assets/index.html`, `app.js`, `app.css` (no inline `<script>`, `<style>` or `style=` attributes; set styles from JS through `element.style`, which CSP allows).
- Modify: `README.md` (a short "Board view" section with the command and a one-line security note), `docs/SPEC.md` (replace the "Web UI." non-goal with "A hosted or multi-user Web UI (the local Board view is ADR 0023)."), `docs/ARCHITECTURE.md` (add `src/board_view/` to the component map and the Maintenance row), `CHANGELOG.md` (Unreleased: Added `orbit board view`).
- Test: `tests/board_view.rs`.

**Interfaces:**
- Consumes: the Task 4 and Task 5 routes and error codes. The page reads the token from `location.hash`, then calls `history.replaceState` to strip it from the address bar.
- Client flow for a drop or action: POST → on `CHOOSE_TRANSITION` show the chooser and resend with `transition`; on `CONFIRM_REQUIRED` show the dialog with the server's copy and resend with `confirmed:true`; on `VERSION_CONFLICT` toast "{KEY} changed while you were looking. Nothing was written." and refresh; on `MOVE_REFUSED` toast the detail.
- Polling: every 3000 ms with `If-None-Match`, paused while a drag, dialog or menu is open or `document.hidden`.

- [ ] **Step 1: Write the failing test** `assets_have_no_inline_code_and_no_external_urls`: `GET /app.js` and `GET /app.css` → 200. `GET /` body contains `src="/app.js"` and has no `<style`, ` style=`, `on[a-z]+=` attribute, `innerHTML`, `https://` or `http://`. `app.js` contains no `innerHTML` and no `eval(`.
- [ ] **Step 2:** Run `cargo test --test board_view assets_`. Expected: FAIL.
- [ ] **Step 3:** Port the assets and update the docs listed above.
- [ ] **Step 4:** Run `cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test --all`. Expected: all pass.
- [ ] **Step 5: Manual check in a browser** against this repo's Board (`cargo run -- board view`). Confirm: both layouts render; dragging a ready card to In progress makes you the Assignee; a claimed card cannot be dragged; Take over on a Stale card (make one with `orbit claim` and wait, or use the test helper) shows the server's confirmation copy; light and dark mode are both legible; at 375 px width the layout is usable.
- [ ] **Step 6:** Commit: `feat(board-view): ship the Columns and Queue page`. Then capture the prototype on a throwaway branch: `git switch -c prototype/board-view`, commit `scripts/board-view-prototype/`, switch back, and delete the folder and the `board-view-prototype` entry in `.claude/launch.json` from the feature branch.
