# Command reference and recovery

The command map at a glance, and what to do when something goes wrong.

## Command map

| Stage | Commands |
| --- | --- |
| **Set up** | `init` · `repo` · `setup` |
| **Author** | `add` · `update` · `depend` · `undepend` · `link` · `label` · `comment` · `delete` |
| **Inspect** | `list` · `search` · `digest` · `ready` · `show` · `tree` · `prime` · `export` · `changes` · `history` · `remote status` |
| **Execute** | `claim` · `heartbeat` · `checkpoint` · `release` · `done` · `cancel` · `reopen` |
| **Maintain** | `doctor` · `sync` · `index` · `board` · `migrate` · `remote` · `slug` · `mcp` |

Every command explains its own semantics:

```bash
orbit <command> --help    # long description, options, and examples
orbit <command> -h        # compact summary
```

The full guide — when to use each command, end-to-end scenarios, and a common-errors table — is in [docs/commands.md](../../docs/commands.md).

## Recovery and maintenance

| Situation | What to do |
| --- | --- |
| Nothing is startable | `NO_READY_TASKS` is a success with `data: null`. Wait, triage, or remove a hold label. |
| Another agent owns the issue | Pick other work, or `orbit claim <id> --by you --steal`. |
| A lease expired | `orbit claim <id> --by you --steal`, or `orbit release <id> --force` to requeue it. |
| Work must be handed off | `orbit checkpoint <id> --by you --progress "..." --next "..."`, then `orbit release <id> --by you`; the next claimant sees it in `ready` and `show`. |
| Dependencies are unfinished | Read `error.details.blocking`, then finish or cancel those IDs. |
| A parent has open children | Finish the children, or pass `--force` to `done`/`cancel` to cancel them. |
| A test failed after a crash | `orbit doctor` reports integrity issues; `doctor --fix` removes stale temp files and re-attaches a deleted `.orbit` worktree. |
| Heartbeats are uncommitted | `orbit sync` commits lease-only changes. |
| The Issue-ID prefix must change | Preview with `orbit slug migrate <slug> --dry-run`, then apply. Live claims require `--force`; external old-ID links still break. |
| The Board is missing | `orbit init` creates one; a missing linked Board returns `LINK_NOT_FOUND`. |
| A pre-ADR-0019 Board or layout needs converting | Preview with `orbit migrate`, then `orbit migrate --apply`; it rewrites `github:` links to `remote:`, `repo:*` labels to the `repository` field, and converts worktree-mode Boards in one commit. |
| A writer is busy | `LOCKED` is retryable. Retry after a short backoff. |

`doctor` never rewrites issues or comments, so it is always safe to run. It works without a Board: it reports the discovery trace, legacy layout, `AMBIGUOUS_BOARD`, `ORPHAN_WORKTREE`, `STORAGE_MISMATCH`, and agent configs that still record an absolute Board path, then checks malformed issues, dangling dependencies, missing or nested parents, parent/ID mismatches, cycles, orphan or corrupt comments, stale temp files, uncommitted changes, the checked-out branch, and a Claude hook that cannot find the Orbit binary. From a Board anchor it additionally reports the resolved Board, member Repository access and cache freshness, migration remnants, duplicate GitHub identities, orphaned links, sync conflicts, and agent setup drift. Plain non-Git member directories are informational warnings rather than health failures; a directory that used to be a repository is still reported as stale. Semantic conflicts are reported with next actions, never resolved by `doctor`.
