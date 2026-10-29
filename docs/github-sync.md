# GitHub Issue sync

Orbit can link a Board to one or more GitHub repositories and keep durable Tasks aligned with GitHub Issues through explicit sync. Orbit remains local-first: Task edits stay on the Board; Issue creation occurs only with `orbit github create --apply`, and Orbit persists the Issue link only after GitHub confirms creation. Changes to an existing Issue are published only with `orbit github sync --apply`.

GitHub sync requires Orbit **v0.9.0 or newer**. That release includes `orbit
github` and `orbit triage` in its default binaries, shell/PowerShell installers,
and npm package. If a binary built without the optional `github-provider`
capability loads a configured Board, it stops with an upgrade message instead
of silently ignoring the GitHub integration.

## Quick start

For an organization-owned repository with Issue Fields ready, create a fresh
Board in the repository checkout and authenticate GitHub CLI:

```bash
orbit init
gh auth login
orbit github connect
orbit github status
orbit add "First GitHub task" -p 1
# Copy the ID printed by `orbit add` and replace orb-a7b2c9 below.
orbit github create orb-a7b2c9
orbit github create orb-a7b2c9 --apply
orbit github sync orb-a7b2c9        # inspect the plan; writes nothing
orbit github sync orb-a7b2c9 --apply
```

## Workspace Remote Issue catalog

A Workspace can keep a local catalog of Remote Issues without importing them as
claimable Tasks. From the Workspace root or a child repository, run:

```bash
orbit workspace issues refresh
orbit workspace issues list --repo acme/api --state open --label bug
orbit workspace issues list --text authentication --unlinked
orbit workspace issues inspect acme/api#42
orbit workspace issues import acme/api#42
orbit workspace issues import acme/api#42 acme/web#8 --apply
orbit workspace issues import acme/api#42 --link acme/api#42=orb-a7b2c9 --apply
```

Refresh explicitly reconciles direct child repositories, then paginates through
all open and closed GitHub Issues for included repositories. Pull requests are
excluded. The rebuildable summary cache is stored at
`<workspace>/.orbit/cache/issues.json`, separate from the Workspace Board. The
cache stores stable identities, labels, timestamps, URLs, and GitHub Issue Field
values (including Area and Verification when configured).
Repository failures are reported independently and preserve the previous local
snapshot as stale. Listing reads cached summaries only; inspect fetches the issue
body and comments from GitHub on demand. Catalog refresh performs no GitHub writes
and creates no Orbit Tasks. Catalog entries remain absent from `orbit list`,
`orbit ready`, claims, and Task export until explicitly imported.

Import accepts one or more explicit `OWNER/REPO#NUMBER` identities; it never
selects the whole catalog. Without `--apply`, it previews the Linked Tasks to
create, Local Tasks to link, terminal status mappings, duplicates, and errors.
Add `--apply` after reviewing the selection. New Tasks copy the Issue title,
body, labels, and configured fields, keep the stable repository/number identity,
and map closed Issues to `done` or `cancelled` when the `wontfix` label applies.
Repeat `--link OWNER/REPO#NUMBER=TASK` to pair a selected Issue with a specific
Local Task. Orbit values remain unchanged; the preview lists field differences
before confirmation. An Issue already linked elsewhere, or a Task already linked
to another Issue, is reported as a skipped duplicate. Matching uses repository
and Issue number, never title. Import fetches Issue details but performs no GitHub
writes; imported Tasks follow normal readiness, dependency, hold-label, and claim
rules.

## Detach and reattach

Detach is an explicit local-only lifecycle operation. Both repository and
individual Linked Task commands preview first; `--apply` is required to change
the Board or Workspace catalog:

```bash
orbit workspace detach acme/api
orbit workspace detach acme/api --apply
orbit workspace issues detach acme/api#42
orbit workspace issues detach acme/api#42 --apply
```

