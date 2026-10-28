# ADR 0014: GitHub Issue provider

## Context

Orbit needs explicit, multi-repository synchronization with GitHub Issues while keeping Orbit-owned workflow semantics (claims, checkpoints, readiness) on the Board.

## Decision

- Boards opt in through `config.toml` `[github]`: an organization for Issue Fields, explicitly linked repositories, and resolved field ids. `orbit github connect` detects and writes this configuration for the current organization-owned `origin`; manual configuration followed by `orbit github setup` is also supported. Setup resolves existing fields by default; `--create-fields` explicitly creates missing fields after checking active organization administrator membership.
- Each durable Task links to at most one GitHub Issue, identified by `owner/repo` and issue number stored in task frontmatter (`github.repo`, `github.number`, optional `last_sync` baseline).
- The `gh` CLI is the transport; tests use `ORBIT_GITHUB_FIXTURE` with a fixture-backed client that records opt-in field creation.
- GitHub is authoritative for title, body, and open/closed state. Organization Issue Fields carry Orbit `area` and `verification`. Labels merge both ways with field-level conflict detection using the stored baseline.
- GitHub assignees are never mapped to Orbit claims. Parent/SubTask and dependency edges map to sub-issues and blocked-by relationships only within the same repository; cross-repository relationships are reported, not silently flattened.

## Consequences

- Sync is explicit (`orbit github sync`, dry-run by default; `--apply` writes).
- Missing Issue Field setup returns `GITHUB_SETUP_INCOMPLETE` instead of falling back to body-text conventions.
- Parallel work on structured metadata (`area`, `verification` frontmatter) shares the task model with ticket `orbit-6pm4p0.2m09a7`.
