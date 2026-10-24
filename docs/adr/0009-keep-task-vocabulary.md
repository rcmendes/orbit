# 0009. Keep Task/SubTask vocabulary and translate Issue at the boundary

- Status: Accepted
- Date: 2026-10-24

## Context

Beads, GitHub, Linear, and Matt Pocock's skills call the tracker unit an
**Issue**, a child a **Sub-issue**, and a grouping issue an **Epic**. Orbit's
domain model calls the unit a **Task**, a child a **SubTask**, and a grouping
Task a **Parent Task**.

Renaming would touch the board layout, frontmatter, JSON envelopes, MCP tool
descriptions, CLI help, Rust types, generated agent instructions, and every
consumer. Beads also supports issue types and arbitrary-depth hierarchy, which
Orbit deliberately does not. A rename would therefore imply parity that Orbit
does not provide.

## Decision

Keep **Task**, **SubTask**, and **Parent Task** as the canonical vocabulary. Do
not rename storage, JSON, MCP, or Rust types.

Translate external vocabulary in documentation and at tool boundaries:

- **Issue** = Task
- **Sub-issue** = SubTask, created with `--parent`
- **Epic** or **spec** = a top-level Task with `type: epic` (see
  [ADR 0010](./0010-structural-type-field.md))

Add familiar views and boundary adapters when they earn their keep, such as a
scoped tree view or a Beads-shaped export/import. Do not introduce a separate
Epic entity.

## Why

- **"Issue" overpromises.** It suggests a general-purpose tracker with types,
  assignees, milestones, attachments, and arbitrary nesting. Orbit is a
  claimable work queue; "Task" describes that contract precisely.
- **"Sub-issue" implies arbitrary nesting.** Orbit enforces one level. The name
  would promise a hierarchy the CLI rejects.
- **The lifecycle reads better.** Claim, heartbeat, done, and cancel are task
  verbs. Issues are opened and closed; tasks are worked and completed.
- **The rename is high-churn, low-gain.** Matt's skills need a one-line mapping,
  not a new schema. A full rename would break existing boards and agent
  integrations for a vocabulary preference.
- **Compatibility is more than a noun.** Beads compatibility also needs types,
  filters, and import/export. Renaming alone would create false expectations.

## Consequences

- Every generated tracker guide states the Issue/Sub-issue/Epic mapping.
- `kind` stays structural (`task` or `subtask`). An epic remains a `task`; its
  Type is `epic`, not a second container model.
- The structural Type field is recorded in
  [ADR 0010](./0010-structural-type-field.md).
- A future Beads export/import maps `issue_type`, `parent-child`, and `blocks`
  at the boundary rather than in the core schema.
- If maintainers later choose "Issue" as the canonical noun, it is a v1.0
  breaking change with a migration command and a legacy JSON flag, not a
  partial rename.

## Alternatives considered

- **Full rename to Issue/Sub-issue.** Rejected for now: high churn, and
  "sub-issue" overpromises nesting.
- **Rename only the docs.** Rejected: two vocabularies in the same product make
  the CLI, JSON, and guides disagree.
- **Add an Epic entity.** Rejected: Parent Task already provides grouping,
  readiness gating, and scoped queues. A second container would duplicate it.
