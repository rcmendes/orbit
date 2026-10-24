# 0010. Add a structural Type field

- Status: Accepted
- Date: 2026-10-24

## Context

Orbit had no machine-readable work shape. An epic or spec was a label
convention (`epic`) on a top-level Parent Task, while `bug` and `enhancement`
were triage labels. Beads and other trackers expose an issue type, so agents and
filters had to agree on a label string instead of a schema field.

Adding a full issue-type system would duplicate triage labels and invite
readiness rules, arbitrary nesting, and tracker parity that Orbit does not want.
Doing nothing would leave `epic` as a fragile convention.

## Decision

Add a first-class **Type** field to every Task:

- Values: `task` (default), `epic`, `spike`, `chore`, `decision`.
- `type` is metadata only. It does not affect readiness, claims, or completion.
- **Kind** stays structural and derived: `task` or `subtask`.
- `epic` is allowed only on a top-level Task; it is refused on a SubTask.
- Every write serializes the field, including `type: task`. Missing `type`
  reads as `task`, so pre-type files remain valid.
- Add `--type` to `add` and `update`; add repeatable `--type` and
  `--exclude-type` filters to `list` and `ready`; mirror them in MCP; include
  `type` in `show`, `list`, `ready`, `tree`, and `export`.
- Human output shows non-task types as a prefix after priority.
- `bug` and `enhancement` remain triage labels, not types.
- `type: epic` becomes the canonical epic/spec convention. The `epic` label is
  legacy: no backfill, no inference, and `--exclude-label epic` remains for old
  boards.
- Native export stays at `schema_version` 2 because schema 2 is unreleased;
  `type` is part of that schema.

> **Update (after v0.5.0):** `orbit list` now renders an aligned table with a
> `TYPE` column, so the non-task prefix described above applies to `ready` and
> `tree` output rather than the list table.

## Why

- A field is filterable, serializable, and visible to MCP clients without a
  vocabulary agreement on labels.
- Keeping `type` metadata-only preserves the narrow readiness model: parent
  edges still do all container work.
- Restricting `epic` to top-level Tasks keeps it consistent with the one-level
  hierarchy.
- Reusing triage labels for `bug`/`enhancement` avoids two sources of truth.

## Consequences

- Existing task files gain `type: task` on their next write. There is no bulk
  backfill or migration.
- Legacy `epic` labels do not match `--type epic` or `--exclude-type epic`.
  Users can remove the label and add the type when convenient.
- `doctor` reports `INVALID_TYPE` when a SubTask is typed `epic`.
- Future work-shape values are an enum change, not a free-form string.

## Alternatives considered

- **Label-only convention.** Rejected: no schema contract and no reliable
  filtering across clients.
- **Beads-shaped types including `bug` and `feature`.** Rejected: duplicates
  the triage labels and makes two sources of truth for category.
- **Free-form type strings.** Rejected: typos become query bugs.
- **Separate Epic entity.** Rejected: Parent Task already provides grouping and
  completion gating.
