> **Historical research note.** This document is an early blueprint, kept for context. It is not maintained; the canonical design is [SPEC.md](./SPEC.md) and the current onboarding flow is in [README.md](../README.md).

Here is the complete blueprint and implementation guide for Orbit, synthesizing everything we discussed into a single, copy-pasteable Markdown document.

You can save this as `orbit-spec.md` or `README.md` to serve as the master architecture document for your repository.

```markdown
# Orbit: AI-Native Issue Tracker

Orbit is a zero-overhead, AI-native local issue tracker built in Rust for Apoema Studio. It is specifically designed to power autonomous loops (Cursor, Claude Code, Superpowers) without sacrificing Git compatibility or developer ergonomics.

## 1. Core Architecture

Orbit uses a **Disposable Index** architecture:
1. **Storage (Truth):** Tasks are stored as flat Markdown files in `.tasks/*.md` with YAML frontmatter. This keeps the project Git-native and human-readable.
2. **Indexing (Speed):** On boot (or file change), an in-memory `rusqlite` database is instantly built from the YAML frontmatter.
3. **Execution (Agents):** AI agents query the SQLite cache via CLI (`--json`) or MCP (`stdio`), using zero context tokens to find unblocked work.
4. **Safety (Locking):** All state mutations use `fs3` non-blocking OS file locks and SQLite Recursive CTEs to prevent race conditions and cyclic dependencies.

---

## 2. Project Setup (`Cargo.toml`)

```toml
[workspace]
members = ["core", "cli", "mcp"]

[workspace.dependencies]
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
serde_yaml = "0.9"
clap = { version = "4.4", features = ["derive"] }
rusqlite = "0.40.2"
fs3 = "0.3"
walkdir = "2.5"
notify = "6.1"
notify-debouncer-mini = "0.4"
regex = "1.10"
rand = "0.8"

```

---

## 3. The Ephemeral SQLite Cache & Watcher

The backbone of Orbit is an in-memory database that stays perfectly synced with the `.tasks/` directory.

### Cache Hydration (`core/src/cache.rs`)

```rust
use rusqlite::{params, Connection, Result};
use std::fs;
use walkdir::WalkDir;
use serde::Deserialize;

#[derive(Deserialize)]
struct TaskFrontmatter {
    id: String,
    title: String,
    state: String,
    #[serde(default)]
    depends_on: Vec<String>,
}

pub fn rebuild_cache_in_place(conn: &mut Connection) {
    conn.execute_batch(
        "DROP TABLE IF EXISTS task_dependencies;
         DROP TABLE IF EXISTS tasks;
         CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT NOT NULL, state TEXT NOT NULL);
         CREATE TABLE task_dependencies (task_id TEXT NOT NULL, depends_on_id TEXT NOT NULL, PRIMARY KEY (task_id, depends_on_id));"
    ).unwrap();

    for entry in WalkDir::new(".tasks").into_iter().filter_map(|e| e.ok()) {
        if entry.path().extension().unwrap_or_default() == "md" {
            let content = fs::read_to_string(entry.path()).unwrap();
            let parts: Vec<&str> = content.split("---").collect();
            if parts.len() >= 3 {
                if let Ok(fm) = serde_yaml::from_str::<TaskFrontmatter>(parts[1]) {
                    conn.execute("INSERT INTO tasks (id, title, state) VALUES (?1, ?2, ?3)", params![fm.id, fm.title, fm.state]).unwrap();
                    for dep in fm.depends_on {
                        conn.execute("INSERT INTO task_dependencies (task_id, depends_on_id) VALUES (?1, ?2)", params![fm.id, dep]).unwrap();
                    }
                }
            }
        }
    }
}

```

### Async File Watcher (`core/src/watcher.rs`)

```rust
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

pub fn start_cache_watcher(shared_conn: Arc<Mutex<rusqlite::Connection>>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut debouncer = new_debouncer(Duration::from_millis(250), move |res: Result<_, _>| {
        if let Ok(events) = res { let _ = tx.send(events); }
    }).unwrap();

    debouncer.watcher().watch(std::path::Path::new(".tasks"), RecursiveMode::Recursive).unwrap();

    tokio::spawn(async move {
        let _d = debouncer; 
        while let Some(events) = rx.recv().await {
            let has_md = events.iter().any(|e| e.path.extension().map_or(false, |ext| ext == "md"));
            if has_md {
                let mut conn = shared_conn.lock().unwrap();
                crate::cache::rebuild_cache_in_place(&mut conn);
            }
        }
    });
}

```

---

## 4. File Operations (Add, Transition, Depend, Delete)

Every operation locks the target file and validates logic against the SQLite graph before mutating.

### Atomic Creation (`orbit add`)

Guarantees collision-free IDs and prevents race conditions.

```rust
// Uses OpenOptions::new().write(true).create_new(true).open(&path)
// Creates format: TASK-A7B2.md

```

### Safe Transition (`orbit transition`)

Ensures dependencies are met before a task is marked `done`.

```rust
// 1. SELECT pending dependencies from SQLite
// 2. Lock file: file.try_lock_exclusive()
// 3. Regex replace state: r"(?m)^state:\s*.*$"
// 4. Atomic file overwrite

```

### Cyclic Dependency Protection (`orbit depend`)

Prevents agents from creating infinite execution loops.

```rust
// 1. Query SQLite Recursive CTE:
//    "WITH RECURSIVE traverse(id) AS (...) SELECT id FROM traverse WHERE id = ?2"
// 2. If cycle exists -> Return structured Err("CYCLE_DETECTED")
// 3. Lock file -> Append to YAML depends_on array -> Overwrite

```

### Cascading Deletions (`orbit delete`)

Removes orphaned dependencies before deleting the target file.

```rust
// 1. SELECT dependent tasks from SQLite
// 2. Loop dependents -> Lock -> Remove ID from YAML -> Overwrite
// 3. fs::remove_file(&target_task)

```

---

## 5. Agent Integrations

Orbit speaks natively to your three primary agent surfaces.

### 1. Superpowers CLI (`--json`)

All commands respect the `--json` flag, piping human logs to `stderr` and clean ASTs to `stdout`.

```rust
#[derive(clap::Parser)]
struct Cli {
    #[arg(global = true, long)]
    json: bool,
    // ... subcommands
}

```

### 2. Claude Code MCP Server

An embedded Tokio server handles `stdio` JSON-RPC communication for native tool calls.

```json
// Add to .claude.json:
{
  "mcpServers": {
    "orbit": { "command": "npx", "args": ["@apoemastudio/orbit", "mcp-start"] }
  }
}

```

### 3. Cursor IDE (`orbit init --agents`)

Scaffolds `.cursor/rules/orbit.mdc` automatically to teach Cursor the JSON CLI.

```yaml
---
description: "Rules for interacting with the project's task management and issue tracker"
globs: ["*"]
alwaysApply: false
---
# Orbit Guidelines
1. Fetch work via `orbit list --json`
2. Mutate state exclusively via `orbit transition <ID> <state>`
3. NEVER manually edit YAML frontmatter.

```

### 4. mattpocock-skills Prompt

Paste this when running `/setup-matt-pocock-skills` -> "Other":

> We use a custom AI-native local issue tracker called Orbit. All tasks are stored as Markdown files inside the `.tasks/` directory, utilizing YAML frontmatter for dependency and state metadata. To find actionable, unblocked work, you MUST run `orbit list --json` and parse the output. You are strictly forbidden from manually editing the YAML frontmatter to change a task's status, as this bypasses cyclic dependency checks and OS-level file locking. To change a task state, you must exclusively use the CLI command `orbit transition <TASK_ID> <state>`. To create new tasks, run `orbit add "Task Title"`. You may read the `.tasks/*.md` files directly to understand acceptance criteria, but all state mutations must occur via the `orbit` CLI.

---

## 6. Automated Distribution (`cargo-dist`)

Orbit is instantly deployable across all OS environments without requiring a local Rust toolchain.

1. **Initialize:** `cargo dist init` (Select Shell, PowerShell, and npm).
2. **Release:** Tagging a git commit (`v0.1.0`) triggers GitHub Actions.
3. **Execution:**
* Mac/Linux: `curl -LsSf https://github.com/apoemastudio/orbit/releases/download/... | sh`
* Node Pipelines: `npx @apoemastudio/orbit list --json`



```

<FollowUp label="Want to load the file generator tool?" query="Load the file generator tool so we can export this directly as a downloadable .md file."/>

<ElicitationsGroup message="Next steps:">
  <Elicitation label="Load file generator" query="Load the file generator tool so we can export this directly as a downloadable .md file."/>
  <Elicitation label="Explore the MCP JSON-RPC" query="Drill down into exactly how the MCP JSON-RPC serialization works for the mcp-start command."/>
</ElicitationsGroup>

```
