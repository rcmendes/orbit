//! Long descriptions and examples printed by `orbit <command> --help`.
//!
//! The one-line summaries live as doc comments on the `Command` variants in
//! [`crate::cli`]. Everything here is attached as `long_about` or
//! `after_long_help`, so `-h` stays compact while `--help` explains the
//! semantics and shows copy-pasteable usage.

pub const ROOT_LONG: &str = "\
Orbit keeps every task as a Markdown file on a dedicated Git branch and serves it
through this CLI. Writes are serialized by one lock and published atomically, so
several agents can share a Board without a database server.

Run `orbit init` once per repository, then let agents take work with
`orbit ready --claim`, hold it with `orbit heartbeat`, and finish it with
`orbit done`. Every command accepts --json and then prints exactly one JSON
object on stdout.

Board discovery order: --dir, then ORBIT_DIR, then the repository link, then the
repository's own Board.

For work that spans repositories, create a shared workspace with
`orbit workspace <folder>` and link each repository with `orbit link <folder>`.";

pub const ROOT_EXAMPLES: &str = "\
Typical agent loop:
  orbit init
  orbit add \"Build the CLI router\" -p 1 --label ready-for-agent
  orbit ready --claim --by claude
  orbit heartbeat orb-a7b2c9 --by claude
  orbit done orb-a7b2c9 --by claude --comment \"Implemented and verified\"

Inspect work without changing it:
  orbit ready --limit 5
  orbit show orb-a7b2c9 --json
  orbit list --status in_progress --claimed-by claude";

pub const INIT_LONG: &str = "\
Creates the Board as a hidden Git worktree on the orbit branch, then detects
installed coding-agent harnesses (Claude Code, Cursor, Codex, OpenCode, and the
mattpocock bridge) and writes only the integrations that apply. Files carry a
managed marker, so re-running reconciles integration instead of failing. A
hand-written docs/agents/issue-tracker.md without the marker is reported as
skipped and left alone.

In a terminal, init asks which harnesses to wire. Without a terminal it wires the
ones it detects. Use --board to create a shared Board and link this repository to
it, or --local to create a repository Board even when a shared one is linked.";

pub const INIT_EXAMPLES: &str = "\
Examples:
  # First-time setup in this repository
  orbit init

  # See every file init would create or update, without writing
  orbit init --print

  # Create a shared Board for a multi-repository workspace
  orbit init --board ~/src/acme-board

  # Create the Board only; skip agent integration
  orbit init --agents none";

pub const WORKSPACE_LONG: &str = "\
Creates a workspace directory and a shared Board at <folder>/board. The Board is
an external Board: its own Git repository on the orbit branch, independent of
every member repository.

The command only creates. It does not clone repositories, link them, or write
agent configuration. Each repository joins explicitly with
`orbit link <folder>`. A workspace must live outside every Git working tree, so
the Board is never tied to one repository's lifecycle. Re-running is safe: an
existing Board is reported, not overwritten.";

pub const WORKSPACE_EXAMPLES: &str = "\
Examples:
  # Create a workspace next to the repositories
  orbit workspace acme

  # Join from each repository root
  orbit link ~/src/acme

  # Inspect the workspace as JSON
  orbit workspace acme --json";

pub const LINK_LONG: &str = "\
Writes a repository-local pointer to an existing Board. PATH may be a Board
directory or a workspace root; a workspace root resolves to its board/
subdirectory, so `orbit link <workspace>` joins a workspace created with
`orbit workspace`. Task commands in this repository then read and write that
Board, including from all of its Git worktrees. Existing harness configurations
are refreshed to point at the shared Board unless --no-refresh is passed.

Linking does not delete a repository's own Board. If one exists it is shadowed,
and link prints a warning; the old Board stays on disk.";

pub const LINK_EXAMPLES: &str = "\
Examples:
  # Share one product Board from a second repository
  orbit link ~/src/acme-board

  # Join a workspace created with `orbit workspace`
  orbit link ~/src/acme

  # Link without rewriting agent configuration
  orbit link ~/src/acme-board --no-refresh";

pub const UNLINK_LONG: &str = "\
Removes the shared-Board pointer and refreshes agent configuration to use this
repository's own Board. The shared Board is left untouched for the other
repositories that use it.

