//! Long descriptions and examples printed by `orbit <command> --help`.
//!
//! The one-line summaries live as doc comments on the `Command` variants in
//! [`crate::cli`]. Everything here is attached as `long_about` or
//! `after_long_help`, so `-h` stays compact while `--help` explains the
//! semantics and shows copy-pasteable usage.

pub const ROOT_LONG: &str = "\
Orbit is a Git-native issue tracker for humans and coding agents. It keeps every
task as a Markdown file on a dedicated Git branch and serves it through this CLI.
Writes are serialized by one lock and published atomically, so several agents
can share a Board without a database server.

Run `orbit init` once per repository or Workspace root, then let agents take work
with
`orbit ready --claim`, hold it with `orbit heartbeat`, record a handoff with
`orbit checkpoint`, and finish it with `orbit done`. Every command accepts --json
and then prints exactly one JSON object on stdout.

Board discovery order: --dir or ORBIT_DIR, then the repository link, then the
containing Workspace's .orbit/board, then the nearest repository .orbit, then
the main checkout's .orbit, then a legacy Board, then ~/.orbit when it exists.

For repositories inside one Workspace, run `orbit init <WORKSPACE>` once; child
repositories discover the Board without per-repository links. Repositories
outside it can join with `orbit link <WORKSPACE>`.";

pub const ROOT_EXAMPLES: &str = "\
Typical agent loop:
  orbit init
  orbit add \"Build the CLI router\" -p 1 --label ready-for-agent
  orbit ready --claim --by claude
  orbit heartbeat orb-a7b2c9 --by claude
  orbit checkpoint orb-a7b2c9 --by claude --progress \"Parser done\" --next \"Wire the CLI\"
  orbit done orb-a7b2c9 --by claude --comment \"Implemented and verified\"

Inspect work without changing it:
  orbit ready --limit 5
  orbit show orb-a7b2c9 --json
  orbit list --status in_progress --claimed-by claude

Change a Board's Task-ID prefix:
  orbit slug show
  orbit slug migrate atlas --dry-run";

pub const INIT_LONG: &str = "\
Creates or reuses a Board. In a Git working tree, the default Board directory is
./.orbit, a worktree on the orbit branch. Outside a Git working tree, the current
directory is a Workspace root and the Board is <WORKSPACE>/.orbit/board. The
catalog cache is reserved at <WORKSPACE>/.orbit/cache; Workspace migration uses
<WORKSPACE>/.orbit/workspace-migration.lock for serialization. A positional path or
--dir/ORBIT_DIR can name a Workspace root; an explicit Board directory remains
accepted for existing Boards.

Inside a Workspace, child repositories find the Workspace Board across their Git
boundary and need no `orbit link`. Repositories outside it can link the Workspace
root. An existing child Board or an unrecognized .orbit layout is reported; init
does not shadow or discard it. A legacy Board directly at <WORKSPACE>/.orbit
remains available with `--dir <WORKSPACE>/.orbit`; back it up and migrate it to
<WORKSPACE>/.orbit/board before Workspace initialization.

--global targets ~/.orbit in repository mode. It conflicts with a positional DIR
and with --dir/ORBIT_DIR. Init refuses to create a second repository-local Board:
a link, an ancestor .orbit, the main checkout's .orbit, or a legacy Board is
reported before anything is written. --global and the ~/.orbit fallback are
exempt.

The slug comes from the Workspace or Board directory name. ~/.orbit and --global
use orb with a warning. Pass --slug to choose it explicitly. Re-running preserves
the stored prefix; an empty Board can change it, and a populated Board returns
SLUG_LOCKED pointing at `orbit slug migrate`.

Init then detects installed coding-agent harnesses (Claude Code, Cursor, Codex,
OpenCode, and the mattpocock bridge) and writes only the integrations that apply.
Files carry a managed marker, so re-running reconciles integration instead of
failing. A hand-written docs/agents/issue-tracker.md without the marker is
reported as skipped and left alone. In a terminal, init asks which harnesses to
wire; without a terminal it writes no agent files and prints the `orbit setup`
hint. A preview (`--print`) uses detection to show what would change but never
prompts and never writes. Use --agents all, none, or a comma-separated list to
select harnesses without a prompt.

