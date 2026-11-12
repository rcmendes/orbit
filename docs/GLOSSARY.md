# Orbit

Orbit is a Git-native issue tracker and local work queue for coding agents. Its unit of work is a Task stored as a human-readable file.

## Language

**Task**:
A single unit of work that one agent can claim and complete. A Task may stand alone or group SubTasks as a Parent Task.
_Avoid_: Issue, ticket, bead, bean

**Slug**:
The Board's short name that prefixes every Task ID, such as `teramigo` in `teramigo-32fdfd`. A Board has exactly one Slug at a time; `orbit slug show` reports it and `orbit slug migrate` rewrites every ID when the Board's identity changes.
_Avoid_: Project name, namespace

The Board may also accept legacy prefixes from imported Tasks in `id.accepted_prefixes`; new Task IDs continue to use the one primary Slug.

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

**Area**:
An optional, single classification of where work primarily lands. Each Board defines its own Areas; a Board that defines none accepts any short lowercase name. Area is shared metadata for filtering and agent routing; it is not personal or session-local state.
_Avoid_: Label, component tag, free-form tag

**Verification**:
Optional, concise checks or commands agents should run for a Task, stored separately from narrative acceptance criteria in the body. Project-wide defaults live in [`docs/agents/project-guidance.md`](./agents/project-guidance.md); Task-level `verification` adds to or narrows those defaults.
_Avoid_: Acceptance criteria, comment, checkpoint

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

**Board**:
The versioned store of Tasks and configuration shared by its member
Repositories. A Board may have no Repository, one, or many, and grows from one
to many without changing kind. A Board in the home folder serves as a personal
Board for any directory that has no closer one.
_Avoid_: Database, store, project, Workspace, Global Board

**Repository**:
A Git repository that is a member of a Board. A Repository joins either by
living inside the folder that holds the Board's Orbit folder, or by being added
explicitly from elsewhere. Every worktree of a Repository is that same
Repository, wherever the worktree lives. A Task belongs to at most one Repository: a Task
without one is Board-wide, such as an epic spanning Repositories, and a SubTask
may belong to a different Repository than its Parent Task. A Repository may name
one Remote Project where its new Tasks are published.
_Avoid_: Linked Repository, project, submodule, workspace member

**Orbit folder**:
The `.orbit/` folder that anchors a Board. Orbit finds a Board by finding the
nearest Orbit folder at or above the current directory, crossing Git
boundaries. It holds the Board folder and the Board cache, or a pointer to the
Board folder when that folder is visible.
_Avoid_: Workspace root, data directory, hidden worktree

**Board folder** (DIR):
The folder that holds the Board's `config.toml`, `tasks/`, and `comments/`. It
is `.orbit/board` by default, or a visible folder with a chosen name, such as
`board/`, next to the Orbit folder. The Board folder keeps its own history,
separate from every code repository: Task changes never enter code branches or
code pull requests.
_Avoid_: Orbit folder, data directory, storage mode

**Board host**:
The one Git repository whose remote stores a Board's history. A Board has no
Board host (the default: its history never leaves the machine) or exactly one,
which may be one of its Repositories or another repository. The host can be set,
changed, or cleared later; clearing it never deletes the history already stored
there. A Repository can host at most one Board, and cloning a hosted Repository
lets a teammate join that Board instead of creating a second one. Being hosted
is independent of having a Remote Tracker.
_Avoid_: Tracked Board, synced Board, published Board

**Board cache**:
The rebuildable, never-versioned local data in the Orbit folder: the Repository
inventory from the last explicit refresh and the Remote Issue catalog. It is a
snapshot; nothing polls or rescans it, and deleting it loses nothing a refresh
cannot rebuild.
_Avoid_: Board, task store, Workspace cache

**Remote Tracker**:
An external issue tracker a Board may connect to, such as GitHub Issues, GitLab,
Linear, or Jira. Orbit is local-first: a Board works fully without one, and
connecting is an explicit opt-in. Orbit talks to every Remote Tracker in Orbit's
own vocabulary; each tracker's terms stay at its boundary.
_Avoid_: Provider, backend, integration, remote

**Remote Project**:
The container of Remote Issues in a Remote Tracker that a Board is connected to:
a GitHub repository, a GitLab project, a Linear team, or a Jira project. A Board
may connect to several Remote Projects, on the same or different Remote Trackers.
_Avoid_: Remote repository, linked repository

**Remote Issue**:
An item in a Remote Project, known to the Board as a cached summary. It is not a
Task, is never claimable, and stays outside the Board until an explicit import
creates or links a Task. Its full content is fetched on demand when inspected.
_Avoid_: Task, claimable Issue

**Remote Issue catalog**:
The rebuildable local cache of Remote Issue summaries for a Board's Remote
Projects. Refresh is explicit; a Remote Project whose refresh fails keeps its
previous snapshot, marked stale. Detached summaries are archived, never
claimable, and excluded from normal listing.
_Avoid_: Board, Task list, mirror

**Mapped field**:
A Task field that a given Remote Project can represent, and that therefore takes
part in sync. Which fields are mapped depends on the Remote Tracker and how the
Remote Project is configured; every other field stays local-only and is never
published or overwritten.
_Avoid_: Capability, synced field

**Linked Task**:
A Task actively paired with one Remote Issue by its stable identity. A new import
copies the Remote Issue's Mapped fields; linking an existing Local Task keeps its
Orbit values and previews differences. Orbit owns the Task's fields and
lifecycle; the Remote Issue changes only through an explicit sync or an enabled
**Auto-push**.
_Avoid_: Remote Issue, mirrored Issue

**Auto-push**:
An opt-in Board policy that publishes a Linked Task's changed Mapped fields to
its Remote Issue after a local mutation. Auto-push is push-only: it never pulls,
never resolves a conflict by overwriting, and never creates a Remote Issue for an
existing Local Task.
_Avoid_: Auto-sync, bidirectional sync, mirror

**Local Task**:
A Task not paired with any Remote Issue. Every Task on a Board without a Remote
Tracker is a Local Task. A detached Linked Task becomes a Local Task without
losing its Task data and keeps Remote provenance.
_Avoid_: Unlinked Issue

**Remote provenance**:
The retained Remote Issue identity and last sync baseline on a Local Task after
detach. It is not an active link, does not sync, and cannot be replaced by import
or export. Reattach previews the exact Remote Issue and its sync plan before
confirmation.
_Avoid_: Active link, inactive GitHub provenance

**Legacy layout**:
A Board stored anywhere other than a Board folder under an Orbit folder: a Board
directly in `.orbit/` (including the former `~/.orbit` Global Board), the hidden
worktree at `<git-common-dir>/orbit/board`, `<anchor>/board`, or
`<anchor>/.board`. Discovery does not resolve a Legacy layout; a one-time
migration converts it to the current layout.
_Avoid_: Old mode, compatibility board

## Compatibility vocabulary

Beads, Linear, and Matt Pocock's skills call the unit an **Issue**, a child a
**Sub-issue**, and a grouping issue an **Epic**. These are translations, not
stored types. An issue in a Remote Tracker is a Remote Issue until explicitly
imported:

- **Beads Issue** = Task
- **GitHub, GitLab, Linear, or Jira Issue** = Remote Issue; after import, its linked Orbit work is a Task
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
