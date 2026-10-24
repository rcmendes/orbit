# Orbit

Orbit is a local work queue for coding agents. Its unit of work is a Task stored as a human-readable file.

## Language

**Task**:
A single unit of work that one agent can claim and complete.
_Avoid_: Issue, ticket, bead, bean

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
The versioned store of Tasks and configuration for a Workspace.
_Avoid_: Database, store, project

**Linked Repository**:
A Repository whose Agents use a Board that does not live inside it.
_Avoid_: Submodule, project, workspace

## See also

- [`SPEC.md`](./SPEC.md) for behavior, commands, and the JSON contract.
- [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) for the component map, state machine, command flow, and sequence diagrams.
