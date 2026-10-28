# Orbit

Orbit is a Git-native issue tracker and local work queue for coding agents. Its unit of work is a Task stored as a human-readable file.

## Language

**Task**:
A single unit of work that one agent can claim and complete. A Task may stand alone or group SubTasks as a Parent Task.
_Avoid_: Issue, ticket, bead, bean

**Slug**:
The Board's short name that prefixes every Task ID, such as `teramigo` in `teramigo-32fdfd`. A Board has exactly one Slug at a time; `orbit slug show` reports it and `orbit slug migrate` rewrites every ID when the Board's identity changes.
_Avoid_: Project name, namespace

**Slug migration**:
An explicit, one-commit rewrite of every Task and SubTask ID from the Board's current prefix to a new Slug. It preserves random suffixes and rewrites structured `parent` and `depends_on` references, comment filenames, and Orbit comment headers. It never rewrites task bodies, checkpoint text, comment text, Git history, or external links.
_Avoid_: Multi-prefix mode, alias table

**SubTask**:
A Task that belongs to exactly one Parent Task. A SubTask is claimable and completable in its own right; it is not a checklist item.
_Avoid_: Child task, checklist item, step

**Parent Task**:
A Task that groups SubTasks and whose completion depends on them.
_Avoid_: Epic (as a synonym for every Parent Task), story

**Close Candidate**:
A non-terminal Parent Task with at least one SubTask and no open SubTasks.
`orbit prime` reports Close Candidates; Orbit never closes one automatically.
_Avoid_: Completed parent, auto-close

**Kind**:
The structural position of a Task: `task` or `subtask`. Kind is derived from `parent`; it is not the same as Type.
_Avoid_: Category, issue type

**Type**:
The work shape of a Task: `task`, `epic`, `spike`, `chore`, or `decision`. Type is metadata and does not affect readiness. `epic` is allowed only on a top-level Task.
_Avoid_: Kind, category, label

**Status**:
The stored lifecycle position of a Task: `todo`, `in_progress`, `done`, or `cancelled`.
_Avoid_: State, phase

**Priority**:
The stored ordering weight of a Task, from P0 (highest) to P3 (lowest).
_Avoid_: Rank, severity

**Label**:
A short lowercase string attached to a Task for filtering or workflow state. A Label does not affect readiness unless it is a Hold Label.
_Avoid_: Tag, category

**Hold Label**:
A Label configured in `non_ready_labels` that makes a Task not Ready while present. The defaults are `needs-triage`, `needs-info`, and `ready-for-human`.
_Avoid_: Blocked label, pause label

**Comment**:
An append-only, authored, timestamped note attached to a Task.
_Avoid_: Note, message

**Dependency**:
Another Task that must be `done` or `cancelled` before this Task can become Ready.
_Avoid_: Blocker, prerequisite, parent

**Ready**:
A condition of a Task that is `todo`, has no Claim (including no Stale Claim), is not Waiting, carries no Hold Label, and whose every Dependency is `done` or `cancelled`. Ready is derived, never stored.
_Avoid_: Unblocked, available, actionable

**Blocked**:
A condition of a Task with at least one Dependency that is neither `done` nor `cancelled`. Blocked is derived, never stored.
_Avoid_: Blocked status, waiting

**Waiting**:
A condition of a Task that cannot proceed because of something outside the dependency graph, such as a human decision, a credential, or a third party. Waiting is recorded separately from Status.
_Avoid_: Blocked, on hold

**Done**:
The terminal Status for a Task whose work is complete. Counts as resolved for dependent Tasks.
_Avoid_: Closed, complete

**Cancelled**:
The terminal Status for a Task that was abandoned without completing its work. Counts as resolved for dependent Tasks.
_Avoid_: Deleted, closed, wontfix

**Claim**:
Exclusive, time-bounded responsibility for a Task, held by one Agent.
_Avoid_: Assignment, lock, reservation

**Lease**:
The time window during which a Claim is valid.
_Avoid_: Timeout, TTL

**Checkpoint**:
The claim holder's latest handoff note on a Task: current progress, the next useful action, and any blocker. A Checkpoint survives release, takeover, and reopen, never affects readiness or Claim eligibility, and stays as archived history after the Task is done or cancelled.
_Avoid_: Status update, progress log, assignment note

**Stale**:
A condition of a Task whose Claim's Lease has expired but the Claim has not been released. A Stale Task is not Ready; it can be released or stolen.
_Avoid_: Expired claim, abandoned

**Agent**:
A process that reads and mutates Tasks on behalf of a developer.
_Avoid_: Worker, bot, client

**Workspace**:
The scope an Agent sees: one Board and every Repository linked to it.
_Avoid_: Project, database

**Board**:
The versioned store of Tasks and configuration for a Workspace. A Board lives in one Orbit folder and has one `orbit` branch.
_Avoid_: Database, store, project

**Orbit folder** (DIR):
The single directory delegated to Orbit that holds `config.toml`, `tasks/`, and `comments/`. The default is `<project>/.orbit`. Inside a Git working tree the folder is a worktree of that repository on the `orbit` branch; outside one it is a standalone Git repository. All commands take DIR the same way.
_Avoid_: Board root, workspace, data directory, hidden worktree

**Storage mode**:
How an Orbit folder is backed by Git: `worktree` when DIR is inside a working tree and shared by every worktree of the repository, or `repository` when DIR is its own Git repository. The mode is recorded in `config.toml` and changes only Git plumbing, never the Board contract.
_Avoid_: Layout, backend, external board

**Global Board**:
The opt-in Board at `~/.orbit`, created only by `orbit init --global`. It is the last discovery fallback and is never created implicitly by a command.
_Avoid_: Default board, user board, central board

**Legacy layout**:
A Board from before the Orbit folder: the hidden worktree at `<git-common-dir>/orbit/board`, a shared `<workspace>/board`, or an old `--board` path. It keeps resolving with a `LEGACY_LAYOUT` warning until `orbit doctor --migrate` moves it.
_Avoid_: Old mode, compatibility board

**Linked Repository**:
A Repository whose Agents use a Board that does not live inside it.
_Avoid_: Submodule, project, workspace

## Compatibility vocabulary

Beads, GitHub, Linear, and Matt Pocock's skills call the unit an **Issue**, a
child a **Sub-issue**, and a grouping issue an **Epic**. These are translations,
not stored types:

- **Issue** = Task
- **Sub-issue** = SubTask, created with `--parent`
- **Epic** or **spec** = top-level Task with `type: epic`

The `epic` label is legacy. Orbit does not infer `type: epic` from it, and
`--exclude-label epic` still works for boards created before the Type field.

See [ADR 0009](./adr/0009-keep-task-vocabulary.md) for why Orbit keeps its own
vocabulary and translates at the boundary, and [ADR 0010](./adr/0010-structural-type-field.md)
for the Type field.

## See also

- [`SPEC.md`](./SPEC.md) for behavior, commands, and the JSON contract.
- [`docs/ARCHITECTURE.md`](./ARCHITECTURE.md) for the component map, state machine, command flow, and sequence diagrams.
- [`docs/adr/0009-keep-task-vocabulary.md`](./adr/0009-keep-task-vocabulary.md) for the Task/Issue naming decision.
- [`docs/adr/0010-structural-type-field.md`](./adr/0010-structural-type-field.md) for the Type field.
- [`docs/adr/0012-orbit-folder.md`](./adr/0012-orbit-folder.md) for the Orbit folder, storage modes, discovery, and legacy migration.
- [`docs/adr/0013-claim-checkpoints.md`](./adr/0013-claim-checkpoints.md) for checkpoint storage and lifecycle.
