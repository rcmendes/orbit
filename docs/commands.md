# Command guide

Orbit has one command per operation. Every command explains itself:

```bash
orbit <command> --help    # long description, options, and examples
orbit <command> -h        # compact summary
```

This guide adds the part help cannot: **when** each command is the right tool.
Board commands print human-readable text by default; add `--json` to get exactly
one JSON object on stdout. Human output is for terminals, not parsers: `orbit
list` renders an aligned table, while `ready`, `tree`, and `prime` use compact
summary lines. The table's columns, ordering, and title truncation are
presentation only; `--json` is the stable contract.

## Global options

| Option | Meaning |
| --- | --- |
| `--json` | Print exactly one JSON object on stdout, including errors. |
| `--dir <PATH>` | Use a specific Board for this command. |
| `ORBIT_DIR` | Environment equivalent of `--dir`. |

Exit codes are `0` for success, `1` for a domain failure (for example
`DEPENDENCY_BLOCKED` or a failing `doctor`), and `2` for a usage error.

Task commands resolve the Board in this order: `--dir`, then `ORBIT_DIR`, then the
repository link, then the repository's own Board. `orbit init` creates a Board
instead of discovering one. See [issue-tracker.md](./agents/issue-tracker.md) for
the multi-repository model.

## Which command do I need?

| Goal | Command |
| --- | --- |
| Set up a repository or workspace | `orbit init`, `orbit workspace`, `orbit link`, `orbit unlink` |
| File a new ticket | `orbit add` |
| Break work into blockers | `orbit add --depends-on`, `orbit depend`, `orbit undepend` |
| Triage incoming work | `orbit label add/remove/list` |
| Find work for an agent | `orbit ready` |
| Take a specific ticket | `orbit claim` |
| Keep a claim alive | `orbit heartbeat` |
| Hand work back | `orbit release` |
| Finish or abandon work | `orbit done`, `orbit cancel` |
| Undo a close or remove a mistake | `orbit reopen`, `orbit delete` |
| Add context to a ticket | `orbit comment add`, `orbit update` |
| Read the board | `orbit list`, `orbit show`, `orbit tree`, `orbit prime` |
| Collect or archive the board | `orbit export` |
| Wire an editor or agent | `orbit setup`, `orbit mcp` |
| Recover or maintain the board | `orbit doctor`, `orbit sync` |

## Set up a Board

### `orbit init` — start using Orbit in a repository

Creates the Board (a hidden Git worktree on the `orbit` branch) and wires the
coding-agent harnesses it detects. Re-running is safe: managed files are
reconciled, not duplicated.

**Good fit:** the first time you use Orbit in a repository, after installing a new
editor integration, or when turning an existing repository into the home of a
shared workspace.

```bash
orbit init                    # detect harnesses and ask which to wire
orbit init --agents none      # Board only
orbit init --print            # preview every file that would change
orbit init --board ~/src/acme-board   # create and link a shared Board
orbit init --slug teramigo    # Task IDs become teramigo-…
```

Task IDs use the Board's slug as their prefix. In a repository, `init` adopts
the main checkout folder name (`teramigo` → `teramigo-`); `--board` defaults to
`orb`. Pass `--slug` to choose it explicitly. Re-running `init` preserves the
stored prefix; an empty Board can change it, while a populated Board returns
`SLUG_LOCKED`. `--print` reports the same prefix without writing. The JSON and
human output carry `prefix`.

### `orbit workspace` — create a shared workspace

Creates a workspace directory with a shared Board at `<folder>/board`. The Board
is independent of every member repository: it is its own Git repository on the
`orbit` branch.

**Good fit:** a feature that spans several repositories. Create the workspace
once, then join each repository with `orbit link`.

```bash
orbit workspace ~/src/acme
orbit workspace ~/src/acme --slug acme   # explicit Task-ID prefix
orbit link ~/src/acme        # from each repository root
```

The Task-ID prefix defaults to the workspace folder name (`acme` → `acme-`).
Pass `--slug` to choose it explicitly. Re-running preserves the prefix; an empty
Board can change it, while a populated Board returns `SLUG_LOCKED`. The JSON and
human output carry `prefix`.