Every run prints a numbered summary of the Board, the coding-agent integration,
and the `orbit doctor` verification step, followed by the next commands. A
standalone Board also prints the `orbit link` command that member repositories
need. `--print` shows the same summary in future tense and includes the generated
file contents.

MCP configurations record the absolute path of the running binary by default, so
GUI-launched editors do not need Orbit on PATH. Pass --portable-mcp to write the
bare `orbit` command instead; every editor or agent that launches the MCP server
must then find Orbit on its PATH. Generated configurations never record the
Board path; they rely on discovery or on --dir/ORBIT_DIR.";

pub const INIT_EXAMPLES: &str = "\
Examples:
  # Create a repository-local .orbit, or a Workspace Board outside Git
  orbit init

  # Create a Workspace Board for child repositories
  orbit init ~/src/acme

  # Create the global Board at ~/.orbit
  orbit init --global

  # See every Board and agent file that would change, without writing
  orbit init --print

  # Choose the Task-ID prefix explicitly
  orbit init --slug teramigo

  # Create the Board only; skip agent integration
  orbit init --agents none

  # Write the portable `orbit` command and rely on the editor's PATH
  orbit init --portable-mcp

  # Record an explicit absolute binary for GUI-launched editors
  orbit init --mcp-bin /usr/local/bin/orbit";

pub const LINK_LONG: &str = "\
Writes a repository-local pointer to an existing Board. PATH can be a Workspace
root (the Board is at <WORKSPACE>/.orbit/board) or a Board directory. Legacy
workspace roots containing board/ and legacy board roots are accepted with a
LEGACY_LAYOUT warning. Task commands in
this repository then read and write that Board, including from all of its Git
worktrees. Existing harness configurations are refreshed unless --no-refresh is
passed.

Linking does not delete a repository's own Board. If one exists it is shadowed,
and link prints a warning; the old Board stays on disk. A missing target returns
LINK_NOT_FOUND; the pointer is not repaired automatically.";

pub const LINK_EXAMPLES: &str = "\
Examples:
  # Share a Workspace Board from an outside repository
  orbit link ~/src/acme

  # Link directly to a Board directory
  orbit link ~/src/acme/.orbit/board

  # Link without rewriting agent configuration
  orbit link ~/src/acme --no-refresh";

pub const WORKSPACE_MIGRATE_LONG: &str = "\
Workspace migration combines the legacy Workspace Board at <WORKSPACE>/.board
(or <WORKSPACE>/board) and Boards in child repositories into
<WORKSPACE>/.orbit/board. It uses an existing Workspace Board when present. If
none exists, it stages a new Board and activates it only after validating the
merged graph. It previews without writing when --dry-run is given; otherwise,
the plan is applied. Source Boards remain intact as backups, and existing
Workspace configuration takes precedence over source configuration.

Task IDs stay unchanged unless they collide. Collisions are remapped with their
structured parent/dependency references and comments. Duplicate GitHub Issue
links stop the migration for explicit resolution. A saved plan allows a
partially-applied migration to resume with --resume. The Workspace Board is
validated before and after application; then run `orbit doctor` to check Board
discovery and member-repository state. The migration is one Board commit; use
`git -C <WORKSPACE>/.orbit/board revert <commit>` to roll it back. Keep the
source Boards until the doctor check succeeds. Source Task-ID prefixes that
differ from the Workspace prefix are recorded as accepted legacy prefixes, so
their IDs do not need to change.";

pub const WORKSPACE_MIGRATE_EXAMPLES: &str = "\
Examples:
  orbit workspace migrate --dry-run
  orbit workspace migrate
  orbit workspace migrate --resume
  orbit workspace migrate ~/src/acme --dry-run";

pub const UNLINK_LONG: &str = "\
Removes the shared-Board pointer and refreshes agent configuration to use this
repository's own Board. The shared Board is left untouched for the other
repositories that use it.

If this repository has no local Board, the link is still removed, previously
managed files are refreshed, and the command warns you to run `orbit init`.";

pub const UNLINK_EXAMPLES: &str = "\
Examples:
  # Stop sharing the product Board
  orbit unlink

  # Create a local Board afterwards
  orbit init";