If this repository has no local Board yet, the link is still removed and the
command warns you to run `orbit init`.";

pub const UNLINK_EXAMPLES: &str = "\
Examples:
  # Stop sharing the product Board
  orbit unlink

  # Create a local Board afterwards
  orbit init";

pub const ADD_LONG: &str = "\
Creates a task in the todo state and prints its ID (use --json to capture
data.id). The body holds Markdown context and acceptance criteria; --body-file -
reads it from stdin so long bodies never hit shell quoting.

Priority runs from 0 (highest) to 3 (lowest); the default comes from the Board
config (2 unless changed). --type sets the work shape: task (default), epic,
spike, chore, or decision. Type is metadata and does not affect readiness.
--depends-on is repeatable and creates blocking edges at creation time.
--waiting-on records a blocker that lives outside the Board, such as an upstream
release.

--parent creates a SubTask under an existing Parent Task. The SubTask ID is the
parent ID, a dot, and a fresh suffix (for example orb-a7b2c9.d4b3df). SubTasks are
one level deep. A spec or epic is a top-level Parent Task created with --type epic;
its tickets are SubTasks. Epic is refused on a SubTask. When an external tracker
or skill says \"sub-issue\", use --parent. Parentage is not a dependency: the
Parent Task waits for its SubTasks, not the other way around. A SubTask can still
have real --depends-on edges. Closing a Parent Task with open SubTasks is refused;
--force cancels them.";

pub const ADD_EXAMPLES: &str = "\
Examples:
  # File a high-priority bug with a Markdown body
  orbit add \"Login rejects valid tokens\" -p 1 --label bug --body-file ticket.md

  # Create work that waits for a blocker task to finish
  orbit add \"Publish the release\" -p 1 --depends-on orb-a7b2c9

  # Record an external blocker while the body comes from stdin
  orbit add \"Upgrade the parser\" --waiting-on \"upstream 2.4\" --body-file -

  # Create a top-level epic or spec
  orbit add \"Spec: SSO login\" --type epic --body-file spec.md

  # Break a feature into a SubTask
  orbit add \"API: add the endpoint\" --parent orb-a7b2c9 --label repo:api

  # Publish a ticket under a spec or epic parent
  orbit add \"web: login redirect\" --parent orb-a7b2c9 --label ready-for-agent";

pub const LIST_LONG: &str = "\
Lists tasks from a consistent snapshot of the Board. Done and cancelled tasks are
hidden by default; pass --all to include them, or name a terminal status with
--status. Filters combine, so --status in_progress --claimed-by claude shows
everything one agent is holding.

--parent lists only the SubTasks of one Parent Task. --tree keeps each Parent
Task next to its SubTasks and indents the children in human output. --type is
repeatable and matches any listed work shape; --exclude-type hides any listed
shape. --label is repeatable and every label must match; --exclude-label is
repeatable and hides a task carrying any of the named labels. --repo NAME is
shorthand for --label repo:NAME. Use --json when another program consumes the
result: it returns data.tasks and data.count. For work an agent can start, use
`orbit ready` instead of filtering this list.";

pub const LIST_EXAMPLES: &str = "\
Examples:
  # What is open right now?
  orbit list

  # Everything awaiting triage
  orbit list --label needs-triage

  # What is claude working on?
  orbit list --status in_progress --claimed-by claude

  # Full history, including closed work
  orbit list --all --json

  # The pieces of one feature
  orbit list --parent orb-a7b2c9

  # Parents and their SubTasks together
  orbit list --tree

  # Everything for one repository, optionally with another label
  orbit list --repo api
  orbit list --repo api --label ready-for-agent

  # Only epics and specs
  orbit list --type epic

  # Hide epics and specs from the list
  orbit list --exclude-type epic";

pub const READY_LONG: &str = "\
Shows tasks that can start now: todo, unclaimed, not waiting on an external
blocker, no hold label, and every dependency already done or cancelled. Results
are ordered by priority, then creation time.

With --claim, the first ready task is claimed atomically in the same command and
moves to in_progress with a lease; --by names the agent and requires --claim.
When nothing is ready the command still succeeds, with data null and
meta.reason NO_READY_TASKS, so a polling loop can wait and retry.