The command only creates. It does not clone or link repositories, and it does
not write agent configuration. A workspace must live outside every Git working
tree; `WORKSPACE_INSIDE_REPO` means the chosen folder is inside one. Re-running
is safe: an existing Board is reported, not overwritten.

### `orbit link` — reuse a Board from another repository

Points this repository at an existing Board or workspace, including from all of
its worktrees, and refreshes local agent configuration. A workspace root
resolves to its `board/` subdirectory, so the same command works for both.

**Good fit:** a workspace with several related repositories (API, web app,
agents) where one product Board should hold all of the work. Create it with
`orbit workspace`, then `link` in each repository.

```bash
orbit link ~/src/acme            # join a workspace
orbit link ~/src/acme-board      # link a Board directly
orbit link ~/src/acme --no-refresh   # leave agent configs alone
```

Linking does not delete a local Board. If one exists it is shadowed and `link`
warns about it.

### `orbit unlink` — go back to a local Board

Removes the shared-Board pointer and refreshes agent configuration to the
repository's own Board. The shared Board is untouched.

**Good fit:** a repository is leaving the workspace, or you want to experiment
against an isolated Board.

```bash
orbit unlink
orbit init        # create the local Board if it does not exist yet
```

### `orbit setup` — adjust one harness

Installs, checks, or removes a harness integration without touching the Board.

**Good fit:** repairing one editor after an upgrade, checking in CI that
integration files are current, or removing Orbit's content before uninstalling.

```bash
orbit setup claude --hooks     # instructions + MCP + SessionStart hook
orbit setup all --check        # installed / missing / stale
orbit setup cursor --print     # preview
orbit setup all --remove       # remove only Orbit-managed content
```

An existing `docs/agents/issue-tracker.md` without Orbit's managed marker is
treated as a hand-written guide: setup reports it as `skipped` and leaves it
unchanged, `--check` treats the integration as satisfied, and agent refresh
still recognizes the file. Tracker files with the marker update and remove
normally.

### `orbit mcp` — expose the Board to an MCP client

Runs the Model Context Protocol server over stdio with 13 tools and the
`orbit://tasks/{id}` resource. Editors launch it from their MCP configuration.
`create` accepts `parent` and `type`; `update` accepts `type`; `list` and `ready`
accept `parent`, `repo`, `labels`, `exclude_labels`, `types`, and `exclude_types`
to scope a feature or repository.

**Good fit:** an editor or agent that speaks MCP should read and update the Board
directly instead of shelling out to the CLI.

```bash
orbit mcp                          # normally launched by the editor
orbit --dir ~/src/acme-board mcp   # serve a shared Board
```

## Create and shape work

### `orbit add` — file a ticket

Creates a task in `todo` and prints its ID, which you need for dependents,
labels, and comments.

**Good fit:** a bug report, a feature, a follow-up from a review, or a ticket
that a planning skill is publishing to the tracker. Put context and acceptance
criteria in the body; use labels for triage state.

```bash
# A high-priority bug with a Markdown body
orbit add "Login rejects valid tokens" -p 1 --label bug --body-file ticket.md

# Work that must wait for a blocker
orbit add "Publish the release" -p 1 --depends-on orb-a7b2c9

# A ticket waiting on something outside the Board
orbit add "Upgrade the parser" --waiting-on "upstream 2.4" --body-file -

# A top-level epic or spec
orbit add "Spec: SSO login" --type epic --body-file spec.md

# Publish several tickets, blockers first; each add prints the new ID
orbit add "Design the schema" -p 0 --label ready-for-agent
# created orb-f14c02  P0  Design the schema
orbit add "Ship the API" -p 1 --depends-on orb-f14c02 --label ready-for-agent
# created orb-a7b2c9  P1  Ship the API

# Break a feature into a SubTask
orbit add "API: add the endpoint" --parent orb-a7b2c9 --label repo:api
# created orb-a7b2c9.b3c4d5  P2  API: add the endpoint  (subtask of orb-a7b2c9)
```

`--parent` creates a SubTask under an existing Parent Task. The SubTask ID is the
parent ID, a dot, and a fresh suffix (`orb-a7b2c9.b3c4d5`); the `parent` field
stores the same parent. SubTasks are one level deep and can still have real
dependencies. The Parent Task becomes Ready only when every SubTask is done or
cancelled.

