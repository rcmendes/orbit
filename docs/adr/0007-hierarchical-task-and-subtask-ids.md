# Hierarchical IDs for Tasks and SubTasks

A Task ID is `<prefix><rand>`, for example `orb-a0b1c2`. A SubTask ID is `<parent-id>.<rand>`, for example `orb-a0b1c2.d4b3df`. The `parent` field in a SubTask's frontmatter is authoritative; the ID is the human-facing label and must agree with it. SubTasks are one level deep: a SubTask cannot own SubTasks, and reparenting is not supported. Legacy IDs such as `orb-5q7bbn` remain valid Tasks and can parent SubTasks without migration. `id.prefix` is restricted to lowercase letters, digits, and dashes so a dot always marks a SubTask.

Parentage is its own edge, not a copied Dependency. A Parent Task is Ready only when every SubTask is done or cancelled; `depends_on` remains reserved for real blockers between units of work.

## Considered options

- **`orb-t-<rand>` and `orb-t-<rand>-st-<rand>`.** Explicit type markers, but two Task grammars (legacy and new) and five more characters per SubTask reference.
- **Opaque child IDs plus a `parent` field.** Short and reparent-safe, but the relationship is invisible in logs, filenames, and pull requests.
- **Title slugs (`orb-t-sso-<rand>`).** Readable, but couples immutable IDs to mutable titles and lengthens every reference.
- **Type-first IDs (`orb-st-<parent>-<rand>`).** Explicit, but the child no longer starts with the parent ID, breaking prefix queries and sort grouping.
- **Copying parentage into `depends_on` and reusing the blocking rule.** Less code, but conflates a feature roll-up with a real blocker and makes `show` unable to tell them apart.

## Consequences

- The `feature:<slug>` label convention is no longer needed for grouping; the parent edge replaces it. `repo:<name>` remains for repository-scoped claiming.
- A prefix query or lexicographic sort groups a Parent Task with its SubTasks.
- Dots in IDs are regex metacharacters, and task files have two dots before `.md` (`orb-a0b1c2.d4b3df.md`). Exact matching and file discovery are unaffected, but humans should use `rg -F` or escape the dot.
- `orbit doctor` reports a parent/ID mismatch, an orphan SubTask, or a nested SubTask.
- `done`, `cancel`, and `delete` guard against open SubTasks; `reopen` of a SubTask reopens a done parent.
