# Project guidance for agents

Shared, durable conventions for work on this repository. These apply to every
agent session unless an Issue explicitly narrows or extends them.

## Components (`component`)

Each Issue may carry one optional `component` value: a short lowercase slug
(letters, digits, `-`) classifying which part of the product the change
primarily lands in. Components are per-Board vocabulary, configured in
`config.toml`:

```toml
[board]
components = ["board", "cli", "mcp", "setup", "docs", "release"]
```

When `board.components` is empty the Board defines no Components and any
lowercase slug is accepted. Do not store personal preferences or session-local
state in `component`.

For this repository the suggested values are:

| Component | Typical scope |
| --- | --- |
| `board` | Board resolution, issue storage, graph, locks, git adapter |
| `cli` | JSON CLI, command handlers, help text |
| `mcp` | MCP stdio server and tool bridge |
| `setup` | `orbit init`, `orbit setup`, harness generation |
| `docs` | User-facing documentation under `docs/` and root guides |
| `release` | Versioning, packaging, CI release automation |

Filter with `orbit list --component cli` or `orbit ready --exclude-component docs`.
The same values appear in JSON (`component`) and in MCP `create`, `update`,
`list`, and `ready`. `orbit triage context` reports the Board's configured
`allowed_components` (`null` when unrestricted).

## Verification guidance

**Project defaults** (run unless an Issue says otherwise):

1. `cargo test` — full test suite must pass before closing work.
2. `cargo fmt --check` and `cargo clippy -- -D warnings` when Rust sources change.
3. After behavior or workflow changes, update the relevant docs (`README.md`,
   `AGENTS.md`, `docs/`) in the same change series.

**Issue-level `verification`** is optional YAML frontmatter set with
`orbit add --verification "..."` or `orbit update --verification "..."`. It holds
concise checks or commands specific to that Issue (for example
`cargo test --test mcp`). It complements narrative acceptance criteria in the
Issue body; it does not replace them.

When both apply, run the project defaults **and** the Issue's `verification`
steps. An Issue may override a default only by stating so explicitly in its
`verification` text.

## What not to store here

Do not put transient execution state, personal tool choices, or session-local
notes in `component` or `verification`. Claims, checkpoints, comments, and labels
remain the right place for workflow state and triage.