`--type` sets the work shape: `task` (default), `epic`, `spike`, `chore`, or
`decision`. Type is metadata and does not affect readiness. `epic` is refused on a
SubTask. `bug` and `enhancement` remain labels, not Types.

A spec or epic is a top-level Task with `type: epic`; its tickets are SubTasks.
This is also the compatibility mapping for Beads and Matt Pocock's skills:
**Issue** = Task, **Sub-issue** = SubTask, **Epic** = top-level Task with
`type: epic`. Put the spec in the parent body, publish tickets with `--parent`,
and use `orbit ready --parent <id>` as the frontier. There is no deeper nesting;
flatten a ticket's own breakdown into sibling tickets with `--depends-on`. Never
use `done --force` or `cancel --force` on a spec parent with open tickets: it
cancels them. See [ADR 0009](./adr/0009-keep-task-vocabulary.md) and
[ADR 0010](./adr/0010-structural-type-field.md).

### `orbit update` — edit a ticket in place

Changes the title, body, priority, type, or external blocker without touching
status, claims, labels, or dependencies. `--type epic` is refused on a SubTask.

**Good fit:** scope changed after triage, a spec needs refinement, or an external
blocker was resolved.

```bash
orbit update orb-a7b2c9 --priority 0
orbit update orb-a7b2c9 --type spike
orbit update orb-a7b2c9 --body-file refined-spec.md
orbit update orb-a7b2c9 --clear-waiting
```

### `orbit depend` and `orbit undepend` — connect work

`depend <id> <dep-id>` makes the first task wait until the second is done or
cancelled. Cycles, self-dependencies, adding a dependency to a terminal task, and
parent/child edges in either direction are rejected. `undepend` removes the edge
and is idempotent.

**Good fit:** a blocker is discovered after both tickets exist, or a dependency
was wrong and must be removed. Use `orbit add --parent` for parentage; a
dependency blocks the child, while a parent blocks the feature.

```bash
orbit depend orb-a7b2c9 orb-f14c02   # orb-a7b2c9 waits for orb-f14c02
orbit undepend orb-a7b2c9 orb-f14c02
```

Prefer `orbit add --depends-on` when the edge is known at creation time.

### `orbit label` — triage state and category

Labels are free-form lowercase slugs. `needs-triage`, `needs-info`, and
`ready-for-human` are hold labels: while present, the task is not ready and
cannot be claimed. Adding and removing labels is idempotent.

**Good fit:** moving a ticket through triage, marking it ready for an agent, or
recording that it is a bug or enhancement.

```bash
orbit label add orb-a7b2c9 bug needs-triage
orbit label remove orb-a7b2c9 needs-triage
orbit label add orb-a7b2c9 ready-for-agent
orbit label list                       # labels in use, with counts
orbit list --label needs-triage        # everything awaiting triage
```

### `orbit comment` — keep a per-task thread

Comments are append-only, with an author and timestamp, and are allowed on closed
tasks. Corrections are new comments; nothing is edited or deleted.

**Good fit:** triage notes, handoff context, links to logs, or a record of why a
decision was made. Put durable requirements in the body instead.

```bash
orbit comment add orb-a7b2c9 --by claude --body "Reproduced on macOS."
cargo test 2>&1 | orbit comment add orb-a7b2c9 --by claude --body-file -
orbit comment list orb-a7b2c9
```

## Find work

### `orbit list` — inspect the board

Lists tasks with combinable filters. Done and cancelled tasks are hidden by
default; pass `--all`, or name a terminal status with `--status`, to include
them. Human output is an aligned table with `ID`, `STATUS`, `P`, `TYPE`,
`TITLE`, `LABELS`, and `CLAIMED BY` columns; titles longer than 60 characters
are shortened with an ellipsis. Use `--json` when a script or agent consumes the
result.

**Good fit:** a human or agent wants an overview, a triage queue, one agent's
in-progress work, or the full history.

```bash
orbit list
orbit list --label needs-triage
orbit list --label repo:api --label ready-for-agent   # every label must match
orbit list --repo api                               # shorthand for repo:api
orbit list --status in_progress --claimed-by claude
orbit list --status done            # closed work, without --all
orbit list --unlabeled
orbit list --parent orb-a7b2c9      # only this feature's SubTasks
orbit list --type epic              # only epics and specs
orbit list --tree                   # parents and SubTasks together
orbit list --exclude-type epic      # hide spec/epic containers
orbit list --all --json
```

