# GitHub Issue sync

Orbit can link a Board to one or more GitHub repositories and keep durable Tasks aligned with GitHub Issues through explicit sync. Orbit remains local-first: the Board is updated first; GitHub changes are published only when you run sync with `--apply`.

## Prerequisites

- GitHub CLI (`gh`) authenticated for the organization and repositories you link.
- Organization-level Issue Fields named **Area** and **Verification** (names are configurable under `[github.issue_fields]`).
- A `[github]` section in `<DIR>/config.toml`:

```toml
[github]
organization = "acme-corp"
repositories = ["acme-corp/widget"]

[github.issue_fields]
area_name = "Area"
verification_name = "Verification"
```

Run `orbit github setup` to resolve and store the Issue Field GraphQL ids. Use `orbit github setup --validate-only` to check configuration without writing ids.

## Link repositories

Repositories are linked explicitly; Orbit does not bulk-import an entire repository.

```bash
orbit github repo link acme-corp/widget
orbit github repo list
```

## Adopt or create Issues

- **Adopt** an existing Issue from its URL: `orbit github adopt <task-id> https://github.com/acme-corp/widget/issues/42`
- **Adopt** an Issue with the legacy form: `orbit github adopt <task-id> --repo acme-corp/widget --number 42`
- **Create** an Issue for a Task: `orbit github create <task-id>` when exactly one repository is linked, or pass `--repo acme-corp/widget` to select one explicitly.

When using a URL, Orbit reads `owner/repo` and the issue number from the standard GitHub Issue URL. Optional `--repo` or `--number` values can confirm the URL; if provided, they must match it. Without a URL, both legacy options are required. Titles and bodies are not used as identifiers.

When creating an Issue, Orbit selects the only linked repository automatically. If zero or multiple repositories are linked, pass `--repo OWNER/REPO` after linking the repository. An explicit `--repo` must name a linked repository.

## Sync behavior

Preview (default):

```bash
orbit github sync <task-id>
```

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