pub const SLUG_LONG: &str = "\
A Board stores one slug as the literal `id.prefix` with a trailing dash, and every
Task ID starts with it. This command inspects that identity or migrates it.

`slug show` is read-only. `slug migrate` is the explicit escape hatch for a
populated Board: it keeps every random suffix, rewrites Task and SubTask IDs plus
`parent` and `depends_on`, renames comment files and their Orbit header, updates
`id.prefix`, and records one Board commit. The Board then returns to the simple
one-prefix invariant.

Migration refuses an inconsistent Board, prefix collisions, and live claims
unless --force. It never rewrites free-form task bodies, checkpoint text,
comment text, Git history, or external references. Use --dry-run first.";

pub const SLUG_EXAMPLES: &str = "\
Examples:
  # Inspect the current slug and prefix
  orbit slug show

  # Preview every ID that would change
  orbit slug migrate atlas --dry-run

  # Rewrite the whole Board to atlas-…
  orbit slug migrate atlas

  # Proceed while agents hold live claims, then tell them to re-read
  orbit slug migrate atlas --force";

pub const SLUG_SHOW_LONG: &str = "\
Prints the bare slug, the literal `id.prefix`, the number of Tasks and SubTasks,
and how many comment files the Board has. It does not validate or rewrite
anything, so it still reports the stored identity when the Board needs repair.

Use `orbit doctor` for health and `orbit slug migrate` to change the prefix.";

pub const SLUG_SHOW_EXAMPLES: &str = "\
Examples:
  orbit slug show
  orbit slug show --json
  orbit --dir ~/src/acme/board slug show";

pub const SLUG_MIGRATE_LONG: &str = "\
Rewrites every ID from the Board's current prefix to <SLUG>-. The random suffix is
preserved: orb-a7b2c9 becomes atlas-a7b2c9, and orb-a7b2c9.d4b3df becomes
atlas-a7b2c9.d4b3df. Parent and dependency references, comment filenames, the
Orbit-generated comment header, and id.prefix move with the IDs.

The operation is all-or-nothing: Orbit builds and validates the migrated Board in
a temporary Git worktree, commits once, then fast-forwards the real Board. It
refuses a Board that is not healthy and a target that would collide. Pending
heartbeat-only changes are committed first, so the migration commit is the single
rollback point. Live claims require --force because the holder still has the old
ID cached; affected holders are listed in the result.

Task bodies, checkpoint text, comment text, Git history, pull requests, and external links are not
rewritten. Existing links to old IDs break; agents must re-read the Board. On an
empty Board this is equivalent to setting the prefix.";

pub const SLUG_MIGRATE_EXAMPLES: &str = "\
Examples:
  # See the counts, references, and affected claims
  orbit slug migrate atlas --dry-run

  # Get every ID mapping as JSON without writing
  orbit slug migrate atlas --dry-run --json

  # Apply once the plan looks right
  orbit slug migrate atlas

  # Migrate a Board whose tasks are currently claimed
  orbit slug migrate atlas --force

  # Inspect the applied result without the human summary
  orbit slug migrate atlas --json";

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

Human output is an aligned table with ID, STATUS, P, TYPE, TITLE, LABELS, and
CLAIMED BY columns. Long titles are shortened to 60 characters. Root Tasks are
ordered by priority, then creation time and ID; each Parent Task is followed by
its SubTasks, also ordered by priority. --group renders one outlined table with
a banner for each Parent Task and its SubTasks beneath it; tasks with no parent
follow in a final No parent section. The table is for reading; use --json when
another program consumes the result, because it returns the full task objects in
data.tasks plus data.count.

--parent lists only the SubTasks of one Parent Task. --type is repeatable and
matches any listed work shape; --exclude-type hides any listed shape. --label is
repeatable and every label must match; --exclude-label is repeatable and hides a
task carrying any of the named labels. --repo NAME is shorthand for --label
repo:NAME. For work an agent can start, use `orbit ready` instead of filtering
this list.";

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

  # Group Parent Tasks and SubTasks in one outlined table
  orbit list --group

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
Unlike `list --group`, which shows the whole Board, and `list --parent`, which
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

