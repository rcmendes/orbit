---
status: accepted
---

# ADR 0022: Issue model and platform architecture

The Orbit platform epic approved a rebuild of the core on market-standard
terminology and contracts, with no backward compatibility. This record fixes
the decisions before code lands; the implementing SubTasks reference it.

**Vocabulary.** The tracker unit is renamed from Task to **Issue**. ADR 0009
("keep Task vocabulary") is superseded: Issue, sub-issue, and epic are now the
canonical words in storage, JSON, MCP, CLI help, and generated agent
instructions, not translations at the boundary. `kind`/`type` metadata from
ADR 0010 is replaced by Issue types.

**Issue types and hierarchy.** Issues carry a type — `epic`, `story`, `task`,
`bug`, or `sub-task` — and a configurable hierarchy (epic above issues above
sub-tasks, with the maximum depth set per Board). `parent` becomes an ordinary
field allowed only where the hierarchy permits; the one-level parent limit and
the structural Task/SubTask split are removed. The structural parts of ADRs
0007 and 0021 are superseded: depth is expressed by the hierarchy, not a
`board.max_subtask_depth` integer. `type: epic` stays top-level only; a
spec/epic container is an Issue of type `epic` with SubTasks or typed links,
never a second container entity.

**Declarative workflow.** The hard-coded state machine in `transition.rs` is
replaced by a Board-defined workflow: named statuses each carrying a category
(`todo`, `active`, `done`, `canceled`), named transitions between them,
resolution recorded when a transition closes an Issue, and `allowed_transitions`
in command output. Three workflow templates ship with Orbit — `minimal`,
`standard`, and `agentic` — and a Board picks or extends one. `ready` becomes an
explicit status (a `Ready` status in the `todo` category) combined with the
derived non-blocked condition: unclaimed-or-leased, not waiting, no hold, and no
open blocking links. Invalid transitions fail with an error that names the
valid ones.

**Priorities.** The integer 0–3 priority is replaced by named, ordered,
configurable priorities — `highest`, `high`, `medium`, `low` by default — stored
and sorted by ordinal so a Board can rename or reorder them.

**Links.** `depends_on` and `waiting_on` are replaced by typed Issue links with
inward and outward names (`blocks`, `relates to`, `duplicates`, `discovered
from`, plus Board-defined types). A link carries a blocking flag and arbitrary
attributes, and may target an external reference instead of an Issue. Blocking
links drive readiness; non-blocking links never do. Cycles are rejected only for
blocking link types.

**People.** Claim is split into **assignee** — a human or agent identity with a
kind — plus an optional time-bounded lease. Humans can be assignees without a
lease. **Reporter** is set at creation and never changes. Stale-lease takeover
keeps the checkpoint handoff.

**Fields.** `area` and `repo` labels become first-class **components** and
**repository** fields, both filterable. Annotations are renamed to
**properties** namespaced by owner. `external_id` records the Issue's identity
in an external system, unique per Board.

**Keys.** Each Issue gets a human- and token-cheap sequential project key
(`ORB-N`) allocated under the Board lock, plus an immutable internal id that
never changes. On Board sync, a key collision re-keys the Issue and records an
alias so the old key keeps resolving.

**Concurrency and idempotency.** Every mutation accepts `--if-version` for
optimistic concurrency; a stale version returns a retryable conflict. Writes
accept an idempotency key; repeating a call with the same key returns the first
result. Batch keeps its all-or-none semantics on top of both.

**One command registry.** Each command is described once — arguments, output
schema, safety hints — and that description generates the clap surface, the MCP
tools (with `readOnly`/`idempotent`/`destructive` annotations and structured
output), JSON Schema, and `docs/commands.md`. A parity test fails the build when
a public command lacks an MCP tool; `orbit schema` publishes the schema.

**Contract.** Errors follow the RFC 9457 problem-details shape: `type`, `title`,
`detail`, `retryable`, and a `hint`. The Board config carries a semver contract
version; `orbit capabilities` is the handshake listing contract version,
modules, and workflow, and an older binary refuses a newer Board. Golden tests
cover the contract surface (`add`, `ready`, `claim`, `show`, `transition`,
`batch`).