Human output looks like this:

```text
ID                 STATUS  P   TYPE  TITLE                                   LABELS                       CLAIMED BY
orb-a7b2c9         todo    P1  epic  Spec: SSO login across api, web, infra  ready-for-agent
orb-a7b2c9.z21c2w  todo    P0  task    infra: register OIDC client           ready-for-agent, repo:infra
orb-a7b2c9.hxdmqt  todo    P1  task    api: OIDC callback                    ready-for-agent, repo:api
```

The table is intentionally not a machine interface. Column widths grow with the
data, labels are joined with commas, a claim holder appears in the last column,
and long titles are truncated. Parse `--json` instead.

`--type` is repeatable and matches any listed work shape; `--exclude-type` hides
any listed shape. `--label` is repeatable and every label must match;
`--exclude-label` is repeatable and hides a task carrying any of the named labels.
`--repo NAME` is shorthand for `--label repo:NAME`. `--tree` keeps each Parent
Task next to its SubTasks and indents the child title by two spaces in the human
table; JSON always carries `kind`, `type`, `parent`, `subtasks`, and
`open_subtasks` with the full title. A Parent Task with open SubTasks is not
Ready, so `list` may show it even though `ready` does not.

For work that can start now, use `orbit ready` instead: `list` shows all open
work, including blocked tasks.

### `orbit ready` — the agent queue

Shows tasks that are `todo`, unclaimed, not waiting on an external blocker, carry
no hold label, and have all dependencies done or cancelled. A Parent Task is not
Ready while any SubTask is open. Results are ordered by priority, then creation
time.

With `--claim`, the first task is claimed atomically in the same command and
moves to `in_progress` with a lease. When nothing is ready, the command succeeds
with `data: null` and `meta.reason: "NO_READY_TASKS"`. `--parent` scopes the
queue to one feature's SubTasks.

**Good fit:** an agent asks what to do next, a supervisor polls for idle work, or
a worker wants to take exactly one task without racing another agent.

```bash
orbit ready --limit 5
orbit ready --claim --by claude --json
orbit ready --parent orb-a7b2c9 --claim --by claude   # one feature only
orbit ready --repo api --claim --by claude            # one repository only
orbit ready --exclude-type epic --claim --by claude   # skip spec containers
orbit ready --json              # NO_READY_TASKS means wait and retry
```

`--parent`, `--repo`, repeatable `--label`, `--type`, and `--exclude-label`
combine with AND. `--type` matches any listed shape; `--exclude-type` hides any
listed shape. `--exclude-label` remains for legacy label-only boards.

### `orbit show` — load one ticket

Prints the full task: body, labels, dependencies, kind, type, parent, SubTasks,
dependents, claim state, and comments. `--json` is the form an agent should load
before starting work.

**Good fit:** before implementing a ticket, when checking why a task is blocked,
or when reading a handoff comment.

```bash
orbit show orb-a7b2c9
orbit show orb-a7b2c9 --json
```

### `orbit tree` — inspect one epic or spec

Shows one Parent Task, its direct SubTasks, and their blocking edges. Unlike
`list --tree` (the whole Board) and `list --parent` (children only), `tree`
always includes the root and its family, with resolved/total progress on the
root.

**Good fit:** inspecting a spec, epic, or feature before publishing tickets,
checking the frontier, or seeing why one ticket is blocked.

```bash
orbit tree orb-a7b2c9
orbit tree orb-a7b2c9 --json
```

`tree` expects a Parent Task; a SubTask returns `INVALID_ARGUMENT`. `--json`
returns `data.root`, `data.tasks` with the root first, and `data.count`.

### `orbit prime` — starting context for a session

Prints the agent workflow instructions, board counts, top ready tasks, Parent
Tasks whose SubTasks are all resolved (close candidates), and stale claims. The
Claude Code SessionStart hook runs `orbit prime --hook-json` automatically.

**Good fit:** the first command of an agent session, a status prompt, or a quick
health view of what is ready and what was abandoned.

