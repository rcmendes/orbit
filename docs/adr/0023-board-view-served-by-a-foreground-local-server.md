---
status: accepted
---

# ADR 0023: The Board view is served by a foreground local server

`docs/SPEC.md` listed a Web UI as a v1 non-goal. We now add a Board view: a
kanban of the Board in a browser. `orbit board view` serves it on `127.0.0.1`
while the command runs in the foreground and stops on Ctrl-C. It opens the
default browser at the URL unless `--no-open` is given. The "no daemon or
background watcher" non-goal still holds, so nothing stays running after the
command exits. Like `orbit mcp`, the server is a bridge that spawns the `orbit`
CLI with `--json` for every read and write (ADR 0004). It never reimplements an
operation, so the Board lock, Workflow, policy, and auto-commit apply unchanged.

**Every Issue is a card, and a claimed Issue never moves.** Epics are cards too,
with a sub-issue rollup, and a sub-issue card shows its Parent's key. In v1 the
writes are drops and the two Stale-card actions below. A claimed card (an
Assignee holding a Lease, live or Stale) cannot be dragged. The server checks
this itself before running anything.

**Columns group Statuses by category, in a fixed order.** This replaces "one
column per Workflow Status" and works on any Workflow without configuration:

1. **Blocked**: every Issue in a `todo`-category Status with an open Dependency,
   plus any Status a Workflow names `blocked`. Blocked is still a derived
   condition, never stored; the column shows it. Nothing is dropped onto
   Blocked, and its cards cannot be dragged (their Move menu offers Cancel).
   They leave it on their own when their blockers close. An Issue in an
   `active` Status that gains a blocker stays where it is, with a chip.
2. **Todo**: every other `todo`-category Status except the Ready one (for
   example `todo`, `needs_human`, `needs_decision`). A card shows its Status
   as a chip when it is not the column's default.
3. **Ready**: the Workflow's Ready Status.
4. One column per `active` Status, in Workflow order (In progress, In review).
5. **Done**: the `done` and `canceled` Statuses. Cancelled cards are muted and
   marked. A drop runs `done`; Cancel is in the Move menu.

When a drop could reach more than one Status in a group, the viewer picks one.

**Two cross-column highlights.** **Needs a human** (glossary) tints the whole
card and adds a band naming the step a person must take: answer the Ask, decide
an Escalation, take over or release a Stale claim, triage, or add missing
information. Escalation holds are the targets of the Workflow's `ask` and
`escalate` Transitions, not fixed names. In Review and `waiting-on` Links do
not count: agents review too, and a `waiting-on` Link waits on a third party.
**Ready for a human** (glossary) stays a plain card in Ready with a "For a
human" tag: it is work to pick up, not an alarm.

**Filters.** Exclusive header pills — **Ready for agents** (exactly the `orbit
ready` set), **Ready for a human**, **Needs a human**, **In progress**,
**Claimed**, **Stale**, and **Ready to close** — combine with the Assignee,
Type, Repository, and Epic filters.

**Two layouts.** A prototype compared three layouts; two were kept. **Columns**
is the kanban described above. **Queue** groups the same Issues into Needs a
human, In progress, Up next, and Closed recently, with a detail pane beside the
list and keyboard moves. A switch in the
header changes layout, and the browser remembers the choice. Both layouts share
the filters, including one by Assignee. A swimlane layout (one row per Assignee)
was rejected: the Assignee filter covers that need with less screen.

**What a drop runs.**
- A drop onto a `done` or `canceled` Status runs `orbit done` or `orbit cancel`,
  keeping their Dependency and open-sub-issue gates.
- A drop onto an `active` Status makes the viewer the Assignee, without a Lease,
  but only on the Workflow's `claim` route. Any other move into an active Status
  runs the named Transition and keeps the Assignee.
  If someone else is assigned without a Lease, the viewer replaces them after
  confirming.
- Any other drop runs `orbit transition`. Leaving an `active` Status already
  clears the Assignee.
- Moves that need input (`submit` evidence, a `review` verdict, an Ask answer)
  are not drop targets.
- An Issue in review is never dropped onto a `done`-category Status, even where
  the Workflow lets `done` start from review. Approval goes through
  `orbit review`, so a drag cannot skip the review verdict.
- While a card is dragged, the columns it can drop on are highlighted; the
  rest fade and say why the move is refused.
- When more than one Transition reaches the target, the viewer picks one.

**Stale cards offer Take over and Release, after confirmation.** **Take over**
makes the viewer the Assignee without a Lease, keeping the Status and Checkpoint
(a Takeover). **Release** clears the claim and returns the Issue to the
Workflow's release target, like `orbit release`. Each action appears only when
its Transition (`claim` or `release`) is allowed from the card's Status;
otherwise the card says to release it from the CLI. A returning agent learns of
a Takeover through the existing `NOT_CLAIM_HOLDER` error, and no automatic
Comment is added.