**Policy, review, feed, and query.** A declarative policy engine evaluates
conditions, validators, and post-functions at transitions — scoped by type,
label, or profile — covering required fields, evidence, WIP limits, attempt
limits, and a readiness lint. Review adds an `In Review` status with `submit`
and `review` commands: the reviewer must differ from the author when policy is
on, evidence is structured (command, exit code, commit, files), attempts are
counted, and escalation produces `needs_human` or an `ask` that blocks the
Issue. A changelog/event feed derives named events from Board git history with
a monotonic cursor (`orbit changes --since`, `orbit history`). Query adds
qualifier search (`status:`, `assignee:`, `label:`), JSON filters, `--fields`,
cursor pagination, a `prime --max-tokens` budget, and per-Issue digest plus epic
rollup.

**Opt-in modules.** Heavyweight behavior stays out of the kernel. Scheduler
strategies — critical path, touches-overlap avoidance, capability claim, WIP
limits — are modules a Board or caller selects. Command hooks and a derived
search index are built only if a benchmark on a large Board shows the plain scan
is too slow; the index is rebuildable from files and git and hooks never block
a write.

**Profiles.** A declarative profile (TOML) translates a plugin's vocabulary onto
Orbit: `requires`/`optional` capabilities, vocabulary mapping, a workflow
template, and hold/role names. `orbit setup <plugin>` applies a profile
idempotently; `doctor` reports a profile whose requirements are unmet;
capability negotiation errors say what is missing. The `minimal`, `standard`,
and `agentic` workflow templates ship with Orbit.

**Layering.** The architecture has three layers: a frozen-contract **kernel**
(the Issue model, Board storage, lock, atomic publish, command registry,
contract errors, and the query/feed reads), **opt-in modules** (scheduler
strategies, hooks, derived index) loaded only when enabled, and **declarative
profiles** that configure the kernel's vocabulary and workflow per consumer.
Nothing in the kernel depends on a module or profile; a profile cannot add code.

**No backward compatibility.** This is a breaking rebuild, not a migration
shim: no legacy flags, no dual vocabulary, no deprecated fields. A one-shot
`orbit migrate` converts existing Boards — Issues, workflow statuses, links,
properties, keys — with a dry-run preview, and `doctor` verifies the result.

## Considered options

- **Keep Task vocabulary and translate Issue at the boundary (ADR 0009).**
  Rejected: the platform targets market-standard terminology across storage,
  JSON, MCP, and agent instructions; boundary translation was the source of the
  impedance the rebuild removes. ADR 0009's own consequence note allowed this
  as a deliberate v1.0-style breaking change.
- **Backward-compatible rename with a legacy JSON flag.** Rejected by the epic:
  dual vocabulary doubles every schema, envelope, and doc surface, and Orbit's
  Boards are few and migratable. A clean break with `orbit migrate` is cheaper
  than permanent compatibility.
- **Unlimited hierarchy depth.** Rejected: depth stays configurable but bounded
  so IDs, filenames, `tree` output, and readiness walks stay simple.
- **A separate Epic entity.** Rejected again (ADR 0009): an `epic` Issue type
  plus parent/typed links already expresses grouping without a second
  container model.
- **Derived status categories only.** Rejected: categories alone cannot name
  `Ready` vs `In Progress` vs `In Review`, so statuses are Board-defined with a
  fixed category enum underneath.
- **A persistent index as the source of truth.** Rejected: the Board files and
  git history remain the only source of truth; any index is a rebuildable
  module enabled only when measured.
- **Modules compiled in but disabled by config.** Rejected: opt-in modules are
  loaded only when enabled, keeping the kernel's contract frozen and its attack
  and dependency surface minimal.

## Consequences

- ADR 0009 is superseded; ADRs 0007 and 0021 lose their structural depth rules
  to the configurable hierarchy. Earlier ADRs stay for history.
- Every stored shape changes: frontmatter fields, JSON envelopes, MCP tool
  descriptions, CLI help, and generated agent instructions all use Issue
  vocabulary. `orbit export` `schema_version` bumps.
- Existing Boards do not work until `orbit migrate` converts them; there is no
  read-compat path.
- `docs/commands.md`, `docs/SPEC.md`, `docs/GLOSSARY.md`, and this document's
  component map are regenerated or rewritten as each phase lands.
- Consumers (including the kinetic refactor) negotiate the contract through
  `orbit capabilities`; an unsupported Orbit is rejected with the missing
  capabilities listed.