```bash
orbit prime --by claude
orbit prime --by claude --repo api   # scope counts and the queue to one repository
orbit prime --json
```

`--repo` and repeatable `--label` scope the counts, ready tasks, close
candidates, and stale claims to a repository or label set. A close candidate is
a non-terminal Parent Task with at least one SubTask and no open SubTasks; Orbit
never closes it automatically.

### `orbit export` — snapshot the board

Emits a deterministic JSON snapshot of every Task and SubTask, including terminal
work, with type, labels, dependency and parent edges, parent progress fields,
derived readiness, and comments.
The snapshot is ordered by task ID and contains no timestamps or machine-local
paths.

**Good fit:** feeding a knowledge base, archiving a board, or diffing two
snapshots. The collector should pin `schema_version`.

```bash
orbit export --json > board.json
orbit export --json | jq '.data.tasks[] | select(.kind == "subtask")'
orbit export --json | jq '.data.board'
```

The snapshot is the standard envelope with `data.schema_version`, `data.board`,
and `data.tasks`; each task has the same shape as `show --json`, plus comments.

## Claim and finish work

### `orbit claim` — take a specific task

Claims one task by ID instead of taking the top of the ready queue. The task must
be active and unclaimed (a stale claim needs `--steal`), not waiting on an
external blocker or an unfinished dependency, free of hold labels, and, if it is
a Parent Task, have no open SubTasks.

**Good fit:** a human assigns a ticket, an agent resumes known work, or another
agent's lease expired and the task must be recovered.

```bash
orbit claim orb-a7b2c9 --by claude
orbit claim orb-a7b2c9 --by claude --steal   # take over a live or stale claim
```

`--steal` overrides the holder's lease, not the readiness rules: hold labels and
unfinished dependencies still block the claim.

### `orbit heartbeat` — keep a claim alive

Extends the lease from now. Only the holder can heartbeat. Heartbeats are
lease-only writes and are not committed until the next mutation or `orbit sync`.

**Good fit:** a long build or test run would outlive the default 30-minute lease,
or a supervisor wants to confirm an agent is still alive.

```bash
orbit heartbeat orb-a7b2c9 --by claude
```

### `orbit release` — hand work back

Clears the claim and returns the task to `todo`. The holder can release; `--force`
releases someone else's claim. Terminal tasks cannot be released.

**Good fit:** work is deliberately handed back, a human takes over, or a claim was
taken by mistake and the queue should see the task again.

```bash
orbit release orb-a7b2c9 --by claude
orbit release orb-a7b2c9 --force
```

### `orbit done` — close completed work

Moves the task to `done`. Unfinished dependencies always block completion. A
Parent Task with open SubTasks is refused; `--force` cancels those SubTasks and
then closes the parent, and also overrides a competing live claim. `--comment`
records the completion note in the same transaction as the status change.

**Good fit:** the work is implemented and verified. This is the normal end of the
agent loop.

```bash
orbit done orb-a7b2c9 --by claude --comment "Implemented and verified."
orbit done orb-a7b2c9 --by claude --comment-file notes.md
orbit done orb-a7b2c9 --force --comment "Deferred the remaining pieces"
```

### `orbit cancel` — abandon work

Cancels a task without completing it. Cancelled tasks count as resolved, so
dependents can become ready. A live claim from another agent blocks cancellation
unless `--force` is passed. A Parent Task with open SubTasks is refused; `--force`
cancels those SubTasks too.

**Good fit:** the work is no longer needed, is a duplicate, or is a wontfix. Add
`--comment` to record the reason.

```bash
orbit cancel orb-a7b2c9 --by claude --comment "Superseded by orb-f14c02"
orbit cancel orb-a7b2c9 --force
```

### `orbit reopen` — undo a close

Returns a done or cancelled task to `todo`. Reopening a SubTask of a done Parent
Task reopens the parent too, so the feature is not left done with reopened work.
A SubTask under a cancelled parent is refused until the parent is reopened. It
also refuses when a dependent is already done, because that would leave completed
work with an unfinished dependency; `--force` allows it and `doctor` reports the
historical warning.

**Good fit:** a release regressed, a ticket was closed by mistake, or a cancelled
idea is back in scope.

