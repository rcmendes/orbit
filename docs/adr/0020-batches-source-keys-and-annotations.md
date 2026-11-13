---
status: accepted
---

# ADR 0020: Batches, Source Keys, and Annotations

Tools that drive Orbit from a plan (a planning skill, an orchestrator, an
importer) need three things the single-task commands do not give them: several
changes that land together, a retry that does not duplicate work, and a place
on the Task for their own data. We added three general features, each usable
without the others.

**Batch.** `orbit batch` applies a list of Task operations under one Board lock
and publishes them as one commit, all or none. Every operation runs against one
in-memory Graph through `src/ops.rs`, the same functions the single commands
call, so a rule has one implementation. Nothing is written until every
operation succeeds; then changed Task files are written atomically one by one,
comments are appended, and one commit is made. The command is named `batch`,
not `apply`, because `--apply` already means "run the previewed change" in
`migrate`, `triage`, and `remote`.

**Source Key.** An optional, caller-chosen string on a Task, unique per Board.
`add` with an existing Source Key returns that Task instead of creating another.
Together with the same-holder claim renewal, the idempotent `label`,
`depend`, `annotate`, and `done`, and `reopen` of an open Task being a no-op
inside a Batch, re-running a Batch converges, except for `comment` (which
appends again) and an operation whose precondition a later operation in the
same Batch removed (such as `claim` followed by `done`). New Tasks are written
in creation order before changed ones, so a crash between writes never leaves
a dependency on a Task file that was not written, and a re-run always commits.

**Annotation.** Structured data one tool keeps on a Task under its own name.
Orbit stores, returns, and shows it but never interprets it, and never syncs it
to a Remote Tracker, for the reason ADR 0013 keeps Checkpoints local. Board host
sync merges Annotations per owner.

## Considered options

- **A transaction log with rollback.** It would make a crash mid-write
  invisible, but it adds a second storage format and a recovery path to every
  command. A crash between file writes is rare, and a re-run of the same Batch
  repairs it, so idempotent operations are enough.
- **Typed dependency edges.** Tools with richer edge semantics keep them in
  their Annotation; Orbit keeps one Dependency kind with one readiness rule.
- **Claims without a Lease for orchestrators.** Rejected: the Lease is what
  returns abandoned work to the queue. An orchestrator renews its Claims with
  `heartbeat`.
- **Validation of Annotations and Source Keys in the Graph's load check.** A
  Strict load fails on any validation issue, so one malformed value would block
  every command. `doctor` reports them instead (`INVALID_ANNOTATION`,
  `DUPLICATE_SOURCE_KEY`), and a keyed `add` refuses a duplicated key.

## Consequences

- `orbit export` `schema_version` is 3.
- `claim` by the current holder renews its Lease instead of failing.
- A crash between file writes can leave part of a Batch on disk, uncommitted
  until the next commit; re-running the Batch completes it.