With --parent, only that Parent Task's Ready SubTasks are considered. --repo NAME
and repeatable --label scope the queue to a repository or a set of labels;
repeatable --type keeps only listed work shapes, and --exclude-type hides them,
which is how an agent skips epics, specs, or other containers. --exclude-label
remains for legacy label-only boards. Filters combine with --parent using AND.
Use them to scope an agent to one feature or one repository instead of the whole
Board.";

pub const READY_EXAMPLES: &str = "\
Examples:
  # See what is startable without taking anything
  orbit ready --limit 5

  # Atomically take the highest-priority ready task
  orbit ready --claim --by claude --json

  # Ask whether an agent should wake up
  orbit ready --json

  # Claim the next ready piece of one feature
  orbit ready --parent orb-a7b2c9 --claim --by claude

  # Claim the next task for one repository
  orbit ready --repo api --claim --by claude

  # Skip epics and specs while claiming normal work
  orbit ready --exclude-type epic --claim --by claude";

pub const SHOW_LONG: &str = "\
Prints the full task: frontmatter fields including type, Markdown body, labels,
dependencies, dependents, and every comment. `orbit show <id> --json` is the
machine-readable form and is how an agent should load a ticket before working on
it.";

pub const SHOW_EXAMPLES: &str = "\
Examples:
  # Read a task before starting work
  orbit show orb-a7b2c9

  # Load the ticket as JSON, including comments
  orbit show orb-a7b2c9 --json";

pub const TREE_LONG: &str = "\
Shows one Parent Task, its direct SubTasks, and the blocking edges around them.
Unlike `list --tree`, which shows the whole Board, and `list --parent`, which
shows only the children, `tree` always includes the root and its family. It is
the scoped view for an epic, spec, or feature; non-task types appear before the
title.

SubTasks are one level deep. `--json` returns data.root, data.tasks (root first),
and data.count; each row has the same shape as `list` and `ready`. The root line
shows resolved/total progress and is marked ready to close when every SubTask is
done or cancelled.";

pub const TREE_EXAMPLES: &str = "\
Examples:
  # Inspect one epic or spec and its tickets
  orbit tree orb-a7b2c9

  # Load the family for an agent
  orbit tree orb-a7b2c9 --json";

pub const CLAIM_LONG: &str = "\
Claims one specific task instead of taking the top of the ready queue. The task
must be active and unclaimed (a stale claim needs --steal), must not wait on an
external blocker or an unfinished dependency, and must carry no hold label such
as needs-triage or ready-for-human.

A claim sets status in_progress, records the agent, and starts a lease (30 minutes
by default, configurable in config.toml). Extend it with `orbit heartbeat`.
--steal takes over another agent's claim even while its lease is live; hold
labels still block. The agent defaults to ORBIT_AGENT, then USER.";

pub const CLAIM_EXAMPLES: &str = "\
Examples:
  # Take a specific task from the backlog
  orbit claim orb-a7b2c9 --by claude

  # Recover a task whose agent crashed
  orbit claim orb-a7b2c9 --by claude --steal

  # Inspect the claim and its lease
  orbit show orb-a7b2c9 --json";

pub const RELEASE_LONG: &str = "\
Clears the claim and returns the task to todo, so it becomes ready again. By
default only the agent holding the claim can release it; --force releases someone
else's claim. Terminal tasks cannot be released.

Use release when work is handed back deliberately. Use heartbeat while still
working, and done or cancel when the work is over.";

pub const RELEASE_EXAMPLES: &str = "\
Examples:
  # Hand a task back to the queue
  orbit release orb-a7b2c9 --by claude

  # Clear a claim left by an agent that is gone
  orbit release orb-a7b2c9 --force";

pub const HEARTBEAT_LONG: &str = "\
Extends the claim's lease from now, proving the agent is still working. Only the
claim holder can heartbeat; a stale claim is recovered with
`orbit claim --steal`. Heartbeat writes are lease-only: they are not committed
until the next meaningful mutation or `orbit sync`.";

pub const HEARTBEAT_EXAMPLES: &str = "\
Examples:
  # Keep the claim alive during a long build
  orbit heartbeat orb-a7b2c9 --by claude

  # List the tasks you currently hold
  orbit list --status in_progress --claimed-by claude";