```bash
orbit reopen orb-a7b2c9
orbit reopen orb-a7b2c9 --force
```

### `orbit delete` — remove a mistake

Permanently deletes a task and its comment thread. It refuses when dependents
exist, when the task has SubTasks, or when another agent holds a live claim;
`--force` also removes the ID from each dependent's dependency list and deletes
the whole SubTask subtree.

**Good fit:** duplicates or tickets created in the wrong Board. Prefer `cancel`
when the work should remain in history.

```bash
orbit delete orb-a7b2c9
orbit delete orb-a7b2c9 --force
```

## Maintain the board

### `orbit doctor` — health check

Reports malformed tasks, dangling dependencies, missing or nested parents,
parent/ID mismatches, cycles, orphan or corrupt comments, stale temp files,
uncommitted changes, a wrong checked-out branch, and a Claude hook that cannot
find the running Orbit binary on `PATH`. Exits `1` when unhealthy. `--fix` only
removes stale temp files.

**Good fit:** after a crash, a merge, a board branch switch, or a suspicious
`NOT_FOUND`/`CORRUPT_TASK` error; also as a CI check.

```bash
orbit doctor
orbit doctor --fix
orbit doctor --json
```

### `orbit sync` — persist lease-only changes

Commits pending board changes, mostly heartbeats, to the board branch. Nothing is
pushed.

**Good fit:** a long session only heartbeated, or `doctor` reports uncommitted
changes.

```bash
orbit sync
```

## Scenarios

### A solo maintainer triages a new report

```bash
orbit add "Login rejects valid tokens" -p 1 --label bug --label needs-triage --body-file report.md
# ... investigate, then decide:
orbit label add orb-a7b2c9 ready-for-agent
orbit label remove orb-a7b2c9 needs-triage
orbit comment add orb-a7b2c9 --by rodrigo --body "Confirmed; add a regression test."
```

### An agent takes one task and finishes it

```bash
orbit ready --claim --by claude --json      # receives one task plus lease
orbit show orb-a7b2c9 --json                # load body and comments
# ... implement ...
orbit heartbeat orb-a7b2c9 --by claude      # during a long verification run
orbit done orb-a7b2c9 --by claude --comment "Implemented and verified; tests added."
```

### Two agents work in parallel without collisions

```bash
# worker A
orbit ready --claim --by agent-a

# worker B, at the same time
orbit ready --claim --by agent-b
```

Each claim is atomic; when several tasks are ready, both workers get one. If two
workers target the same task, the loser gets `ALREADY_CLAIMED` with the holder and
lease.

### An agent crashes mid-task

```bash
orbit prime --json                     # lists stale claims
orbit claim orb-a7b2c9 --by agent-b --steal
orbit heartbeat orb-a7b2c9 --by agent-b
```

Use `release --force` instead when the task should go back to the queue unclaimed.

### A release train with ordered work

```bash
orbit add "Design the schema" -p 0 --label ready-for-agent
# created orb-f14c02  P0  Design the schema
orbit add "Ship the API" -p 1 --depends-on orb-f14c02 --label ready-for-agent
# created orb-a7b2c9  P1  Ship the API
orbit add "Publish the SDK" -p 1 --depends-on orb-a7b2c9 --label ready-for-agent
```

`orbit ready` returns only the first task. Completing it makes the next one ready
in the same order.

### A feature split into SubTasks

```bash
# the feature
orbit add "SSO login across api, web, infra" --body-file feature.md
# created orb-a7b2c9  P2  SSO login across api, web, infra

# one SubTask per repository
orbit add "api: OIDC callback" --parent orb-a7b2c9 --label repo:api
orbit add "web: login redirect" --parent orb-a7b2c9 --label repo:web
orbit add "infra: register client" --parent orb-a7b2c9 --label repo:infra

# an agent in the api repository picks its piece
orbit list --parent orb-a7b2c9 --label repo:api --json
orbit claim orb-a7b2c9.b3c4d5 --by api-agent

# when every piece is done or cancelled, close the feature explicitly
orbit done orb-a7b2c9 --by coordinator --comment "SSO shipped"
```

Each SubTask is claimed and completed independently. The Parent Task only becomes
Ready when the last one is done or cancelled. `orbit done --force` on the parent
cancels the remaining SubTasks instead of leaving them open.

