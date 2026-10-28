# GitHub Issue sync

Orbit can link a Board to one or more GitHub repositories and keep durable Tasks aligned with GitHub Issues through explicit sync. Orbit remains local-first: the Board is updated first; GitHub changes are published only when you run sync with `--apply`.

## Prerequisites

- GitHub CLI (`gh`) authenticated for the organization and repositories you link.
- Organization-level Issue Fields named **Area** and **Verification** (names are configurable under `[github.issue_fields]`).
- An organization-owned GitHub repository. `orbit github connect` detects it from the current checkout's `origin`, validates the setup, and writes `[github]` automatically. To configure the Board manually instead, add:

```toml
[github]
organization = "acme-corp"
repositories = ["acme-corp/widget"]

[github.issue_fields]
area_name = "Area"
verification_name = "Verification"
```

For manual configuration, run `orbit github setup` to resolve and store the Issue Field GraphQL ids. Use `orbit github setup --validate-only` to check configuration without writing ids.

For a new Board, run `orbit github connect` from a checkout with a GitHub `origin`. It checks `gh auth status`, verifies repository access, detects whether the owner is an organization or a personal account, validates the organization fields, and writes the `[github]` organization, repository link, and field ids. It preserves other Board configuration and is safe to rerun. Use `orbit github connect --json` to inspect `owner_type`, `organization`, `repository`, `field_status`, `next_action`, and whether configuration changed.

The current Issue Fields integration requires an organization-owned repository. For a personal-account repository, connect leaves configuration unchanged and reports these options: use an organization-owned repository, use `orbit github basic` when personal-account basic mode is available, or cancel. Missing `gh` authentication, repository access, or Issue Fields is reported with a remediation action.

## Link repositories

Repositories are linked explicitly; Orbit does not bulk-import an entire repository.

```bash
orbit github repo link acme-corp/widget
orbit github repo list
```

## Adopt or create Issues

- **Adopt** an existing Issue: `orbit github adopt <task-id> --repo acme-corp/widget --number 42`
- **Create** an Issue for a Task: `orbit github create <task-id> --repo acme-corp/widget`

Issue identity is always `owner/repo` plus issue number. Titles and bodies are not used as identifiers.

## Sync behavior

Preview (default):

```bash
orbit github sync <task-id>
```

The default preview is human-readable and does not write to Orbit or GitHub. It
shows each linked Task and Issue, the fields planned to be pulled into Orbit or
pushed to GitHub, and any conflicts with both competing values. Empty plans say
that no linked Tasks were found; a linked Task with no pending changes is marked
**No changes needed**. Use `--json` for the existing machine-readable plan and
conflict details.

Apply changes:

```bash
orbit github sync <task-id> --apply
orbit github sync --apply                 # all linked Tasks
orbit github sync <task-id> --apply --direction pull
```

Rules:

| Field | Authority | Notes |
| --- | --- | --- |
| Title, body, open/closed | GitHub | Pulled on sync; closed + `wontfix` label → Orbit `cancelled` |
| Labels | Both | Merged; concurrent edits since last sync → conflict |
| `area`, `verification` | Orbit (via Issue Fields) | Pushed to GitHub fields; concurrent edits → conflict |
| Assignees | GitHub only | Never mapped to Orbit claims |
| Claims, checkpoints, status while in progress | Orbit | Open GitHub Issues preserve `todo` / `in_progress` |

Conflicts return `GITHUB_ISSUE_CONFLICT` with per-field details. Sync never silently overwrites concurrent edits on both sides.

## Relationships

SubTasks and dependencies sync to GitHub sub-issues and blocked-by relationships when linked Issues live in the **same** repository as the parent. Cross-repository relationships are listed in sync output as unsupported rather than flattened.

## Task frontmatter

Linked Tasks store:

```yaml
area: backend
verification: cargo test
github:
  repo: acme-corp/widget
  number: 42
  last_sync:
    at: 2026-10-23T12:00:00Z
    issue_updated_at: 2026-10-23T11:55:00Z
    orbit_updated_at: 2026-10-23T11:50:00Z
    title: "Example"
    body: "Body"
    labels: [bug]
    area: backend
    verification: cargo test
```

Controlled `area` values match `TaskArea`: `board`, `cli`, `mcp`, `setup`, `docs`, `release`.

## Testing

Integration tests set `ORBIT_GITHUB_FIXTURE` to a JSON file with mock org fields and issues so CI does not call GitHub.

See [ADR 0014](./adr/0014-github-issue-provider.md).