**`--force` only with `--if-version`, after a check and a confirmation.**
Replacing another Assignee (Take over, Release, or an active drop over a
lease-free Assignee) needs `--force` in the kernel. The server passes it only
after it has checked the precondition itself (the claim is Stale, or there is no
Lease) and the viewer has confirmed in a dialog that names who is replaced.
Every write carries the card's `updated_at` as `--if-version`. A heartbeat or
claim updates `updated_at`, so an agent that comes back between the check and
the write makes the write fail with a retryable conflict instead of losing its
claim. The kernel's assign and release semantics do not change.

**One human actor per run.** `orbit board view --by <name>` (default
`ORBIT_AGENT`, then `USER`) is the identity every write records, with Identity
Kind `human`.

**Freshness by polling, not watching.** The browser polls one snapshot endpoint
(`orbit list --json --all --include-closed`) every few seconds; it takes about
60 ms on a 160-Issue Board. An ETag saves the transfer for an unchanged Board,
but not the read. The Change feed is not used for this, because lease-only
changes are not committed until `orbit sync`, so lease badges would go stale.

**Hand-rolled HTTP and an embedded page.** The server is a single-threaded
HTTP/1.1 loop on `std::net` with strict request-size limits, following ADR 0003.
It adds no Tokio and no HTTP crate. The page is plain HTML, JavaScript, and CSS,
embedded with `include_str!`, with no build step and no CDN.

**Local-only security.** The server binds to `127.0.0.1` on a random free port
unless `--port` is given. A per-run secret token travels in the URL fragment
(`#token=…`), so it never reaches a request line, a log, or a `Referer`. The
page reads it, keeps it in the tab's `sessionStorage` so a reload keeps
working (cleared on a 401), and sends it as a header on every API call. The server rejects any
request whose `Host` or `Origin` is not local. The HTML, JavaScript, and CSS are
served as separate same-origin routes, so the CSP can stay `default-src 'self'`
with no inline script or style. Responses also send `Referrer-Policy:
no-referrer`. All Issue text is rendered as text, never as HTML, because Issues
may come from a Remote Tracker.

## Considered options

- **Static HTML export** (`orbit export --html`). Rejected: it cannot run
  Transitions, and drag-and-drop is the core of a kanban.
- **Hosted multi-user service.** Rejected: it contradicts the "no hosted sync"
  non-goal and needs authentication. A Board host already covers sharing.
- **Calling `ops::*` in-process.** Rejected for v1: spawning the CLI gives exact
  parity with the CLI and MCP at the cost of one process per request. ADR
  0004's escape hatch applies here too.
- **Letting `assign` and `release` take a Stale claim without `--force`.**
  Rejected: it changes ADR 0022's frozen kernel contract for every caller, and
  `--if-version` already makes the server-side check race-free.
- **Polling the Change feed.** Rejected: it misses lease-only changes.
- **`axum` + Tokio, or a SPA framework with a build step.** Rejected: both add
  runtime and build weight, and the view needs only a handful of routes on
  localhost.

## Consequences

- The "Web UI" line in `docs/SPEC.md` non-goals is replaced by a pointer to this
  ADR when the feature lands.
- `orbit board view` is a front door like `orbit mcp`: it is exempt from MCP
  parity in the command registry, listed in `docs/commands.md`, and reported as
  a `board-view` Capability. It is not a Board module.
- This build bumps the contract to 1.1.0, so Boards it creates or migrates are
  refused by orbit 1.0.x with `CONTRACT_VERSION_UNSUPPORTED`; existing 1.0.0
  Boards keep working.
- The HTTP parser is a trust boundary and gets its own tests.
- Issues had no close timestamp, so v1 showed closed Issues whose `updated_at`
  fell in the last 14 days. ADR 0024 adds `resolved_at`; the **Closed window**
  amendment below replaces that shortcut.

## Amendment: configurable Closed window

The fixed 14-day window and the "Show all closed" checkbox are replaced by a
**Closed window** (glossary).

- **One window, both layouts, both categories.** It governs the Done column in
  Columns and Closed recently in Queue, and `done` and `canceled` share it.
- **Header dropdown "Closed:"** with `24 hours`, `3 days`, `1 week`, `1 month`,
  `All`, and `Custom…`. Custom is an absolute from–to date range, kept for the
  current session only and never saved.
- **Three layers.** Built-in default `14d`; a team default in the Board config,
  `[board_view] closed_window = "14d"` (`<N>h|d|w` or `"all"`; a value outside
  the presets appears as an extra dropdown entry); and a per-viewer choice kept
  in `localStorage` (`orbit.closedWindow`) with a reset to the team default.
  There is no CLI flag.
- **Delivery.** The server reads the config value and sends it in the snapshot
  response; the page falls back to `14d` if it is absent.
- **Invalid value.** A malformed `closed_window` falls back to `14d`;
  `orbit doctor` reports it. The Board view still starts.
- **Hidden is not gone.** The Done column ends with "+N older · show" for cards
  outside the window. A text search that matches an Issue shows it regardless of
  the window; the Type, Epic, and Assignee filters still respect it.
- **Display filter only.** Hidden closed Issues still count in Epic rollups,
  Blocked-by lists, Ready logic, and the Ready to close pill.
