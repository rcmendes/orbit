# GitHub Issue sync

Orbit can link a Board to one or more GitHub repositories and keep durable Tasks aligned with GitHub Issues through explicit sync. Orbit remains local-first: the Board is updated first; GitHub changes are published only when you run sync with `--apply`.

## Provider modes

Orbit supports two modes. Existing configuration defaults to `full`, so existing
organization setups keep their current behavior.

### Full mode (organization Issue Fields)

- Requires organization-level Issue Fields named **Area** and **Verification** (names are configurable under `[github.issue_fields]`).
- Run `orbit github setup` to resolve and store the Issue Field GraphQL ids. Use `orbit github setup --validate-only` to check configuration without writing ids.
- Full mode syncs Orbit `area` and `verification` through those organization Issue Fields.

Connect an organization-owned repository from its checkout with `orbit github
connect`. It checks `gh auth status` and repository access, validates the
organization fields, and writes the `[github]` configuration. It preserves other
Board settings and is safe to rerun. Use `orbit github connect --json` for
machine-readable status. To configure the Board manually, use:

```toml
[github]
mode = "full" # optional; full is the default
organization = "acme-corp"
repositories = ["acme-corp/widget"]

[github.issue_fields]
area_name = "Area"
verification_name = "Verification"
```

For manual configuration, run `orbit github setup` to resolve existing Issue Fields and store their GraphQL ids. This default command does not create fields. Use `orbit github setup --validate-only` to check configuration without writing ids.

To let Orbit create missing fields, explicitly run `orbit github setup --create-fields`. It creates **Area** as a single-select field with `board`, `cli`, `mcp`, `setup`, `docs`, and `release` options, and **Verification** as a text field. Existing fields with those names are reused and are not changed. Before creating anything, Orbit verifies that the authenticated `gh` user has an active organization administrator membership. If the membership check fails, refresh the `read:org` scope with `gh auth refresh --hostname github.com --scopes read:org`. If field creation is denied, confirm that `gh` is authenticated as an organization administrator with permission to create organization Issue Fields. An administrator can also create the fields in GitHub and rerun the default `orbit github setup` to store their ids.

### Basic mode (personal repositories)

Basic mode supports personal repositories without organization Issue Fields or
`orbit github setup`:

```toml
[github]
mode = "basic"
repositories = ["alice/widget"]
```

Link repositories, adopt or create Issues, and sync GitHub-owned title, body,
open/closed state, and labels. Title, body, and state flow from GitHub to Orbit;
labels merge in both directions, and Orbit status changes can open or close an
Issue. GitHub assignees remain informational and are never mapped to Orbit
claims.

Basic mode does **not** support organization Issue Fields: `area` and
`verification` are neither imported from GitHub nor pushed to GitHub. Create
and push sync fail with a clear error when a Task has either value, rather than
silently dropping it. Pull-only sync preserves Orbit's local values. Triage
context reports these capabilities as unavailable; Triage review/apply may
change labels, but proposals that change `area` or `verification` are rejected.
`orbit github setup` is unavailable in basic mode. Do not set `organization`,
custom Issue Field names, or Issue Field ids with `mode = "basic"`.

To upgrade, change `mode` to `"full"`, configure `organization` and the field
names, then run `orbit github setup`. Once the Issue Field ids are stored,
create and push sync can publish `area` and `verification`.

### Connect behavior and prerequisites

- GitHub CLI (`gh`) authenticated for the repositories you link.
- Organization-level Issue Fields only when using full mode.

`orbit github connect` reads the current checkout's GitHub `origin`, checks
authentication and access, and detects whether the owner is an organization or a
personal account. For a personal-account repository it leaves configuration
unchanged and reports next steps; manually configure basic mode to use it. For
an organization repository, it validates and stores the repository and Issue
Field configuration. Missing authentication, repository access, or fields are
reported with remediation actions. JSON output includes `owner_type`,
`organization`, `repository`, `field_status`, `next_action`, and whether the
configuration changed.

For manual full-mode configuration, run `orbit github setup` to resolve and
store the Issue Field GraphQL ids. Use `orbit github setup --validate-only` to
check configuration without writing ids.

## Diagnose the integration

`orbit github status` checks the current `gh` authentication identity and scopes,
linked repository access, and—when using full mode—the configured organization
and whether its Area and Verification Issue Fields are present with current stored
ids. It also loads linked Tasks and previews their sync plans to count pending
pull and push changes and detect conflicts. Cross-repository SubTask and
dependency links are reported as unsupported. The command is read-only: it does
not change Board configuration, Tasks, GitHub Issues, or the fixture used by
integration tests. In basic mode, it reports the `repo` scope, skips organization
checks, and marks Area and Verification as not applicable.

Use `--json` for the stable diagnostic structure. `data.pending.pull` and
`data.pending.push` count field changes; `pull_tasks` and `push_tasks` identify the
affected Tasks and counts. `data.conflicts` and `data.conflict_details` report
conflicts, while `data.unsupported_relationships` and `data.unsupported_details`
report relationships Orbit cannot synchronize. `data.next_action` recommends the
next step, and `data.next_command` supplies an executable command when one applies.
`data.auth.scopes_known` distinguishes an unreported scope list from an empty one.
When the GitHub provider is configured,
`orbit doctor` also reports authentication, repository access, and Issue Field
configuration problems alongside normal Board diagnostics.

```bash
orbit github status
orbit github status --json
orbit doctor --json
```

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
| Title, body | GitHub | Pulled on sync |
| Open/closed | GitHub + Orbit status | Pulled on sync; Orbit `done`/`cancelled` pushes closed/open state; closed + `wontfix` → Orbit `cancelled` |
| Labels | Both | Merged; concurrent edits since last sync → conflict |
| `area`, `verification` | Orbit (via Issue Fields) | Full mode only; pushed to fields; concurrent edits → conflict; unavailable in basic mode |
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

Integration tests set `ORBIT_GITHUB_FIXTURE` to a JSON file with mock org fields and issues so CI does not call GitHub. Basic mode ignores Issue Field values in fixture Issues.

See [ADR 0014](./adr/0014-github-issue-provider.md).