pub const CHECKPOINT_LONG: &str = "\
Records or replaces the latest handoff checkpoint on a claimed task: the current
progress, the next useful action, and any blocker. The claim holder owns the
checkpoint; a stale lease still counts until another agent takes the claim with
`orbit claim --steal`. The checkpoint survives release, takeover, and reopen, so
the next claimant can read it from `orbit show`, `orbit ready`, or a claim result.

The first checkpoint requires --progress. Later updates may pass only the fields
that changed; omitted fields keep their previous value, and --clear-next or
--clear-blocker removes one. Progress, next, and blocker are free-form text.

Checkpoints are execution state: they do not affect readiness, claim eligibility,
or dependencies, and they are not synchronized to an external issue tracker. On a
done or cancelled task the latest checkpoint stays in the task file as archived
history, but the command refuses new checkpoints there; use comments for notes
after closing.";

pub const CHECKPOINT_EXAMPLES: &str = "\
Examples:
  # Record progress and the next action while holding the claim
  orbit checkpoint orb-a7b2c9 --by claude \
      --progress \"Parser and tests pass\" --next \"Wire the CLI\"

  # Add a blocker, then clear it once resolved
  orbit checkpoint orb-a7b2c9 --by claude --blocker \"Waiting on review\"
  orbit checkpoint orb-a7b2c9 --by claude --clear-blocker

  # Release the task with a checkpoint the next agent can pick up
  orbit release orb-a7b2c9 --by claude
  orbit ready --json";

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

--scope project (the default) writes repository-level files. --scope user writes
the same managed content under the user's home: Claude memory, settings, and
~/.claude.json; Cursor rules, skill, and mcp.json; Codex global AGENTS.md, skill,
and config.toml; OpenCode's global config and AGENTS.md; and the user-level
tracker guide. --print, --check, and --remove all operate on the selected scope.
Generated configs never record the Board path, so moving the Orbit folder does
not stale them.

Use init for first-time setup. setup is for adjusting one harness later, checking
drift in CI with --check (non-zero when missing or stale), or removing Orbit's
content with --remove.

By default MCP configs record the absolute path of the running binary, which
works for editors launched outside a shell. Pass --portable-mcp to write the
bare `orbit` command instead; the editor or agent that launches the MCP server
must then find Orbit on PATH.";

pub const SETUP_EXAMPLES: &str = "\
Examples:
  # Add the Claude hook and MCP entry
  orbit setup claude --hooks

  # Write machine-wide instruction and MCP files
  orbit setup all --scope user

  # Write the portable `orbit` command and rely on the editor's PATH
  orbit setup claude --portable-mcp

  # Check that every integration is installed and current
  orbit setup all --check

  # See what would change without writing
  orbit setup cursor --print

  # Remove only Orbit-managed content
  orbit setup all --remove";

pub const MCP_LONG: &str = "\
Runs the Model Context Protocol server over stdio. Editors start this process from
their MCP configuration, so it is rarely run by hand. It exposes 17 tools (ready,
list, get, create, update, claim, release, heartbeat, checkpoint, finish, link,
delete, label, comment, issue_refresh, issue_list, issue_inspect) and the orbit://tasks/{id} Markdown resource, reusing the
CLI's lock, atomic writes, and auto-commit. `create` accepts `parent` to make a
SubTask and `type` to set the work shape; `update` accepts `type`; `checkpoint`
records the claim holder's handoff. `list` and `ready` accept `parent`, `repo`,
`labels`, `exclude_labels`, `types`, and `exclude_types` to scope a feature,
repository, or work shape.

Use `orbit mcp` as the command in a client configuration. Pass --dir globally to
serve a specific Board.";

pub const MCP_EXAMPLES: &str = "\
Examples:
  # Run the server (normally launched by the editor)
  orbit mcp

  # Serve a shared Board explicitly
  orbit --dir ~/src/acme-board mcp";

pub const DOCTOR_LONG: &str = "\
Runs without requiring a Board and prints the discovery trace: explicit DIR,
link, nearest .orbit, main checkout, legacy layout, and the global fallback. It
reports LEGACY_LAYOUT, AMBIGUOUS_BOARD, ORPHAN_WORKTREE, STORAGE_MISMATCH, and
ABSOLUTE_BOARD_PATH findings. When a Board resolves, it also checks malformed
tasks, dangling dependencies, missing or nested parents, cycles, orphan or
corrupt comments, stale temp files, uncommitted changes, the checked-out branch,
and a Claude hook that cannot find the Orbit binary on PATH.