Repository detach previews all active Linked Tasks and cached Remote Issue
summaries for that repository. Applying it excludes the repository, changes its
Linked Tasks into ordinary Local Tasks, and moves cached summaries into the
`archived_issues` section of `<workspace>/.orbit/cache/issues.json`. Individual
detach archives only that Issue snapshot. Task title, body, status, labels,
relationships, comments, and last sync baseline remain intact. The stable
repository/Issue number stays in Task frontmatter with `active: false`; it is
inactive provenance, not a sync link. Archived summaries are not returned by
normal catalog listing, and they are never claimable Tasks. Neither detach path
closes, deletes, or otherwise writes to GitHub. Detach and reattach are safe to
repeat.

Reattach also previews first and matches only exact repository/Issue number
identities:

```bash
orbit workspace reattach acme/api
orbit workspace reattach acme/api --apply
orbit workspace issues reattach acme/api#42
orbit workspace issues reattach acme/api#42 --apply
```

Repository reattach previews current sync plans for its inactive Tasks and the
archived snapshots. Applying it re-includes the repository, restores snapshots,
and reactivates those exact links. Task reattach requires its repository to be
included and previews current GitHub values, planned changes, and field-level
conflicts for the matching Local Task. Applying reactivates the link but does not
pull, push, resolve conflicts, or write GitHub. Use `orbit github sync` as a
separate explicit operation to review and resolve the plan. A conflicting
identity on another Task blocks reattachment instead of creating duplicate
links.

