---
title: Triage
status: todo
type: epic
priority: 2
created_at: 2026-10-28T14:07:08.818604Z
updated_at: 2026-10-28T14:07:08.818604Z
---

## Goal

Deliver GitHub-backed, multi-repository task management for Orbit, with a reusable agent Triage workflow that reviews linked tickets and proposes useful metadata updates. Orbit remains local-first: changes are applied to the Orbit Board first and published to GitHub through explicit sync.

## Domain and ownership

- Every durable Orbit Task maps one-to-one to a GitHub Issue, identified by repository and issue number. Orbit keeps Task/SubTask terminology; GitHub Issue/sub-issue terminology is translated at the provider boundary.
- GitHub owns shared issue title, body, and open/closed state. Orbit-owned `area` and `verification` values are shared through organization-level GitHub Issue Fields; field setup must be explicit and missing configuration must not silently fall back to body-text conventions.
- `area` is one controlled value per Task. `verification` contains concise checks/commands. Shared project-wide agent guidance lives in a checked-in repository guidance file.
- GitHub labels sync both ways. Triage may propose changes only to canonical Orbit triage/category labels and requires approval before applying them.
- Parent/SubTask and dependency relationships sync through GitHub's native relationships where they fit Orbit's one-level hierarchy; unsupported relationships are reported, not silently flattened. GitHub assignees remain distinct from Orbit claims.

## Sync and Triage behavior

- Repositories are explicitly linked to an Orbit Board; existing GitHub Issues are adopted explicitly, not bulk-imported by default.
- Sync is explicit and detects field-level conflicts rather than silently overwriting concurrent edits.
- GitHub open Issues remain nonterminal and preserve Orbit todo/in_progress state; closed Issues map to done unless `wontfix` means cancelled; reopening maps to todo.
- Triage reviews only Orbit-linked Tasks. It previews metadata/label proposals and requires approval; approval writes to Orbit first, and a later explicit sync publishes the changes to GitHub.
- Triage does not rewrite GitHub-owned title, body, or status.

## Completion criteria

The GitHub provider, structured metadata, and Triage workflow work together across explicitly linked repositories, with tests and user-facing documentation for setup, adoption, sync, conflicts, and the agent workflow.