It exits 1 when it finds issues. --fix removes stale temp files and re-attaches an
orphan orbit worktree. --migrate moves a legacy Board to the Orbit folder and is
resumable; it never rewrites task bodies or comments.";

pub const DOCTOR_EXAMPLES: &str = "\
Examples:
  # Health check after a crash or merge
  orbit doctor

  # Clean up stale temp files and re-attach an orphan worktree
  orbit doctor --fix

  # Move a legacy hidden or workspace Board to .orbit
  orbit doctor --migrate

  # Inspect the discovery trace and reported issues as JSON
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

pub const GITHUB_LONG: &str = "\
Connects an Orbit Board to explicitly linked GitHub repositories. Each durable
Task can map one-to-one to a GitHub Issue identified by repository and issue
number. Sync is explicit, detects field-level conflicts, and never maps GitHub
assignees to Orbit claims.

Full mode (the default) uses organization-level Issue Fields for Orbit-owned
area and verification values; run setup after editing config.toml. Basic mode
supports personal repositories without setup, but cannot sync area or
verification. Triage in basic mode supports labels only. `orbit github connect`
checks the current origin and configures organization-owned repositories; for a
personal-account origin, it leaves the Board unchanged and reports the exact
`config.toml` entries to add or merge for basic mode, preserving other linked
repositories.

`orbit github setup` resolves existing fields by default. Creating missing
fields is opt-in with `--create-fields` and requires organization administrator
access. Run `orbit github status` for a read-only health report of authentication
and scopes, linked repositories, Issue Fields when applicable, pending changes,
conflicts, unsupported relationships, and the next recommended action. `orbit
doctor` also checks GitHub configuration when the provider is enabled. GitHub sync
requires Orbit 0.9.0 or newer; default release builds include the
`github-provider` capability. `github create` previews one or more Local Tasks by
default. Pass `--apply` to create Issues and persist their links; done and cancelled
Tasks also require `--include-terminal`. Workspace Boards infer the destination
from the included repository catalog when exactly one repository is available.";

pub const GITHUB_EXAMPLES: &str = "\
Examples:
  orbit github status
  orbit github status --json
  orbit github connect
  orbit github connect --json
  orbit github repo link acme/widget
  orbit github setup
  orbit github adopt orb-a7b2c9 https://github.com/acme/widget/issues/42
  orbit github adopt orb-a7b2c9 --repo acme/widget --number 42
  orbit github create orb-a7b2c9                 # preview only
  orbit github create orb-a7b2c9 --apply
  orbit github create orb-a7b2c9 orb-d4e5f6 --repo acme/widget --apply
  orbit github create orb-d4e5f6 --include-terminal --apply
  orbit github sync orb-a7b2c9
  orbit github sync orb-a7b2c9 --apply";

pub const WORKSPACE_LONG: &str = "\
Manage repositories that belong to the current Workspace.

`refresh` explicitly scans direct child directories, reads GitHub origins,
checks repository access, and updates the local Workspace catalog. `status` is
read-only and reports the last refresh; it does not poll GitHub or scan for new
folders. Full mode uses the organization in `[github].organization`.

Repository inclusion and exclusion are stored in the Workspace Board. Excluding
a repository does not delete its cached catalog data. Discovery never creates
Orbit Tasks or GitHub Issues.
";

pub const WORKSPACE_EXAMPLES: &str = "\
Examples:
  orbit workspace refresh
  orbit workspace status
  orbit workspace exclude acme/legacy
  orbit workspace include acme/service";

pub const WORKSPACE_ISSUES_LONG: &str = "\
Browse the Workspace's rebuildable Remote Issue catalog. Refresh first to
reconcile direct child repositories and cache each included repository's open
and closed Issues. Pull requests are excluded. Refresh performs GitHub reads
only, never creates Tasks, and retains the last snapshot for repositories that
fail. List filters cached summaries without fetching issue bodies or comments;
inspect fetches those details from GitHub on demand.

Remote Issues stay catalog-only and never appear in `orbit list` or `orbit ready`
until a separate import operation creates a Linked Task.";