`connect` detects the checkout's `origin`. Organization-owned repositories
configure full mode when their Issue Fields are ready. For personal-account
repositories, it leaves the Board unchanged and reports the exact `[github]`
entries to add or merge into the Board's `config.toml`, preserving other linked
repositories; apply the
[basic-mode configuration](#basic-mode-personal-repositories) before creating
or syncing Issues. Basic mode does not sync `area` or `verification`.

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

Link repositories, adopt or create Issues, and explicitly sync linked Tasks.
Orbit owns title, body, status, and labels. A GitHub-only edit can be pulled when
the matching Orbit field still equals its saved baseline; an Orbit-only edit is
pushed. Concurrent edits to one field require a reviewed resolution. GitHub
assignees remain informational and are never mapped to Orbit claims.

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
unchanged and reports the exact `[github]` entries for the Board's `config.toml`
to configure basic mode; merge them with existing settings and preserve other
linked repositories. For an organization repository, it validates and
stores the repository and Issue Field configuration. Missing authentication,
repository access, or fields are reported with remediation actions. JSON output
includes `owner_type`, `organization`, `repository`, `field_status`,
`next_action`, and whether the configuration changed. For a personal-account
origin, it also includes `next_actions` and `remediation` with those entries and
a link to the basic-mode setup instructions; human output shows the same
guidance. No configuration is changed by this response.

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
- **Preview** Issue creation for one or more Local Tasks with `orbit github create <task-id> [<task-id> ...]`. Preview shows repository, title, body, labels, Area, Verification, Task status, and the resulting open/closed Issue state. It does not write to Orbit or GitHub.
- **Apply** a reviewed export with `orbit github create <task-id> --apply`. A batch uses an explicit list of Task IDs; it never exports the whole Board. Done and cancelled Tasks require `--include-terminal`.
- Full mode includes Task `area` and `verification` values as organization Issue Fields when configured with `orbit github setup`. Basic mode rejects Tasks with those values instead of dropping them.
- A repository is inferred only when exactly one repository is available. On a Workspace Board, run `orbit workspace refresh`; the included repository catalog is used. On a repository Board, link repositories with `orbit github repo link`. Pass `--repo OWNER/REPO` when the destination is ambiguous.
- GitHub creates each Issue before Orbit persists the stable `owner/repo#number` link and sync baseline. Batch output reports each success or failure. If a Task already has a link, retry is refused and directs you to `orbit github sync`; this prevents a second Issue from being created.

Preview and apply examples:

```bash
orbit github create orb-a7b2c9
orbit github create orb-a7b2c9 --apply
orbit github create orb-a7b2c9 orb-d4e5f6 --repo acme-corp/widget --apply
orbit github create orb-terminal --include-terminal --apply
```

When using a URL, Orbit reads `owner/repo` and the issue number from the standard GitHub Issue URL. Optional `--repo` or `--number` values can confirm the URL; if provided, they must match it. Without a URL, both legacy options are required. Titles and bodies are not used as identifiers.

On a repository Board, Orbit selects the only linked repository automatically. On a Workspace Board, it selects from the included repositories in the last refreshed Workspace catalog. If zero or multiple repositories are available, refresh/include one or pass `--repo OWNER/REPO`. An explicit destination must be linked on a repository Board or included in the Workspace catalog.

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
| Title, body | Orbit | A remote-only edit is pulled if Orbit still matches the baseline; local edits push. Same-field edits conflict. |
| Status and open/closed | Orbit status | `todo`/`in_progress` stay open; `done`/`cancelled` close; remote state changes pull only if Orbit status is unchanged. A closed Issue with `wontfix` maps to `cancelled`. |
| Labels | Orbit | Labels are synchronized as an exact field value, not merged. Remote-only edits pull; edits on both sides conflict. |
| `area`, `verification` | Orbit (via Issue Fields) | Full mode only; remote-only edits can pull; Orbit changes push; concurrent edits conflict. Unavailable in basic mode. |
| Assignees | GitHub only | Never mapped to Orbit claims |
| Claims, checkpoints | Orbit only | Never synchronized to GitHub |

Conflicts return `GITHUB_ISSUE_CONFLICT` with per-field details and both values.
No write occurs while any selected Task has an unresolved conflict. Choose a
side for each conflicting field with repeatable `--resolve FIELD=orbit` or
`--resolve FIELD=github`, then require `--apply` to write. `orbit` keeps the
Orbit value and pushes it; `github` pulls the GitHub value. A resolution can be
previewed without writing:

```bash
orbit github sync orb-a7b2c9 --resolve title=orbit
orbit github sync orb-a7b2c9 --resolve title=orbit --resolve labels=github --apply
```

For a batch, one resolution applies to every selected Task that has a conflict
in that field. Sync plans all selected Tasks before writing. Apply then reports
each Task's result independently; a failed Task keeps its previous baseline, so
it can be retried without repeating already successful Tasks' writes.

GitHub command errors include a `Next:` action in human output. With `--json`,
the existing `{ "ok": false, "error": ... }` envelope is unchanged and
`error.details` includes the original `cause` and the same `remediation`, plus
the affected organization, repository, Task, Issue, or fields when known.

- `GITHUB_NOT_CONFIGURED`: run `orbit github connect` from the checkout with
  the intended GitHub origin; authenticate first with `gh auth login` if needed.
- `GITHUB_SETUP_INCOMPLETE`: run `orbit github setup` to resolve field ids. If
  a named Issue Field is missing, create it under organization **Settings >
  Projects > Issue fields** or run `orbit github setup --create-fields` as an
  organization administrator. Run `orbit github connect` if the organization
  is not configured.
- `GITHUB_REPO_NOT_LINKED`: the error names `OWNER/REPO` and gives the exact
  `orbit github repo link OWNER/REPO` command.
- `GITHUB_ISSUE_CONFLICT`: the error names the Task, Issue, and conflicting
  fields. Review both values with `orbit github sync <task-id>`, choose each
  side with `--resolve FIELD=orbit|github`, then apply with `--apply`.

## Relationships

SubTasks and dependencies are previewed as GitHub sub-issues and blocked-by
relationships when both linked Issues live in the **same** repository. `--apply`
adds supported relationships. Cross-repository or unlinked GitHub relationships
are listed as unsupported and preserved; Orbit does not flatten them or guess
which remote-only relationship to remove. GitHub assignees remain separate from
Orbit claims.

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
    status: todo
    issue_state: open
```

An active link omits `active` (older Task files remain active by default). A
detached Local Task keeps the same repository, Issue number, and `last_sync`, but
stores `active: false`; Orbit treats that identity as provenance and excludes it
from synchronization until reattachment.

Controlled `area` values match `TaskArea`: `board`, `cli`, `mcp`, `setup`, `docs`, `release`.

## Testing

Integration tests set `ORBIT_GITHUB_FIXTURE` to a JSON file with mock org fields and issues so CI does not call GitHub. Basic mode ignores Issue Field values in fixture Issues.

See [ADR 0014](./adr/0014-github-issue-provider.md).