### A Board with its own Task-ID prefix

```bash
orbit init --slug teramigo
orbit add "First task" --json
# teramigo-32fdfd
```

The slug is adopted once at creation. Existing `orb-` Boards keep working without
migration. To change the prefix, the Board must have no tasks; otherwise Orbit
returns `SLUG_LOCKED`.

### A workspace of several repositories

```bash
# create the workspace and its shared Board
orbit workspace ~/src/acme

# in each related repository
orbit link ~/src/acme
```

All repositories and their worktrees share one dependency graph. The Board lives
at `~/src/acme/board`; use `orbit --dir ~/src/acme/board list` for a one-off
command against it regardless of the current repository.

### Collect the board into a knowledge base

```bash
orbit export --json > board.json
```

The snapshot includes every task, including done and cancelled work, with labels,
dependency and parent edges, comments, and derived readiness. It is ordered by
task ID and contains no timestamp, so the same board state produces the same
data. Pin `schema_version` in the collector.

### Move work from triage to an agent

```bash
orbit list --label needs-triage --json
orbit label add orb-a7b2c9 ready-for-agent
orbit label remove orb-a7b2c9 needs-triage
orbit ready --claim --by claude
```

Hold labels keep untriaged work out of the agent queue, so the second and third
commands are what actually publish the ticket.

### Recover after a crash or a bad merge

```bash
orbit doctor
orbit doctor --fix      # remove stale temp files
orbit sync              # commit anything left uncommitted
```

`doctor` never rewrites tasks or comments, so it is always safe to run.

## Common outcomes and errors

| Error | What to do |
| --- | --- |
| `NO_READY_TASKS` | Not an error: nothing is startable. Wait, triage, or remove a hold label. |
| `ALREADY_CLAIMED` | Another agent holds a live lease. Pick other work, or `--steal` to take it over. |
| `CLAIM_STALE` | The holder's lease expired. Recover with `orbit claim <id> --steal`. |
| `DEPENDENCY_BLOCKED` | Finish or cancel the IDs in `error.details.blocking`. |
| `WAITING_EXTERNAL` | Resolve the blocker, then `orbit update <id> --clear-waiting`. |
| `LABEL_BLOCKED` | Remove the hold label (`needs-triage`, `needs-info`, `ready-for-human`). |
| `NOT_CLAIM_HOLDER` | Use the holder's `--by`, or `--force` where the command allows it. |
| `SUBTASKS_OPEN` | Finish or cancel the IDs in `error.details.subtasks`, or pass `--force` to `done` or `cancel`. |
| `SUBTASKS_EXIST` | The Parent Task still has SubTasks. Use `delete --force` to remove the whole subtree, or delete the SubTasks first. |
| `PARENT_MISMATCH` | The SubTask ID and its `parent` field disagree. Rename the file or fix the frontmatter so the ID encodes the same parent. |
| `INVALID_SLUG` | `--slug` must be a bare 1–20 character lowercase word with alphanumeric ends and no `--`; use the normalized suggestion in the error. |
| `SLUG_LOCKED` | The Board has tasks. Create a new Board or keep the current prefix. |
| `PREFIX_MISMATCH` | `id.prefix` and the task filenames disagree. Restore the configured prefix or create a new Board. |
| `DANGLING_PARENT` | Run `doctor`; the parent ID is missing. Recreate the parent or fix the SubTask's frontmatter. |
| `NESTED_SUBTASK` | A SubTask points to itself or to another SubTask. SubTasks are one level deep. |
| `INVALID_TYPE` | `epic` was used on a SubTask. Epic is top-level only; use `task`, `spike`, `chore`, or `decision` for a child. |
| `WORKSPACE_INSIDE_REPO` | Choose a workspace folder outside every Git working tree; the Board must not live inside a member repository. |
| `LOCKED` | Retryable: another writer holds the lock. Retry after a short backoff. |

## See also

- [README](../README.md) for the overview, quick start, and workspace models.
- [Architecture](./ARCHITECTURE.md) for the component map, state machine, command flow, and sequence diagrams.
- [Specification](./SPEC.md) for the canonical JSON contract and transition rules.
- [Glossary](./GLOSSARY.md) for the canonical vocabulary.