pub const WORKSPACE_ISSUES_EXAMPLES: &str = "\
Examples:
  orbit workspace issues refresh
  orbit workspace issues list --repo acme/api --state open --label bug
  orbit workspace issues list --text authentication --unlinked
  orbit workspace issues inspect acme/api#42";

pub const GITHUB_STATUS_EXAMPLES: &str = "\
Examples:
  # Check GitHub configuration and the next recommended action
  orbit github status

  # Read the same diagnostic fields as JSON
  orbit github status --json";

pub const GITHUB_CONNECT_EXAMPLES: &str = "\
Examples:
  # Detect origin, authenticate with gh, and configure an organization-owned repo
  orbit github connect

  # Inspect the detected owner, fields, and next action as JSON
  orbit github connect --json";

pub const GITHUB_SETUP_EXAMPLES: &str = "\
Examples:
  orbit github setup --validate-only
  orbit github setup
  orbit github setup --create-fields";

pub const GITHUB_REPO_LINK_EXAMPLES: &str = "\
Examples:
  orbit github repo link acme/widget
  orbit github repo unlink acme/widget
  orbit github repo list";

pub const GITHUB_ADOPT_EXAMPLES: &str = "\
Examples:
  orbit github adopt orb-a7b2c9 https://github.com/acme/widget/issues/42
  orbit github adopt orb-a7b2c9 --repo acme/widget --number 42";

pub const GITHUB_CREATE_EXAMPLES: &str = "\
Examples:
  orbit github create orb-a7b2c9
  orbit github create orb-a7b2c9 --apply
  orbit github create orb-a7b2c9 orb-d4e5f6 --repo acme/widget --apply
  orbit github create orb-d4e5f6 --include-terminal --apply";

pub const GITHUB_SYNC_CMD_EXAMPLES: &str = "\
Examples:
  orbit github sync orb-a7b2c9
  orbit github sync orb-a7b2c9 --apply --direction pull";

pub const TRIAGE_LONG: &str = "\
Review GitHub-linked Tasks and apply approved metadata changes on the Orbit Board first.

Triage only touches Orbit-owned `area`, task-level `verification`, and canonical triage/category
labels. In GitHub basic mode, it supports labels only. It never rewrites GitHub-owned title, body,
or open/closed state, and it does not run verification commands. After approving a review or
`triage apply --approve`, publish changes with `orbit github sync`.

Agents draft a proposal JSON file and run `triage review` to see a field-by-field preview. With a
proposal file and both stdin and stdout connected to a terminal, review prompts for explicit
approval or cancellation; only `y` or `yes` applies changes. Without both terminals, review only
prints the preview and never waits for input. `--json` remains non-interactive, and
`--proposal-file -` reads JSON from stdin.

`orbit triage` rejects a proposal whose resulting labels contain more than one state role or more
than one category role, with `TRIAGE_INVALID_PROPOSAL`.

Review and apply change the Orbit Board only. GitHub is unchanged until an explicit
`orbit github sync` command. When evidence is insufficient, record `uncertainty` and leave
metadata unchanged.";

pub const TRIAGE_EXAMPLES: &str = "\
Examples:
  orbit triage context orb-a7b2c9 --json
  orbit triage review orb-a7b2c9 --proposal-file proposal.json
  orbit triage review orb-a7b2c9 --proposal-file proposal.json --json
  orbit triage review orb-a7b2c9 --proposal-file - --json < proposal.json
  orbit triage apply orb-a7b2c9 --proposal-file proposal.json --approve --json";

pub const TRIAGE_CONTEXT_EXAMPLES: &str = "\
Examples:
  orbit triage context orb-a7b2c9 --json";

pub const TRIAGE_REVIEW_EXAMPLES: &str = "\
Examples:
  orbit triage review orb-a7b2c9 --proposal-file proposal.json
  orbit triage review orb-a7b2c9 --proposal-file proposal.json --json
  orbit triage review orb-a7b2c9 --proposal-file - --json < proposal.json";

pub const TRIAGE_APPLY_EXAMPLES: &str = "\
Examples:
  orbit triage apply orb-a7b2c9 --proposal-file proposal.json --approve --json
  orbit github sync orb-a7b2c9 --direction push --apply";
