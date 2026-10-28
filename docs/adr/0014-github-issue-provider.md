# ADR 0014: GitHub Issue provider

## Context

Orbit needs explicit, multi-repository synchronization with GitHub Issues while keeping Orbit-owned workflow semantics (claims, checkpoints, readiness) on the Board.

## Decision

- Boards opt in through `config.toml` `[github]` and explicitly linked repositories. The default `full` mode uses an organization for Issue Fields and resolved ids after `orbit github setup`; `basic` mode supports repositories without Issue Fields.
- Each durable Task links to at most one GitHub Issue, identified by `owner/repo` and issue number stored in task frontmatter (`github.repo`, `github.number`, optional `last_sync` baseline).
- The `gh` CLI is the transport; tests use `ORBIT_GITHUB_FIXTURE` with an in-memory client.
- GitHub is authoritative for title and body; open/closed state syncs with Orbit status. In full mode, organization Issue Fields carry Orbit `area` and `verification`. Labels merge both ways with field-level conflict detection using the stored baseline.
- GitHub assignees are never mapped to Orbit claims. Parent/SubTask and dependency edges map to sub-issues and blocked-by relationships only within the same repository; cross-repository relationships are reported, not silently flattened.

## Consequences

- Sync is explicit (`orbit github sync`, dry-run by default; `--apply` writes).
- Missing Issue Field setup returns `GITHUB_SETUP_INCOMPLETE` instead of falling back to body-text conventions.
- Basic mode syncs titles, bodies, labels, and open/closed state, but does not import or publish `area` and `verification`; Triage is limited to labels. Switching to full mode and configuring the organization fields is the upgrade path.
- Parallel work on structured metadata (`area`, `verification` frontmatter) shares the task model with ticket `orbit-6pm4p0.2m09a7`.