pub const DONE_LONG: &str = "\
Moves the task to done and records the completion. Unfinished dependencies always
block completion. A Parent Task with open SubTasks is also refused; --force
cancels those SubTasks and then closes the parent, and also overrides a competing
live claim. Calling done on an already-done task is harmless, and with --comment
it appends another note.

Pass --comment or --comment-file to write the completion note in the same locked
transaction as the status change, so a note cannot be recorded before the task is
closed.";

pub const DONE_EXAMPLES: &str = "\
Examples:
  # Finish with a note recorded atomically
  orbit done orb-a7b2c9 --by claude --comment \"Implemented and verified.\"

  # Read the completion note from a file (or stdin with -)
  orbit done orb-a7b2c9 --by claude --comment-file notes.md

  # See what the completion unblocked
  orbit ready --json

  # Close a feature whose remaining SubTasks are no longer needed
  orbit done orb-a7b2c9 --force --comment \"Deferred the remaining pieces\"";

pub const CANCEL_LONG: &str = "\
Abandons a task without completing it. Cancelled tasks count as resolved for
dependents, so blocked work becomes ready. A Parent Task with open SubTasks is
refused; --force cancels those SubTasks too, and also overrides a live claim from
another agent. Pass --comment to record the reason with the status change.";

pub const CANCEL_EXAMPLES: &str = "\
Examples:
  # Drop work that is no longer needed
  orbit cancel orb-a7b2c9 --by claude --comment \"Superseded by orb-f14c02\"

  # Cancel another agent's task after a handoff
  orbit cancel orb-a7b2c9 --force

  # Abandon a feature and its remaining SubTasks
  orbit cancel orb-a7b2c9 --force --comment \"Superseded\"";

pub const REOPEN_LONG: &str = "\
Returns a done or cancelled task to todo and clears any claim. Reopening a SubTask
of a done Parent Task reopens the parent too. Reopening is refused when a
dependent task is already done, because that would leave a completed task with an
unfinished dependency; --force allows it. Doctor reports the resulting historical
warning, and a later completion of that dependent still enforces its
dependencies.";

pub const REOPEN_EXAMPLES: &str = "\
Examples:
  # A release regressed; reopen the fix
  orbit reopen orb-a7b2c9

  # Reopen even though a dependent was already completed
  orbit reopen orb-a7b2c9 --force

  # Reopen a SubTask; its done parent reopens with it
  orbit reopen orb-a7b2c9.d4b3df";

pub const UPDATE_LONG: &str = "\
Edits a task without changing its lifecycle. Use --title, --body/--body-file,
--priority, --type, or --waiting-on/--clear-waiting. Status, claims, labels,
comments, and dependencies have their own commands. --type epic is refused on a
SubTask.";

pub const UPDATE_EXAMPLES: &str = "\
Examples:
  # Reprioritize work
  orbit update orb-a7b2c9 --priority 0

  # Change the work shape
  orbit update orb-a7b2c9 --type spike

  # Record that an external blocker is gone
  orbit update orb-a7b2c9 --clear-waiting

  # Replace the body from a file
  orbit update orb-a7b2c9 --body-file refined-spec.md";

pub const LABEL_LONG: &str = "\
Labels are free-form lowercase slugs stored on the task. They are the triage state
and category mechanism: needs-triage, needs-info, ready-for-agent,
ready-for-human, wontfix, bug, enhancement.

Tasks carrying a configured hold label (needs-triage, needs-info, ready-for-human
by default) are not ready and cannot be claimed until the label is removed.
Adding and removing labels is idempotent.";

pub const LABEL_EXAMPLES: &str = "\
Examples:
  # Mark a ticket ready for an agent
  orbit label add orb-a7b2c9 ready-for-agent

  # File it as a bug and clear triage
  orbit label add orb-a7b2c9 bug
  orbit label remove orb-a7b2c9 needs-triage

  # See which labels are in use and how many tasks carry them
  orbit label list";

pub const LABEL_ADD_LONG: &str = "\
Adds labels that are not already present. Idempotent: existing labels are not
duplicated. Labels must be lowercase slugs.";

