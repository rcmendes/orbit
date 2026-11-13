---
status: accepted
---

# ADR 0018: Remote Trackers behind an Orbit-defined port

Orbit is a local-first tracker: a Board works fully without any Remote Tracker,
and connecting one is an explicit opt-in. The GitHub provider (ADR 0014, ADR
0016) grew inside the core, so GitHub names leaked into Task frontmatter
(`github:`), configuration (`[github]`), the CLI, Triage, and the Workspace
catalog. We decided that Orbit's core owns a provider-independent port in
Orbit's own vocabulary, and each Remote Tracker is an adapter behind it. GitHub
Issues is the first adapter; GitLab, Linear, or Jira would be more adapters,
never changes to the core model.

## Decision

- The core owns the sync engine once, for every Remote Tracker: sync
  baselines, three-way conflict detection, previews before any remote write,
  the Auto-push policy, and the Remote Issue catalog.
- An adapter only translates: fetch, create, and update a Remote Issue in
  Orbit field terms, list a Remote Project's Remote Issues, and declare its
  Mapped fields. Fields an adapter cannot represent stay local-only.
- A Task's link is provider-neutral: it names the Remote Tracker, the Remote
  Project, the Remote Issue identity, and the baseline. A Board may connect to
  several Remote Projects on different Remote Trackers.
- Triage is a core, local feature that works on any Task. Mapped fields only
  limit what can be published afterwards.
- The `github-provider` Cargo feature and `GITHUB_UNSUPPORTED` are removed; the
  Board's configuration is the only switch.

## Considered options

- **Keep GitHub as a built-in feature and hide it when unconfigured.** Rejected:
  it keeps GitHub's model (Issue Fields, `state_reason`, the reserved label) as
  the shape every later tracker must imitate.
- **Let each adapter own its own sync logic.** Rejected: baselines and conflict
  rules would be reimplemented per tracker and drift apart; the robustness
  belongs in one place.
- **A Cargo feature per adapter.** Rejected for now: the GitHub adapter drives
  the `gh` CLI and adds no crate dependencies, so a compile-time switch buys
  nothing a configuration switch does not.

## Consequences

- Existing Boards need a migration: `github:` frontmatter, `[github]`
  configuration, and Remote Issue cache entries move to provider-neutral forms.
- GitHub-only concepts (Issue Fields, basic/full modes, the reserved
  `orbit:in-progress` label, `state_reason`, GitHub Projects) are documented in
  the GitHub adapter's documentation, not the glossary.
- Supersedes the provider-shape parts of ADR 0014; the push-only Auto-push rules
  of ADR 0016 stay and become tracker-independent.

## Known limitation

The port covers the sync engine (`src/remote/sync.rs`, `execution.rs`,
`catalog.rs`). The `orbit remote` command layer (`src/remote/commands.rs`)
still calls the GitHub client directly for setup and diagnostics: `gh` auth
scopes, Issue Field creation and validation, and the reserved
`orbit:in-progress` label in `connect`; `status` and `doctor` share one typed
diagnosis in `src/github/diagnosis.rs` that makes the same direct calls. Adding a second
adapter means first moving those calls behind an adapter setup capability on
`RemoteAdapter`. That move waits for the second adapter, so its shape follows
a real need rather than a guess.
