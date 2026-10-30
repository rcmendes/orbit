# ADR 0014: GitHub Issue provider

## Context

Orbit needs explicit, multi-repository synchronization with GitHub Issues while keeping Orbit-owned workflow semantics (claims, checkpoints, readiness) on the Board.

## Decision

- Boards opt in through `config.toml` `[github]` and explicitly linked repositories. The default `full` mode uses an organization for Issue Fields and resolved ids after `orbit github setup`; `basic` mode supports personal repositories without Issue Fields. `orbit github connect` detects and configures an organization-owned `origin`; for a personal-account `origin`, it leaves the Board unchanged and reports the exact `[github]` entries to add or merge into `config.toml` for basic mode, preserving other linked repositories. Manual configuration is also supported. Setup resolves existing fields by default; `--create-fields` explicitly creates missing fields after checking active organization administrator membership.
- Each durable Task links to at most one GitHub Issue, identified by `owner/repo` and issue number stored in task frontmatter (`github.repo`, `github.number`, optional `last_sync` baseline).
- Orbit is authoritative for Linked Task title, body, status, labels, and (in full mode) `area` and `verification`. A remote-only edit can be pulled while the matching Orbit field remains at its saved baseline; local-only edits push. Same-field concurrent edits require a per-field Orbit/GitHub choice. Labels are exact values, not merged.
- The `gh` CLI is the transport; tests use `ORBIT_GITHUB_FIXTURE` with a fixture-backed client that records opt-in field creation.
- GitHub assignees are never mapped to Orbit claims. Parent/SubTask and dependency edges map to GitHub sub-issues and blocked-by relationships only within the same repository. Sync previews and applies supported additions; unsupported or unlinked remote relationships are reported and preserved, not flattened or guessed away.

## Consequences

- Sync is explicit (`orbit github sync`, dry-run by default; `--apply` writes) unless the Board opts into the automatic push defined by [ADR 0016](./0016-opt-in-auto-push.md).
- Missing Issue Field setup returns `GITHUB_SETUP_INCOMPLETE` instead of falling back to body-text conventions.
- Basic mode syncs titles, bodies, labels, and open/closed state, but does not import or publish `area` and `verification`; Triage is limited to labels. Switching to full mode and configuring the organization fields is the upgrade path.
- Parallel work on structured metadata (`area`, `verification` frontmatter) shares the task model with ticket `orbit-6pm4p0.2m09a7`.