pub const LABEL_ADD_EXAMPLES: &str = "\
Examples:
  orbit label add orb-a7b2c9 bug needs-triage
  orbit label add orb-a7b2c9 ready-for-agent --json";

pub const LABEL_REMOVE_LONG: &str = "\
Removes labels from a task. Idempotent: removing a label the task does not carry
is not an error. Removing a hold label is what makes a triaged task ready again.";

pub const LABEL_REMOVE_EXAMPLES: &str = "\
Examples:
  orbit label remove orb-a7b2c9 needs-triage
  orbit label remove orb-a7b2c9 needs-triage needs-info";

pub const LABEL_LIST_LONG: &str = "\
Lists every label in use across the Board with the number of tasks carrying it.
There is no label registry; a label exists while some task carries it.";

pub const LABEL_LIST_EXAMPLES: &str = "\
Examples:
  orbit label list
  orbit label list --json";

pub const COMMENT_LONG: &str = "\
Comments form an append-only per-task thread with an author and RFC 3339
timestamp. Comments on terminal tasks are allowed. There is no edit or delete:
add a new comment that corrects the old one.

Use comments for triage notes, handoff context, and decisions that do not belong
in the task body.";

pub const COMMENT_EXAMPLES: &str = "\
Examples:
  orbit comment add orb-a7b2c9 --by claude --body \"Reproduced on macOS.\"
  orbit comment list orb-a7b2c9";

pub const COMMENT_ADD_LONG: &str = "\
Appends one comment to a task. The body can be passed inline, from a file, or from
stdin with --body-file -. The author defaults to ORBIT_AGENT, then USER.";

pub const COMMENT_ADD_EXAMPLES: &str = "\
Examples:
  # Record a triage decision
  orbit comment add orb-a7b2c9 --by claude --body \"Confirmed; needs a regression test.\"

  # Append a command's output
  cargo test 2>&1 | orbit comment add orb-a7b2c9 --by claude --body-file -";

pub const COMMENT_LIST_LONG: &str = "\
Lists a task's comments in the order they were appended. Use
`orbit show <id> --json` to get comments together with the task itself.";

pub const COMMENT_LIST_EXAMPLES: &str = "\
Examples:
  orbit comment list orb-a7b2c9
  orbit comment list orb-a7b2c9 --json";

pub const DEPEND_LONG: &str = "\
Creates a blocking edge: the first task will not be ready until the second one is
done or cancelled. The command rejects self-dependencies, missing tasks, cycles,
adding a dependency to a terminal task, and parent/child edges. Parentage is
created with `orbit add --parent`, not with depend.

Prefer `orbit add --depends-on` when the edge is known at creation time. Use
depend later, when a blocker is discovered after both tasks exist.";

pub const DEPEND_EXAMPLES: &str = "\
Examples:
  # \"Ship the API\" depends on \"Design the schema\"
  orbit depend orb-a7b2c9 orb-f14c02

  # Inspect the resulting graph
  orbit show orb-a7b2c9 --json

  # Parentage is not a dependency
  orbit add \"API: add the endpoint\" --parent orb-a7b2c9";

pub const UNDEPEND_LONG: &str = "\
Removes a blocking edge between two tasks. Idempotent: removing an edge that is
not there succeeds. The dependent task may become ready immediately, unless other
dependencies, an external blocker, or a hold label still block it.";

pub const UNDEPEND_EXAMPLES: &str = "\
Examples:
  orbit undepend orb-a7b2c9 orb-f14c02
  orbit ready --json";

pub const DELETE_LONG: &str = "\
Permanently removes a task and its comment thread from the Board. By default it
refuses when dependents exist, when the task has SubTasks, or when another agent
holds a live claim; --force removes the ID from each dependent's dependency list,
deletes the whole SubTask subtree, and deletes anyway.

Prefer cancel when the work should stay visible in history. Delete is for tasks
created by mistake.";

pub const DELETE_EXAMPLES: &str = "\
Examples:
  # Remove a duplicate ticket
  orbit delete orb-a7b2c9

  # Remove a task that other tasks still reference
  orbit delete orb-a7b2c9 --force

  # Remove a feature and every SubTask under it
  orbit delete orb-a7b2c9 --force";

