# Claim checkpoints live in task frontmatter

A Task stores at most one latest checkpoint in its own frontmatter: `by`, `at`, `progress`, and optional `next` and `blocker`. `orbit checkpoint` replaces it in place. The field is Orbit-owned execution state, not issue metadata, and is never synchronized to an external tracker.

The checkpoint must survive `release`, `claim --steal`, and `reopen`, because those are exactly the handoffs it exists for. Storing it on the Task keeps one file and one atomic write per update. The claim relation already lives there, and task-detail surfaces (`show`, `ready`, `prime`, MCP `get`, `export`) already load the whole file, so no extra read or merge path is needed. One record is enough: the next claimant needs the current state, not the full log. Comments remain the append-only history when an audit trail matters.

## Considered options

- **A separate `checkpoints/<id>.md` sidecar**, like comments 0005. It would preserve every checkpoint as immutable history, but it adds a second file, a second delete cascade, and a second write on every handoff. The ticket asks for the latest checkpoint, so full history is not worth the cost.
- **An append-only checkpoint list in frontmatter.** It bloats metadata and every read/write of the Task, and it makes "the latest" a derived query instead of a field.
- **Store checkpoints as comments.** They are append-only and already visible, but a comment is prose with no structured `progress`, `next`, or `blocker`, and agents cannot query or update it reliably.
- **Synchronize checkpoints to GitHub Issue metadata.** It would leak transient execution state into shared issue fields and conflict with the rule that Orbit owns claims and leases.

## Consequences

A later checkpoint overwrites the previous one; the Board's Git history is the audit trail for older values. A terminal Task keeps its latest checkpoint in the file as archived history, and detail projections add `checkpoint_archived: true` so active handoff surfaces can ignore it. The command refuses terminal and unclaimed Tasks, and never changes Status, readiness, Claim eligibility, dependencies, or `waiting_on`.

Because the checkpoint is free-form text, `orbit slug migrate` does not rewrite IDs quoted inside it, matching the treatment of task bodies and comments.
