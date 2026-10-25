# Rewrite IDs when the slug changes

A Board has one literal prefix (`id.prefix`, such as `orb-`) and every Task ID starts with it. The prefix is part of validation: a file whose name does not match the configured prefix is not a valid Task for that Board. That invariant catches copied files and hand-edited config, but it also means renaming a populated Board used to be impossible without editing every file by hand.

The Board's identity can legitimately change: a repository is renamed, a workspace is created under a different product name, or a prototype prefix should become the real one. Keeping the old ID forever is safe for links but leaves the Board with mixed namespaces. Supporting a permanent registry of legacy prefixes would move complexity into every validation, lookup, doctor check, and JSON contract.

## Decision

Use one explicit migration operation, `orbit slug migrate <SLUG>`, with a read-only companion, `orbit slug show`.

- The migration keeps each random suffix and rewrites only the prefix: `orb-a7b2c9` becomes `atlas-a7b2c9`, and `orb-a7b2c9.d4b3df` becomes `atlas-a7b2c9.d4b3df`.
- Structured `parent` and `depends_on` values, comment filenames, the Orbit-generated comment header, and `id.prefix` move with the IDs. Free-form task bodies and comment text are copied verbatim.
- The Board must pass strict loading plus the integrity checks that matter for a rewrite: orphan or corrupt comments, comment-header mismatches, parent/ID mismatches, and the checked-out branch. An unhealthy Board is refused with `MIGRATION_BLOCKED`.
- Live claims require `--force`, because the holder still has the old ID cached. The plan and result list each affected ID, new ID, holder, and lease.
- The plan is available with `--dry-run`, which performs the same checks and writes nothing. The same slug is a no-op.
- The rewrite is built and validated in a temporary detached Git worktree. Orbit commits once as `orbit: migrate task prefix from <old> to <new>`, verifies that the Board branch has not moved, resets the real Board to that commit, and removes the worktree. A failure before the reset leaves the original Board untouched.
- Pending heartbeat-only writes are committed first, so the migration commit is the single rollback point.
- This is a CLI-only operator action. It is deliberately not an MCP tool.

`orbit init --slug` remains the creation and empty-Board path. A populated Board still returns `SLUG_LOCKED`; the message now points at `orbit slug migrate`.

## Considered options

- **Keep every prefix forever (`legacy_prefixes`).** Old links keep working, but every ID validator, parent derivation, doctor check, filter, and export consumer must understand a set of namespaces. The Board also loses its simple one-prefix identity.
- **Create a new Board and copy work.** Clean files, but loses the graph, comments, history, and links in one operation; it is not an in-place rename.
- **Hide the migration behind `init --slug --migrate`.** Fewer commands, but `init` already does onboarding and agent reconciliation. A destructive multi-file rewrite should have its own verb and `--dry-run`.
- **Rewrite free-form text with search and replace.** Simple to implement and unsafe: prose, examples, code blocks, and unrelated IDs can be corrupted. Structured references are rewritten structurally; bodies and comments stay as written.
- **Write every file in place with a journal.** Avoids a temporary worktree but requires recovery logic for every possible interruption. A temporary worktree plus one commit gives a clear transaction boundary with ordinary Git primitives.
- **Generate new random suffixes when IDs collide.** Two IDs mapping to one target should be a rare planning error, not a reason to silently change identity. The migration returns `PREFIX_COLLISION` and writes nothing.

## Consequences

- After migration the Board has one prefix again, with no permanent legacy mode in the CLI.
- External references break immediately: pull requests, chat, docs, agent memory, shell history, and old links still contain the previous IDs. `git revert` restores the files but not those references. Run `--dry-run` first and treat the chosen prefix as immutable afterwards.
- The temporary worktree requires the Board's Git repository to support linked worktrees. Board initialization already uses one, so this adds no new platform requirement.
- `doctor` catches a hand-edited `id.prefix` as `PREFIX_MISMATCH`; migration refuses that state rather than guessing which prefix is intended. Restore the configured prefix first.
- Every future slug change uses the same explicit command, so there is no growing set of accepted prefixes and no alias table to maintain.