pub const PRIME_LONG: &str = "\
Prints the agent workflow instructions, Board counts, the top ready tasks, open
Parent Tasks ready to close, and stale claims. This is the context an agent
session starts with; the Claude Code SessionStart hook calls
`orbit prime --hook-json` automatically.

--repo NAME and repeatable --label scope the counts, ready tasks, close
candidates, and stale claims to a repository or label set. --hook-json prints
Claude Code's hook envelope instead of the normal output and cannot be combined
with --json.";

pub const PRIME_EXAMPLES: &str = "\
Examples:
  # Start an agent session with Board context
  orbit prime --by claude

  # Inspect stale claims through JSON
  orbit prime --json

  # Scope the session context to one repository
  orbit prime --by claude --repo api";

pub const EXPORT_LONG: &str = "\
Emits a deterministic JSON snapshot of the whole Board: every Task and SubTask,
including terminal work, with type, labels, dependency and parent edges, derived
readiness, and comments. The snapshot is ordered by task ID and contains no
timestamps or machine-local paths, so the same board state always produces the
same data.

The top-level data object carries schema_version; consumers should pin it. Use
export to feed a knowledge base, archive a board, or diff two snapshots.";

pub const EXPORT_EXAMPLES: &str = "\
Examples:
  # Write a snapshot for a collector
  orbit export --json > board.json

  # Inspect the schema version and task count
  orbit export --json | jq '.data | {schema_version, tasks: (.tasks | length)}'";

pub const SETUP_LONG: &str = "\
Installs or repairs one harness integration. Each client writes its instruction
file, skill, and MCP configuration as appropriate; Claude can also receive the
SessionStart hook with --hooks. Re-running is safe: only managed sections are
rewritten. A hand-written docs/agents/issue-tracker.md without the marker is
reported as skipped and left alone.

Use init for first-time setup. setup is for adjusting one harness later, checking
drift in CI with --check (non-zero when missing or stale), or removing Orbit's
content with --remove.";

pub const SETUP_EXAMPLES: &str = "\
Examples:
  # Add the Claude hook and MCP entry
  orbit setup claude --hooks

  # Check that every integration is installed and current
  orbit setup all --check

  # See what would change without writing
  orbit setup cursor --print

  # Remove only Orbit-managed content
  orbit setup all --remove";

pub const MCP_LONG: &str = "\
Runs the Model Context Protocol server over stdio. Editors start this process from
their MCP configuration, so it is rarely run by hand. It exposes 13 tools (ready,
list, get, create, update, claim, release, heartbeat, finish, link, delete, label,
comment) and the orbit://tasks/{id} Markdown resource, reusing the CLI's lock,
atomic writes, and auto-commit. `create` accepts `parent` to make a SubTask;
`list` and `ready` accept `parent`, `repo`, and `labels` to scope a feature or
repository.

Use `orbit mcp` as the command in a client configuration. Pass --dir globally to
serve a specific Board.";

pub const MCP_EXAMPLES: &str = "\
Examples:
  # Run the server (normally launched by the editor)
  orbit mcp

  # Serve a shared Board explicitly
  orbit --dir ~/src/acme-board mcp";

pub const DOCTOR_LONG: &str = "\
Checks the Board for malformed tasks, dangling dependencies, missing or nested
parents, cycles, orphan or corrupt comments, stale temp files, uncommitted
changes, the checked-out branch, and a Claude hook that cannot find the Orbit
binary on PATH.

It exits 1 when it finds issues. --fix only removes stale temp files; it never
rewrites tasks or comments.";

pub const DOCTOR_EXAMPLES: &str = "\
Examples:
  # Health check after a crash or merge
  orbit doctor

  # Clean up stale temp files left by an interrupted write
  orbit doctor --fix

  # Inspect the reported issues as JSON
  orbit doctor --json";

pub const SYNC_LONG: &str = "\
Commits pending board changes, usually the lease updates written by heartbeat, to
the board branch. Heartbeats deliberately do not commit on every call; a later
mutation, or this command, persists them. Nothing is pushed.

Use sync after a long heartbeat-only period, or when doctor reports uncommitted
Board changes.";

pub const SYNC_EXAMPLES: &str = "\
Examples:
  orbit sync
  orbit sync --json";
