---
status: accepted
---

# ADR 0021: Nested SubTasks behind a Board setting

> **Superseded in part by [ADR 0022](./0022-issue-model-and-platform-architecture.md):**
> `board.max_subtask_depth` and the structural SubTask kind are replaced by
> Issue types and a configurable hierarchy; the nesting-depth idea survives as
> the hierarchy's configurable maximum depth.

Board owners want a hierarchy like Jira's epic, story, and sub-task or Linear's
sub-issues: a Task, its SubTasks, and their SubTasks. ADR 0007 fixed SubTasks
at one level deep and ADR 0009 cited that fixed rule as a reason Orbit's
vocabulary stays narrow. The one-level rule blocks the hierarchy outright.

We decided that Orbit gains no new kind of Task and no new vocabulary word.
Instead, each Board carries `board.max_subtask_depth`, an integer from 1 to 4
defaulting to 1. Depth counts `parent` hops: a top-level Task is depth 0, its
SubTask depth 1, its SubTask's SubTask depth 2. A Board at the default behaves
exactly as before; a Board that raises the setting can nest SubTasks further.
When the setting is 1 it is omitted from `config.toml`, so a default Board's
config file is byte-for-byte unchanged.

Hierarchical IDs (ADR 0007) extend naturally: `orb-a0b1c2.d4b3df.e5f6g7` is a
SubTask of `orb-a0b1c2.d4b3df`, which is itself a SubTask of `orb-a0b1c2`. A
Task's parent is its ID minus the last segment; every segment must satisfy the
existing segment grammar. `type` stays metadata and `epic` stays top-level
only. Readiness, Close Candidates, `delete`, and slug migration already work
for a chain; `done --force`, `cancel --force`, and `reopen` walk every
open descendant or Done ancestor rather than only the direct parent. The
setting is rejected with a configuration error outside 1 to 4.

This partly reverses ADR 0007 ("SubTasks are one level deep") and amends the
part of ADR 0009 that says Orbit deliberately has no arbitrary-depth
hierarchy. The vocabulary decision of ADR 0009 stands: no "story", no new
container noun, and Parent Task becomes a role — any Task that has SubTasks —
rather than a kind.

## Considered options

- **A new Task kind for the middle level.** Rejected: a new kind means new
  storage, new JSON, new MCP descriptions, and a new vocabulary word, all to
  name a position the graph already expresses. A SubTask that owns SubTasks is
  still a SubTask; it is also a Parent Task.
- **Unlimited depth.** Rejected: no use case needs more than a few levels, and
  deeper trees make IDs, filenames, and `tree` output harder to read. The
  1 to 4 bound keeps every grammar and rendering simple.
- **A typed link instead of nesting.** Rejected: `parent` already carries
  roll-up and readiness; a second edge kind would duplicate both rules and
  split a single hierarchy across two fields.

## Consequences

- A Board that does not set `board.max_subtask_depth` sees no change: the
  same validations, the same error messages, the same files.
- A SubTask under a SubTask is refused with `INVALID_ARGUMENT` on a default
  Board, and `NESTED_SUBTASK` in `orbit doctor` still reports hand-written
  violations beyond the configured depth.
- `done --force` and `cancel --force` can now cancel several levels; the
  result lists every cancelled Task.
- `reopen` walks every Done ancestor, so reopening a deep SubTask can reopen
  a chain of done parents.
- Task files gain more dots: `orb-a0b1c2.d4b3df.e5f6g7.md` is valid. Exact
  matching and file discovery are unaffected; humans should still use `rg -F`
  or escape dots.
- `orbit export` `schema_version` stays 3 and no readable field changes;
  new JSON fields are additive.
- Remote sync needs no depth logic: each Orbit `parent` edge becomes one
  sub-issue edge between the two linked Remote Issues, so a nested chain
  syncs level by level (verified against the fixture provider).
