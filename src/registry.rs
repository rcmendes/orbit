//! Single source of truth for the public command surface.
//!
//! Every public command is described once in [`COMMANDS`]; every MCP tool in
//! [`TOOLS`]. From these tables the crate generates:
//!
//! - the MCP `tools/list` definitions, including `readOnlyHint`,
//!   `destructiveHint`, `idempotentHint`, and `openWorldHint` annotations and
//!   a shared structured-output schema (`orbit_tracker::mcp`),
//! - the JSON Schema document printed by `orbit schema`,
//! - the generated section of `docs/commands.md` (`orbit schema --docs`).
//!
//! The clap surface stays hand-written with derive in [`crate::cli`]; the
//! parity tests at the bottom of this file fail the build when a clap
//! subcommand is missing from [`COMMANDS`], when a command has neither an MCP
//! tool nor an explicit exemption, or when `docs/commands.md` drifts from the
//! generated reference.
//!
//! Meta commands that configure the environment rather than operate on a
//! Board (`init`, `setup`, `mcp`, `schema`) are exempt from MCP exposure;
//! every other public command is reachable through a tool.

use serde_json::{Map, Value, json};

use crate::cli::SchemaArgs;
use crate::error::{OrbitError, Result};
use crate::output::CommandOutput;

/// How one tool argument is rendered on the spawned `orbit` argv. Only used
/// by tools whose [`Tool::custom`] flag is `false`; custom tools translate
/// arguments by hand in `mcp::dispatch_tool`.
#[derive(Clone, Copy)]
pub enum CliTarget {
    /// Positional value, emitted in declaration order. A JSON array emits one
    /// token per item (for example `remote import`'s issue list).
    Positional,
    /// `--flag <value>`; the flag token is stored verbatim (`"--by"`, `"-p"`).
    Long(&'static str),
    /// Bare flag emitted when the argument is `true`.
    Switch(&'static str),
    /// JSON array of strings; each item emits `--flag <item>`.
    Repeat(&'static str),
}

/// The JSON type an argument accepts in `inputSchema` and tool calls.
#[derive(Clone, Copy)]
pub enum JsonType {
    Str,
    Int,
    Bool,
    StrList,
    Obj,
    ObjList,
    /// JSON object or explicit `null` (used by `property`'s `value`).
    ObjOrNull,
}

/// One argument of an MCP tool. `name` is the JSON property name; `cli`
/// describes the argv translation for registry-driven tools and is `None`
/// for [`Tool::custom`] tools.
pub struct Arg {
    pub name: &'static str,
    pub description: &'static str,
    pub ty: JsonType,
    /// Allowed string values for enum-like arguments; empty means unrestricted.
    pub values: &'static [&'static str],
    pub cli: Option<CliTarget>,
    pub required: bool,
}

const fn arg(
    name: &'static str,
    description: &'static str,
    ty: JsonType,
    values: &'static [&'static str],
    cli: Option<CliTarget>,
    required: bool,
) -> Arg {
    Arg {
        name,
        description,
        ty,
        values,
        cli,
        required,
    }
}

// --- Custom-tool argument constructors (cli: None) -------------------------

const fn cstr(name: &'static str, description: &'static str) -> Arg {
    arg(name, description, JsonType::Str, &[], None, false)
}
const fn creq(name: &'static str, description: &'static str) -> Arg {
    arg(name, description, JsonType::Str, &[], None, true)
}
const fn cint(name: &'static str, description: &'static str) -> Arg {
    arg(name, description, JsonType::Int, &[], None, false)
}
const fn cbool(name: &'static str, description: &'static str) -> Arg {
    arg(name, description, JsonType::Bool, &[], None, false)
}
const fn cstrs(name: &'static str, description: &'static str) -> Arg {
    arg(name, description, JsonType::StrList, &[], None, false)
}
const fn cenum(
    name: &'static str,
    description: &'static str,
    values: &'static [&'static str],
    required: bool,
) -> Arg {
    arg(name, description, JsonType::Str, values, None, required)
}
const fn cobj(name: &'static str, description: &'static str) -> Arg {
    arg(name, description, JsonType::Obj, &[], None, false)
}
const fn cobjornull(name: &'static str, description: &'static str) -> Arg {
    arg(name, description, JsonType::ObjOrNull, &[], None, false)
}
const fn cobjlist(name: &'static str, description: &'static str, required: bool) -> Arg {
    arg(name, description, JsonType::ObjList, &[], None, required)
}

// --- Registry-driven argument constructors ---------------------------------

/// Required positional string (`id`, `title`, ...).
const fn pos(name: &'static str, description: &'static str) -> Arg {
    arg(
        name,
        description,
        JsonType::Str,
        &[],
        Some(CliTarget::Positional),
        true,
    )
}

/// Optional positional string.
const fn opt_pos(name: &'static str, description: &'static str) -> Arg {
    arg(
        name,
        description,
        JsonType::Str,
        &[],
        Some(CliTarget::Positional),
        false,
    )
}

/// Positional list of strings; each item becomes one argv token.
const fn pos_list(name: &'static str, description: &'static str) -> Arg {
    arg(
        name,
        description,
        JsonType::StrList,
        &[],
        Some(CliTarget::Positional),
        true,
    )
}

/// `--flag <value>` string option.
const fn long(name: &'static str, description: &'static str, flag: &'static str) -> Arg {
    arg(
        name,
        description,
        JsonType::Str,
        &[],
        Some(CliTarget::Long(flag)),
        false,
    )
}

/// Required `--flag <value>` string option.
const fn long_req(name: &'static str, description: &'static str, flag: &'static str) -> Arg {
    arg(
        name,
        description,
        JsonType::Str,
        &[],
        Some(CliTarget::Long(flag)),
        true,
    )
}

/// Bare `--flag` switch, emitted when the argument is `true`.
const fn switch(name: &'static str, description: &'static str, flag: &'static str) -> Arg {
    arg(
        name,
        description,
        JsonType::Bool,
        &[],
        Some(CliTarget::Switch(flag)),
        false,
    )
}

/// Repeatable `--flag <item>` string array.
const fn repeat(name: &'static str, description: &'static str, flag: &'static str) -> Arg {
    arg(
        name,
        description,
        JsonType::StrList,
        &[],
        Some(CliTarget::Repeat(flag)),
        false,
    )
}

/// `--flag <value>` enum option.
const fn long_enum(
    name: &'static str,
    description: &'static str,
    flag: &'static str,
    values: &'static [&'static str],
) -> Arg {
    arg(
        name,
        description,
        JsonType::Str,
        values,
        Some(CliTarget::Long(flag)),
        false,
    )
}

/// Positional enum option.
const fn pos_enum(
    name: &'static str,
    description: &'static str,
    values: &'static [&'static str],
) -> Arg {
    arg(
        name,
        description,
        JsonType::Str,
        values,
        Some(CliTarget::Positional),
        true,
    )
}

/// One MCP tool. `argv` is the CLI path the tool spawns; for [`Tool::custom`]
/// tools it is informational (the hand-written translation may pick a
/// different subcommand, as `finish` does for `done`/`cancel`).
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub argv: &'static [&'static str],
    pub args: &'static [Arg],
    /// `true` when `mcp::dispatch_tool` translates arguments by hand.
    pub custom: bool,
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    /// `true` when the tool talks to external systems (Remote Trackers, git
    /// hosts) rather than only reading or writing the local Board.
    pub open_world: bool,
}

// The four trailing booleans are the MCP safety hints; keeping them positional
// keeps the spec table readable.
#[allow(clippy::too_many_arguments)]
const fn tool(
    name: &'static str,
    description: &'static str,
    argv: &'static [&'static str],
    args: &'static [Arg],
    custom: bool,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    open_world: bool,
) -> Tool {
    Tool {
        name,
        description,
        argv,
        args,
        custom,
        read_only,
        destructive,
        idempotent,
        open_world,
    }
}

const ID_DESC: &str = "Task ID or project key, for example orb-a7b2c9 or ORB-12.";
const BY_DESC: &str = "Agent identity. Defaults to ORBIT_AGENT, then USER.";
const TYPE_DESC: &str =
    "Issue type name from the Board's `board.types` (defaults: epic, story, task, bug, sub-task).";
const TYPE_VALUES: &[&str] = &["epic", "story", "task", "bug", "sub-task"];
const PRIORITY_DESC: &str =
    "Priority name from the Board's configured list (default: highest, high, medium, low).";
const COMPONENT_DESC: &str =
    "Component slug; restricted to the Board's configured components when set.";

/// Every MCP tool, in `tools/list` order. Custom tools keep their historical
/// names (`get`, `create`, `finish`, ...) so existing MCP clients keep
/// working; registry-driven tools are named after their CLI path.
pub static TOOLS: &[Tool] = &[
    // --- Task queue and reads -------------------------------------------------
    tool(
        "ready",
        "List ready tasks. With claim=true, atomically claim the highest-priority one.",
        &["ready"],
        &[
            cbool("claim", "Claim the first ready task."),
            cstr("by", "Agent identity for the claim."),
            cint("limit", "Maximum tasks to list."),
            cstr("parent", "Only Ready SubTasks of this Parent Task."),
            cstr(
                "repository",
                "Only tasks with this member Repository; matches the `repository` field.",
            ),
            cstrs("labels", "Only tasks carrying every label."),
            cstrs(
                "exclude_labels",
                "Exclude tasks carrying any of these labels.",
            ),
            cenum(
                "types",
                "Only tasks with any of these Issue types.",
                TYPE_VALUES,
                false,
            ),
            cenum(
                "exclude_types",
                "Exclude tasks with any of these Issue types.",
                TYPE_VALUES,
                false,
            ),
            cstrs(
                "components",
                "Only tasks with any of these Component slugs; restricted to the Board's configured components when set.",
            ),
            cstrs(
                "exclude_components",
                "Exclude tasks with any of these Component slugs.",
            ),
            cbool(
                "all",
                "Include Ready tasks from every Repository; default scopes to the current Repository plus Board-wide Tasks.",
            ),
        ],
        true,
        false,
        false,
        false,
        false,
    ),
    tool(
        "list",
        "List tasks with filters. Terminal tasks are hidden unless include_closed=true; all=true widens Repository scope.",
        &["list"],
        &[
            cstr(
                "status",
                "Workflow status name; `orbit show` lists each task's status and allowed transitions.",
            ),
            cstr("priority", PRIORITY_DESC),
            cstr("assignee", "Only tasks assigned to this name."),
            cstr("claimed_by", "Deprecated alias for `assignee`."),
            cbool("waiting", "Only tasks with an external blocker."),
            cstr("label", "Filter by label."),
            cbool("unlabeled", "Only tasks with no labels."),
            cstr("parent", "Only SubTasks of this Parent Task."),
            cstr(
                "repository",
                "Only tasks with this member Repository; matches the `repository` field.",
            ),
            cstrs("labels", "Only tasks carrying every label."),
            cstrs(
                "exclude_labels",
                "Exclude tasks carrying any of these labels.",
            ),
            cenum(
                "types",
                "Only tasks with any of these Issue types.",
                TYPE_VALUES,
                false,
            ),
            cenum(
                "exclude_types",
                "Exclude tasks with any of these Issue types.",
                TYPE_VALUES,
                false,
            ),
            cstrs(
                "components",
                "Only tasks with any of these Component slugs; restricted to the Board's configured components when set.",
            ),
            cstrs(
                "exclude_components",
                "Exclude tasks with any of these Component slugs.",
            ),
            cbool(
                "all",
                "Include tasks from every Repository; default scopes to the current Repository plus Board-wide Tasks.",
            ),
            cbool("include_closed", "Include done and cancelled tasks."),
            cbool("linked", "Only Tasks with an active Remote Issue link."),
            cbool(
                "local",
                "Only Tasks without an active Remote Issue link; includes detached provenance.",
            ),
        ],
        true,
        true,
        false,
        true,
        false,
    ),
    tool(
        "get",
        "Read one task, including its Markdown body and dependents.",
        &["show"],
        &[creq("id", ID_DESC)],
        true,
        true,
        false,
        true,
        false,
    ),
    tool(
        "tree",
        "Show a Task, its SubTasks at every level, and blocking edges.",
        &["tree"],
        &[pos(
            "id",
            "Parent Task ID or project key, for example orb-a7b2c9 or ORB-12.",
        )],
        false,
        true,
        false,
        true,
        false,
    ),
    tool(
        "prime",
        "Print starting context for an agent session: ready work, claims, and counts.",
        &["prime"],
        &[
            long("by", "Agent identity used in the context.", "--by"),
            long(
                "repository",
                "Only Ready tasks with this member Repository; matches the `repository` field.",
                "--repository",
            ),
            repeat(
                "labels",
                "Only Ready tasks carrying every label.",
                "--label",
            ),
            switch(
                "hook_json",
                "Emit Claude Code's SessionStart hook envelope.",
                "--hook-json",
            ),
        ],
        false,
        true,
        false,
        true,
        false,
    ),
    tool(
        "export",
        "Export a deterministic JSON snapshot of the Board.",
        &["export"],
        &[],
        false,
        true,
        false,
        true,
        false,
    ),
    // --- Writes ----------------------------------------------------------------
    tool(
        "create",
        "Create a task. Returns the new task id.",
        &["add"],
        &[
            creq("title", "Task title. Required and must not be empty."),
            cenum("type", TYPE_DESC, TYPE_VALUES, false),
            cstr("priority", PRIORITY_DESC),
            cstrs("labels", "Initial labels."),
            cstrs("depends_on", "Make the new task wait for existing tasks."),
            cstr("parent", "Create a SubTask under this Parent Task."),
            cstr("waiting_on", "External blocker."),
            cstr("component", COMPONENT_DESC),
            cstr(
                "external_id",
                "The task's identity in an external system; unique per Board.",
            ),
            cstr("verification", "Concise checks or commands for agents."),
            cstr(
                "reporter",
                "Reporter recorded at creation; defaults to the caller identity.",
            ),
            cstr("body", "Markdown body and acceptance criteria."),
        ],
        true,
        false,
        false,
        false,
        false,
    ),
    tool(
        "update",
        "Change a task's title, body, priority, component, repository, external id, verification, or external wait. Does not change status.",
        &["update"],
        &[
            creq("id", ID_DESC),
            cstr("title", "New title."),
            cstr("body", "New body."),
            cenum("type", "New Issue type.", TYPE_VALUES, false),
            cstr("priority", PRIORITY_DESC),
            cstr("waiting_on", "New external blocker."),
            cbool("clear_waiting", "Remove the external blocker."),
            cstr("component", COMPONENT_DESC),
            cbool("clear_component", "Remove the Component classification."),
            cstr(
                "repository",
                "New member Repository name; absent means Board-wide.",
            ),
            cbool(
                "clear_repository",
                "Remove the member Repository classification.",
            ),
            cstr("external_id", "New External ID; unique per Board."),
            cbool("clear_external_id", "Remove the External ID."),
            cstr(
                "verification",
                "New verification checks or commands for agents.",
            ),
            cbool(
                "clear_verification",
                "Remove task-level verification guidance.",
            ),
        ],
        true,
        false,
        false,
        true,
        false,
    ),
    tool(
        "claim",
        "Claim a specific task: set the assignee and start a lease.",
        &["claim"],
        &[
            creq("id", ID_DESC),
            cstr("by", "Agent identity."),
            cenum(
                "kind",
                "The assignee's kind; defaults to agent.",
                &["agent", "human"],
                false,
            ),
            cbool("steal", "Take over a live or stale claim."),
        ],
        true,
        false,
        false,
        false,
        false,
    ),
    tool(
        "assign",
        "Assign a task to a person or agent without a lease.",
        &["assign"],
        &[
            creq("id", ID_DESC),
            creq("to", "The assignee's name."),
            cenum(
                "kind",
                "The assignee's kind; defaults to human.",
                &["agent", "human"],
                false,
            ),
            cbool("force", "Replace a different assignee."),
        ],
        true,
        false,
        false,
        true,
        false,
    ),
    tool(
        "release",
        "Release a claim and return the task to todo.",
        &["release"],
        &[
            creq("id", ID_DESC),
            cstr("by", BY_DESC),
            cbool("force", "Release another agent's claim."),
        ],
        true,
        false,
        false,
        false,
        false,
    ),
    tool(
        "heartbeat",
        "Extend a claim's lease while working.",
        &["heartbeat"],
        &[creq("id", ID_DESC), cstr("by", BY_DESC)],
        true,
        false,
        false,
        false,
        false,
    ),
    tool(
        "checkpoint",
        "Record or update the active work checkpoint on a claimed task.",
        &["checkpoint"],
        &[
            creq("id", ID_DESC),
            cstr(
                "progress",
                "Current progress. Required for the first checkpoint.",
            ),
            cstr("next", "Next useful action for the next claimant."),
            cbool("clear_next", "Remove the next action."),
            cstr("blocker", "A blocker the next claimant must know about."),
            cbool("clear_blocker", "Remove the blocker."),
            cstr("by", "Agent identity; must match the claim holder."),
        ],
        true,
        false,
        false,
        true,
        false,
    ),
    tool(
        "finish",
        "Complete or cancel a task (`orbit done` or `orbit cancel`). Done still requires finished dependencies.",
        &["done", "cancel"],
        &[
            creq("id", ID_DESC),
            cenum("status", "Target status.", &["done", "cancelled"], true),
            cstr("by", BY_DESC),
            cbool(
                "force",
                "Override a competing live claim; never bypasses dependencies.",
            ),
            cstr(
                "comment",
                "Completion note recorded atomically with the status change.",
            ),
        ],
        true,
        false,
        false,
        false,
        false,
    ),
    tool(
        "reopen",
        "Reopen a done or cancelled task.",
        &["reopen"],
        &[
            pos("id", ID_DESC),
            switch(
                "force",
                "Reopen even when a dependent task is already done.",
                "--force",
            ),
        ],
        false,
        false,
        false,
        false,
        false,
    ),
    tool(
        "transition",
        "Run a named workflow transition on a task; `get` reports each task's allowed_transitions.",
        &["transition"],
        &[
            creq("id", ID_DESC),
            creq(
                "name",
                "Workflow transition name, for example promote, block, or defer.",
            ),
            cstr("by", "Caller identity for Claim-holder checks."),
            cbool(
                "force",
                "Run even when another holder's live Claim blocks the transition.",
            ),
        ],
        true,
        false,
        false,
        false,
        false,
    ),
    tool(
        "link",
        "Add, remove, or list typed links. Without `type`/`external`, add/remove manage the blocking `blocks` link between two tasks (depend/undepend).",
        &["link", "depend", "undepend"],
        &[
            creq("id", ID_DESC),
            cstr(
                "dep_id",
                "Orbit Task ID or project key target; omit when `external` is given or action is `list`.",
            ),
            cstr(
                "external",
                "External reference target as SYSTEM:ID, for example jira:PROJ-123.",
            ),
            cenum(
                "action",
                "Link operation.",
                &["add", "remove", "list"],
                true,
            ),
            cstr(
                "type",
                "Link type: blocks, relates-to, duplicates, discovered-from, or a custom slug. Defaults to blocks for task targets.",
            ),
            cstr("url", "Canonical URL for an external target."),
            cbool(
                "blocking",
                "Override the link type's default blocking behavior.",
            ),
            cobj(
                "attributes",
                "Arbitrary key/value attributes stored on the link.",
            ),
        ],
        true,
        false,
        false,
        false,
        false,
    ),
    tool(
        "delete",
        "Delete a task. With force=true, clean up dependent tasks first.",
        &["delete"],
        &[
            creq("id", ID_DESC),
            cstr("by", BY_DESC),
            cbool(
                "force",
                "Delete even when dependents exist, or another agent holds a live claim.",
            ),
        ],
        true,
        false,
        true,
        false,
        false,
    ),
    tool(
        "label",
        "Add, remove, or list task labels.",
        &["label"],
        &[
            cenum(
                "action",
                "Label operation.",
                &["add", "remove", "list"],
                true,
            ),
            cstr("id", ID_DESC),
            cstrs("labels", "One or more lowercase labels."),
        ],
        true,
        false,
        false,
        false,
        false,
    ),
    tool(
        "comment",
        "Add or list task comments.",
        &["comment"],
        &[
            cenum("action", "Comment operation.", &["add", "list"], true),
            cstr("id", ID_DESC),
            cstr("body", "Comment body."),
            cstr("by", "Comment author."),
        ],
        true,
        false,
        false,
        false,
        false,
    ),
    tool(
        "property",
        "Set a tool's Property (a JSON object, namespaced by owner) on a task, or remove it with value null.",
        &["property"],
        &[
            creq("id", ID_DESC),
            creq(
                "owner",
                "Owner: the lowercase slug naming the tool, for example ci.",
            ),
            cobjornull(
                "value",
                "The Property, a JSON object of at most 8192 bytes; null removes it.",
            ),
        ],
        true,
        false,
        false,
        true,
        false,
    ),
    tool(
        "batch",
        "Apply many task operations in one commit, all or none. Same operations as `orbit batch`.",
        &["batch"],
        &[
            cobjlist(
                "operations",
                "Batch operations: add, update, label, depend, undepend, link, claim, release, heartbeat, checkpoint, done, cancel, reopen, delete, annotate, comment, property.",
                true,
            ),
            cbool(
                "dry_run",
                "Validate every operation and report the result without writing.",
            ),
            cstr("by", "Default agent for operations without `by`."),
        ],
        true,
        false,
        false,
        false,
        false,
    ),
    // --- Board maintenance ------------------------------------------------------
    tool(
        "doctor",
        "Check the Board for integrity issues; fix=true removes stale temp files and re-attaches an orphaned orbit worktree.",
        &["doctor"],
        &[switch(
            "fix",
            "Remove stale temp files and re-attach an orphaned orbit worktree.",
            "--fix",
        )],
        false,
        false,
        false,
        true,
        false,
    ),
    tool(
        "sync",
        "Commit pending lease-only changes to the board branch.",
        &["sync"],
        &[],
        false,
        false,
        false,
        true,
        false,
    ),
    // --- Repositories ------------------------------------------------------------
    tool(
        "repo_add",
        "Add a Repository as an explicit member of this Board.",
        &["repo", "add"],
        &[
            opt_pos(
                "path",
                "Repository path or its main checkout. Defaults to the current directory.",
            ),
            long(
                "name",
                "Member name Tasks record in their `repo` field.",
                "--name",
            ),
        ],
        false,
        false,
        false,
        false,
        false,
    ),
    tool(
        "repo_remove",
        "Remove an explicitly added member Repository.",
        &["repo", "remove"],
        &[pos(
            "repository",
            "Member name or repository path recorded by `orbit repo add`.",
        )],
        false,
        false,
        false,
        false,
        false,
    ),
    tool(
        "repo_list",
        "List the Board's member Repositories.",
        &["repo", "list"],
        &[],
        false,
        true,
        false,
        true,
        false,
    ),
    tool(
        "repo_refresh",
        "Rescan child Repositories and rebuild the Repository inventory.",
        &["repo", "refresh"],
        &[],
        false,
        false,
        false,
        true,
        false,
    ),
    // --- Slug and migration -------------------------------------------------------
    tool(
        "slug_show",
        "Show the Board's slug, literal prefix, and task counts.",
        &["slug", "show"],
        &[],
        false,
        true,
        false,
        true,
        false,
    ),
    tool(
        "slug_migrate",
        "Rewrite every Task and SubTask ID to a new slug.",
        &["slug", "migrate"],
        &[
            pos("slug", "New slug. Bare word: atlas, not atlas-."),
            switch(
                "dry_run",
                "Print the migration plan without writing any files.",
                "--dry-run",
            ),
            switch(
                "force",
                "Migrate even while tasks have live claims, then list the affected holders.",
                "--force",
            ),
        ],
        false,
        false,
        true,
        false,
        false,
    ),
    tool(
        "migrate",
        "Convert a pre-ADR-0019 Board into a standalone Board folder. Previews by default; apply=true converts.",
        &["migrate"],
        &[
            switch(
                "apply",
                "Apply the migration. Without this flag the command only previews.",
                "--apply",
            ),
            switch(
                "dry_run",
                "Preview the migration without writing anything (the default).",
                "--dry-run",
            ),
        ],
        false,
        false,
        true,
        false,
        false,
    ),
    tool(
        "migrate_model",
        "Rewrite a pre-ADR-0022 Board's Tasks to the Issue model: workflow statuses, named priorities, typed links, properties, components/repository fields, Issue types, and project keys. Previews by default; apply=true rewrites in one commit.",
        &["migrate", "model"],
        &[
            switch(
                "apply",
                "Apply the migration. Without this flag the command only previews.",
                "--apply",
            ),
            switch(
                "dry_run",
                "Preview the migration without writing anything (the default).",
                "--dry-run",
            ),
        ],
        false,
        false,
        true,
        false,
        false,
    ),
    // --- Board host ---------------------------------------------------------------
    tool(
        "board_host",
        "Show, set, or clear the Board host repository. With no target the current host is shown; none=true clears it.",
        &["board", "host"],
        &[
            opt_pos(
                "target",
                "Member Repository name/path or git URL to host the Board.",
            ),
            switch(
                "none",
                "Clear the host without deleting the remote `orbit` branch.",
                "--none",
            ),
        ],
        false,
        false,
        false,
        false,
        true,
    ),
    tool(
        "board_sync",
        "Fetch the host, merge Orbit-aware, and push the `orbit` branch.",
        &["board", "sync"],
        &[repeat(
            "resolutions",
            "Resolve a field conflict as `FIELD=local` or `FIELD=host`. Repeat as needed.",
            "--resolve",
        )],
        false,
        false,
        false,
        false,
        true,
    ),
    tool(
        "board_move",
        "Move the Board folder to DIR and repoint every anchor at it.",
        &["board", "move"],
        &[pos(
            "dir",
            "New Board folder; must not exist or be an empty directory.",
        )],
        false,
        false,
        true,
        false,
        false,
    ),
    // --- Remote Trackers -----------------------------------------------------------
    tool(
        "remote_status",
        "Report Remote Tracker health without changing the Board or tracker.",
        &["remote", "status"],
        &[long(
            "repo",
            "Limit checks to Linked Tasks on one repository (`owner/repo`).",
            "--repo",
        )],
        false,
        true,
        false,
        true,
        true,
    ),
    tool(
        "remote_connect",
        "Detect the origin Remote Project and connect its Remote Tracker, or configure the connected tracker's mapped fields.",
        &["remote", "connect"],
        &[
            opt_pos(
                "project",
                "Remote Project to connect: `TRACKER:PROJECT` or `PROJECT` when the tracker is unambiguous. Omit to detect the `origin` remote instead.",
            ),
            switch(
                "fields",
                "Configure the connected Remote Tracker's mapped fields instead of detecting `origin`.",
                "--fields",
            ),
            switch(
                "validate_only",
                "Validate configuration without writing field ids.",
                "--validate-only",
            ),
            switch(
                "create_fields",
                "Create missing organization Issue Fields (requires organization admin access).",
                "--create-fields",
            ),
        ],
        false,
        false,
        false,
        false,
        true,
    ),
    tool(
        "remote_disconnect",
        "Disconnect a Remote Project from this Board.",
        &["remote", "disconnect"],
        &[pos(
            "project",
            "Remote Project identity: `TRACKER:PROJECT` or `PROJECT` when the tracker is unambiguous.",
        )],
        false,
        false,
        true,
        false,
        true,
    ),
    tool(
        "remote_list",
        "List the Remote Projects connected to this Board.",
        &["remote", "list"],
        &[],
        false,
        true,
        false,
        true,
        false,
    ),
    tool(
        "remote_issues",
        "Browse and list cached Remote Issues without fetching their bodies.",
        &["remote", "issues"],
        &[
            long("tracker", "Filter by Remote Tracker.", "--tracker"),
            long("project", "Filter by Remote Project.", "--project"),
            long_enum(
                "state",
                "Filter by Remote Issue state.",
                "--state",
                &["open", "closed"],
            ),
            repeat(
                "labels",
                "Require each label. Repeat to require more than one label.",
                "--label",
            ),
            long(
                "text",
                "Case-insensitive text search in issue titles.",
                "--text",
            ),
            switch(
                "linked",
                "Show only Issues linked to Orbit Tasks.",
                "--linked",
            ),
            switch(
                "unlinked",
                "Show only Issues without an Orbit Task link.",
                "--unlinked",
            ),
        ],
        false,
        true,
        false,
        true,
        false,
    ),
    tool(
        "remote_issues_refresh",
        "Refresh every connected Remote Project's catalog snapshot.",
        &["remote", "issues", "refresh"],
        &[],
        false,
        false,
        false,
        true,
        true,
    ),
    tool(
        "remote_issues_show",
        "Show one Remote Issue, fetched live from its Remote Tracker.",
        &["remote", "issues", "show"],
        &[pos(
            "issue",
            "Remote Issue identity: `TRACKER:PROJECT#ID`, `PROJECT#ID`, or a Remote Tracker Issue URL.",
        )],
        false,
        true,
        false,
        true,
        true,
    ),
    tool(
        "remote_import",
        "Preview selected Remote Issues or explicitly apply their import. The tracker is never written.",
        &["remote", "import"],
        &[
            pos_list("issues", "Explicit Remote Issue identities to review."),
            repeat(
                "links",
                "Link this selected Issue to a specific Local Task. Repeat as IDENTITY=TASK.",
                "--link",
            ),
            switch(
                "apply",
                "Apply the reviewed selection to the Orbit Board.",
                "--apply",
            ),
        ],
        false,
        false,
        false,
        false,
        true,
    ),
    tool(
        "remote_link",
        "Link a Local Task to a Remote Issue.",
        &["remote", "link"],
        &[
            pos("task", "Local Task ID or project key."),
            pos(
                "issue",
                "Remote Issue identity or URL: `TRACKER:PROJECT#ID`, `PROJECT#ID`, or a Remote Tracker Issue URL.",
            ),
        ],
        false,
        false,
        false,
        false,
        true,
    ),
    tool(
        "remote_create",
        "Preview or create Remote Issues for selected Local Tasks.",
        &["remote", "create"],
        &[
            pos_list("tasks", "One or more Local Task IDs to export."),
            long(
                "project",
                "Choose a connected Remote Project (inferred only when one is available).",
                "--project",
            ),
            switch(
                "apply",
                "Create Issues and link Tasks. Without this flag, show a read-only preview.",
                "--apply",
            ),
            switch(
                "include_closed",
                "Include done and cancelled Tasks in the export.",
                "--include-closed",
            ),
        ],
        false,
        false,
        false,
        false,
        true,
    ),
    tool(
        "remote_detach",
        "Preview or detach a Linked Task from its Remote Issue.",
        &["remote", "detach"],
        &[
            pos(
                "target",
                "Remote Issue identity (`TRACKER:PROJECT#ID`, `PROJECT#ID`) or a Task id whose Remote link should be detached.",
            ),
            switch(
                "apply",
                "Apply the reviewed operation. Without this flag, nothing is written.",
                "--apply",
            ),
        ],
        false,
        false,
        false,
        false,
        true,
    ),
    tool(
        "remote_reattach",
        "Preview or reattach a Local Task by its inactive Remote provenance.",
        &["remote", "reattach"],
        &[
            pos(
                "target",
                "Remote Issue identity (`TRACKER:PROJECT#ID`, `PROJECT#ID`) or a Task id whose Remote link should be reattached.",
            ),
            switch(
                "apply",
                "Apply the reviewed operation. Without this flag, nothing is written.",
                "--apply",
            ),
        ],
        false,
        false,
        false,
        false,
        true,
    ),
    tool(
        "remote_sync",
        "Preview or apply synchronization for Linked Tasks.",
        &["remote", "sync"],
        &[
            opt_pos(
                "task",
                "Local Task ID or project key to sync; omit for all Linked Tasks.",
            ),
            long_enum(
                "direction",
                "Sync direction.",
                "--direction",
                &["both", "pull", "push"],
            ),
            switch(
                "apply",
                "Apply the planned sync. Without this flag the command is a dry run.",
                "--apply",
            ),
            repeat(
                "resolutions",
                "Resolve a field conflict as `FIELD=orbit` or `FIELD=remote`. Repeat as needed.",
                "--resolve",
            ),
        ],
        false,
        false,
        false,
        false,
        true,
    ),
    tool(
        "remote_auto_push",
        "Enable or disable automatic push of Task changes to Remote Issues.",
        &["remote", "auto-push"],
        &[pos_enum(
            "state",
            "Turn automatic push on or off.",
            &["on", "off"],
        )],
        false,
        false,
        false,
        true,
        true,
    ),
    // --- Triage --------------------------------------------------------------------
    tool(
        "triage_context",
        "Load task context, project guidance, and allowed triage fields.",
        &["triage", "context"],
        &[pos("task", "Task ID or project key.")],
        false,
        true,
        false,
        true,
        false,
    ),
    tool(
        "triage_review",
        "Validate and review a proposal JSON file or stdin without publishing.",
        &["triage", "review"],
        &[
            pos("task", "Task ID or project key."),
            long_req(
                "proposal_file",
                "Proposal JSON path on this machine, or `-` is not supported over MCP; pass a file path.",
                "--proposal-file",
            ),
        ],
        false,
        true,
        false,
        true,
        false,
    ),
    tool(
        "triage_apply",
        "Apply an approved proposal to Orbit only; approve=true is required after reviewing the preview.",
        &["triage", "apply"],
        &[
            pos("task", "Task ID or project key."),
            long_req(
                "proposal_file",
                "Proposal JSON path on this machine.",
                "--proposal-file",
            ),
            switch(
                "approve",
                "Required after reviewing the preview; writes Orbit metadata only.",
                "--approve",
            ),
        ],
        false,
        false,
        false,
        false,
        false,
    ),
];

/// Every leaf of the clap `Command` tree. `tool` names the MCP tool that
/// exposes the command; `exempt` explains why a meta command has no tool.
/// Exactly one of the two must be set (enforced by the parity tests).
pub struct CommandEntry {
    /// CLI path tokens, e.g. `&["remote", "issues", "refresh"]`.
    pub path: &'static [&'static str],
    pub summary: &'static str,
    pub tool: Option<&'static str>,
    pub exempt: Option<&'static str>,
}

const fn cmd(
    path: &'static [&'static str],
    summary: &'static str,
    tool: &'static str,
) -> CommandEntry {
    CommandEntry {
        path,
        summary,
        tool: Some(tool),
        exempt: None,
    }
}

const fn exempt(
    path: &'static [&'static str],
    summary: &'static str,
    reason: &'static str,
) -> CommandEntry {
    CommandEntry {
        path,
        summary,
        tool: None,
        exempt: Some(reason),
    }
}

pub static COMMANDS: &[CommandEntry] = &[
    exempt(
        &["init"],
        "Create the Board and wire coding-agent harnesses.",
        "creates Boards and agent harnesses; not a Board operation",
    ),
    cmd(&["repo", "add"], "Add a member Repository.", "repo_add"),
    cmd(
        &["repo", "remove"],
        "Remove a member Repository.",
        "repo_remove",
    ),
    cmd(&["repo", "list"], "List member Repositories.", "repo_list"),
    cmd(
        &["repo", "refresh"],
        "Rescan child Repositories.",
        "repo_refresh",
    ),
    cmd(&["slug", "show"], "Show the Board's slug.", "slug_show"),
    cmd(
        &["slug", "migrate"],
        "Rewrite every Task ID to a new slug.",
        "slug_migrate",
    ),
    cmd(&["migrate"], "Convert a pre-ADR-0019 Board.", "migrate"),
    cmd(
        &["migrate", "model"],
        "Convert a Board to the Issue model.",
        "migrate_model",
    ),
    cmd(&["add"], "Create a task.", "create"),
    cmd(&["list"], "List tasks with filters.", "list"),
    cmd(&["ready"], "List or claim the next ready task.", "ready"),
    cmd(&["show"], "Show a task.", "get"),
    cmd(&["tree"], "Show a Task tree.", "tree"),
    cmd(&["claim"], "Claim a task.", "claim"),
    cmd(&["assign"], "Assign without a lease.", "assign"),
    cmd(&["release"], "Release a claim.", "release"),
    cmd(&["heartbeat"], "Extend a lease.", "heartbeat"),
    cmd(&["checkpoint"], "Record a work checkpoint.", "checkpoint"),
    cmd(&["done"], "Complete a claimed task.", "finish"),
    cmd(&["cancel"], "Cancel a task.", "finish"),
    cmd(&["reopen"], "Reopen a closed task.", "reopen"),
    cmd(&["transition"], "Run a workflow transition.", "transition"),
    cmd(&["update"], "Edit a task.", "update"),
    cmd(&["label", "add"], "Add labels.", "label"),
    cmd(&["label", "remove"], "Remove labels.", "label"),
    cmd(&["label", "list"], "List labels.", "label"),
    cmd(&["batch"], "Apply many operations.", "batch"),
    cmd(&["property", "set"], "Set a Property.", "property"),
    cmd(&["property", "remove"], "Remove a Property.", "property"),
    cmd(&["comment", "add"], "Add a comment.", "comment"),
    cmd(&["comment", "list"], "List comments.", "comment"),
    cmd(&["depend"], "Make a task wait for another.", "link"),
    cmd(&["undepend"], "Remove a dependency.", "link"),
    cmd(&["link", "add"], "Add a typed link.", "link"),
    cmd(&["link", "remove"], "Remove typed links.", "link"),
    cmd(&["link", "list"], "List typed links.", "link"),
    cmd(&["delete"], "Delete a task.", "delete"),
    cmd(&["prime"], "Print session context.", "prime"),
    cmd(&["export"], "Export a JSON snapshot.", "export"),
    exempt(
        &["setup"],
        "Install, check, or remove one agent integration.",
        "writes agent harness configuration; not a Board operation",
    ),
    exempt(
        &["mcp"],
        "Serve the MCP tools and resources over stdio.",
        "is the MCP server itself",
    ),
    cmd(&["doctor"], "Check Board integrity.", "doctor"),
    cmd(&["sync"], "Commit pending lease-only changes.", "sync"),
    exempt(
        &["schema"],
        "Print the command registry schema.",
        "publishes this registry; not a Board operation",
    ),
    cmd(
        &["board", "host"],
        "Show, set, or clear the Board host.",
        "board_host",
    ),
    cmd(&["board", "sync"], "Sync the Board host.", "board_sync"),
    cmd(&["board", "move"], "Move the Board folder.", "board_move"),
    cmd(
        &["remote", "status"],
        "Report Remote Tracker health.",
        "remote_status",
    ),
    cmd(
        &["remote", "connect"],
        "Connect a Remote Tracker.",
        "remote_connect",
    ),
    cmd(
        &["remote", "disconnect"],
        "Disconnect a Remote Project.",
        "remote_disconnect",
    ),
    cmd(&["remote", "list"], "List Remote Projects.", "remote_list"),
    cmd(
        &["remote", "issues"],
        "List cached Remote Issues.",
        "remote_issues",
    ),
    cmd(
        &["remote", "issues", "refresh"],
        "Refresh the Remote Issue catalog.",
        "remote_issues_refresh",
    ),
    cmd(
        &["remote", "issues", "list"],
        "List cached Remote Issues.",
        "remote_issues",
    ),
    cmd(
        &["remote", "issues", "show"],
        "Show one Remote Issue.",
        "remote_issues_show",
    ),
    cmd(
        &["remote", "import"],
        "Import Remote Issues.",
        "remote_import",
    ),
    cmd(
        &["remote", "link"],
        "Link a Task to a Remote Issue.",
        "remote_link",
    ),
    cmd(
        &["remote", "create"],
        "Create Remote Issues for Tasks.",
        "remote_create",
    ),
    cmd(
        &["remote", "detach"],
        "Detach a Task from its Remote Issue.",
        "remote_detach",
    ),
    cmd(
        &["remote", "reattach"],
        "Reattach a Task to its Remote Issue.",
        "remote_reattach",
    ),
    cmd(
        &["remote", "sync"],
        "Synchronize Linked Tasks.",
        "remote_sync",
    ),
    cmd(
        &["remote", "auto-push"],
        "Enable or disable auto-push.",
        "remote_auto_push",
    ),
    cmd(
        &["triage", "context"],
        "Load triage context.",
        "triage_context",
    ),
    cmd(
        &["triage", "review"],
        "Review a triage proposal.",
        "triage_review",
    ),
    cmd(
        &["triage", "apply"],
        "Apply a triage proposal.",
        "triage_apply",
    ),
];

fn find_tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
}

fn arg_schema(arg: &Arg) -> Value {
    let mut schema = match arg.ty {
        JsonType::Str => json!({ "type": "string" }),
        JsonType::Int => json!({ "type": "integer" }),
        JsonType::Bool => json!({ "type": "boolean" }),
        JsonType::StrList => json!({ "type": "array", "items": { "type": "string" } }),
        JsonType::Obj => json!({ "type": "object" }),
        JsonType::ObjList => json!({ "type": "array", "items": { "type": "object" } }),
        JsonType::ObjOrNull => json!({ "type": ["object", "null"] }),
    };
    if !arg.values.is_empty() {
        schema["enum"] = json!(arg.values);
    }
    if !arg.description.is_empty() {
        schema["description"] = json!(arg.description);
    }
    schema
}

/// The JSON Schema every tool result's `structuredContent` satisfies: the
/// Orbit envelope (`{"ok": true, "data": ...}` or `{"ok": false, "error": ...}`).
pub fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" },
            "data": {},
            "meta": {},
            "warnings": { "type": "array", "items": { "type": "string" } },
            "error": {
                "type": "object",
                "properties": {
                    "code": { "type": "string" },
                    "message": { "type": "string" },
                    "retryable": { "type": "boolean" },
                    "details": {}
                },
                "required": ["code", "message", "retryable"]
            }
        },
        "required": ["ok"]
    })
}

fn annotations(tool: &Tool) -> Value {
    json!({
        "readOnlyHint": tool.read_only,
        "destructiveHint": tool.destructive,
        "idempotentHint": tool.idempotent,
        "openWorldHint": tool.open_world
    })
}

fn input_schema(tool: &Tool) -> Value {
    let mut properties = Map::new();
    for arg in tool.args {
        properties.insert(arg.name.to_string(), arg_schema(arg));
    }
    let required: Vec<&str> = tool
        .args
        .iter()
        .filter(|arg| arg.required)
        .map(|arg| arg.name)
        .collect();
    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": required,
        "additionalProperties": false
    })
}

/// The MCP `tools/list` entry for one tool, including annotations and the
/// structured-output schema.
pub fn tool_definition(tool: &Tool) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": input_schema(tool),
        "outputSchema": output_schema(),
        "annotations": annotations(tool)
    })
}

/// All MCP tool definitions, generated from [`TOOLS`].
pub fn tool_definitions() -> Vec<Value> {
    TOOLS.iter().map(tool_definition).collect()
}

fn stringify(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Build the `orbit` argv for a registry-driven (non-custom) tool.
///
/// Returns `Err` with a human-readable reason when a required argument is
/// missing, an argument has the wrong JSON shape, or the caller passed a
/// property the tool does not declare.
pub fn tool_argv(name: &str, arguments: &Value) -> std::result::Result<Vec<String>, String> {
    let Some(tool) = TOOLS.iter().find(|tool| tool.name == name && !tool.custom) else {
        return Err(format!("unknown tool `{name}`"));
    };
    let object = arguments.as_object().cloned().unwrap_or_default();
    for key in object.keys() {
        if !tool.args.iter().any(|arg| arg.name == key) {
            return Err(format!("unknown argument `{key}` for tool `{name}`"));
        }
    }

    let mut argv: Vec<String> = tool.argv.iter().map(|token| token.to_string()).collect();
    for arg in tool.args {
        let value = object.get(arg.name).filter(|value| !value.is_null());
        let Some(cli) = arg.cli else {
            continue;
        };
        match cli {
            CliTarget::Positional => match value {
                None => {
                    if arg.required {
                        return Err(format!("missing `{}`", arg.name));
                    }
                }
                Some(Value::Array(items)) => {
                    for item in items {
                        let Some(text) = stringify(item) else {
                            return Err(format!("`{}` must be a string", arg.name));
                        };
                        argv.push(text);
                    }
                }
                Some(value) => {
                    let Some(text) = stringify(value) else {
                        return Err(format!("`{}` must be a string", arg.name));
                    };
                    argv.push(text);
                }
            },
            CliTarget::Long(flag) => match value {
                None => {
                    if arg.required {
                        return Err(format!("missing `{}`", arg.name));
                    }
                }
                Some(value) => {
                    let Some(text) = stringify(value) else {
                        return Err(format!("`{}` must be a string or number", arg.name));
                    };
                    argv.push(flag.to_string());
                    argv.push(text);
                }
            },
            CliTarget::Switch(flag) => {
                if value.and_then(Value::as_bool) == Some(true) {
                    argv.push(flag.to_string());
                }
            }
            CliTarget::Repeat(flag) => match value {
                None => {
                    if arg.required {
                        return Err(format!("missing `{}`", arg.name));
                    }
                }
                Some(Value::Array(items)) => {
                    for item in items {
                        let Some(text) = stringify(item) else {
                            return Err(format!("`{}` must be an array of strings", arg.name));
                        };
                        argv.push(flag.to_string());
                        argv.push(text);
                    }
                }
                Some(_) => return Err(format!("`{}` must be an array of strings", arg.name)),
            },
        }
    }
    Ok(argv)
}

fn command_path(entry: &CommandEntry) -> String {
    format!("orbit {}", entry.path.join(" "))
}

fn command_label(entry: &CommandEntry) -> String {
    format!("`{}`", command_path(entry))
}

fn tool_for(entry: &CommandEntry) -> Option<&'static Tool> {
    entry.tool.and_then(find_tool)
}

/// One command's entry in the `orbit schema` document.
fn command_schema(entry: &CommandEntry) -> Value {
    let mut schema = json!({
        "command": command_path(entry),
        "path": entry.path,
        "summary": entry.summary,
    });
    match (entry.tool, entry.exempt) {
        (Some(tool_name), _) => {
            if let Some(tool) = find_tool(tool_name) {
                schema["mcp_tool"] = json!(tool.name);
                schema["inputSchema"] = input_schema(tool);
                schema["outputSchema"] = output_schema();
                schema["annotations"] = annotations(tool);
            }
        }
        (None, Some(reason)) => {
            schema["mcp_tool"] = Value::Null;
            schema["mcp_exempt"] = json!(reason);
        }
        (None, None) => {}
    }
    schema
}

/// The JSON Schema document printed by `orbit schema`: every public command
/// with its CLI path, summary, MCP tool mapping, argument schema, safety
/// annotations, and the shared output schema.
pub fn schema_document() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "orbit command registry",
        "orbit_version": env!("CARGO_PKG_VERSION"),
        "contract": "orbit-command-registry/1",
        "commands": COMMANDS.iter().map(command_schema).collect::<Vec<_>>(),
        "tools": TOOLS.iter().map(tool_definition).collect::<Vec<_>>()
    })
}

/// Resolve a `orbit schema <filter>` argument: a command path
/// (`remote issues refresh`), a bare command name (`ready`), or an MCP tool
/// name (`create`). Full-path matches win over bare-name matches so `add`
/// resolves to `orbit add`, not `orbit repo add`.
pub fn schema_lookup(filter: &str) -> Result<Value> {
    let wanted = filter.trim();
    for entry in COMMANDS {
        if entry.path.join(" ") == wanted {
            return Ok(json!({ "command": command_schema(entry) }));
        }
    }
    for tool in TOOLS {
        if tool.name == wanted {
            return Ok(json!({ "tool": tool_definition(tool) }));
        }
    }
    for entry in COMMANDS {
        if entry.path.last() == Some(&wanted) {
            return Ok(json!({ "command": command_schema(entry) }));
        }
    }
    Err(OrbitError::invalid_argument(format!(
        "unknown command or tool `{filter}`"
    )))
}

/// `orbit schema [COMMAND] [--docs]`.
pub fn cmd_schema(args: &SchemaArgs) -> Result<CommandOutput> {
    if args.docs {
        let mut output = CommandOutput::new(Value::Null, "");
        output.raw_output = Some(commands_markdown());
        return Ok(output);
    }
    let data = match args.command.as_deref() {
        Some(filter) if !filter.trim().is_empty() => schema_lookup(filter)?,
        _ => schema_document(),
    };
    let human = serde_json::to_string_pretty(&data)
        .map_err(|error| OrbitError::config(format!("serialize schema: {error}")))?;
    Ok(CommandOutput::new(data, human))
}

fn access_label(tool: &Tool) -> String {
    let mut parts = Vec::new();
    parts.push(if tool.read_only {
        "read-only".to_string()
    } else {
        "write".to_string()
    });
    if tool.destructive {
        parts.push("destructive".to_string());
    }
    if tool.idempotent {
        parts.push("idempotent".to_string());
    }
    parts.join(", ")
}

/// The generated `docs/commands.md` reference section. The surrounding
/// markers must stay in `docs/commands.md`; the parity test compares the
/// block byte-for-byte.
pub fn commands_markdown() -> String {
    let mut out = String::from(
        "<!-- BEGIN GENERATED COMMANDS (orbit schema --docs; edit src/registry.rs) -->\n\n\
         | Command | MCP tool | Access |\n\
         | --- | --- | --- |\n",
    );
    for entry in COMMANDS {
        let access = match tool_for(entry) {
            Some(tool) => access_label(tool),
            None => format!("— ({})", entry.exempt.unwrap_or("no MCP tool")),
        };
        let tool_cell = match entry.tool {
            Some(name) => format!("`{name}`"),
            None => "—".to_string(),
        };
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            command_label(entry),
            tool_cell,
            access
        ));
    }
    out.push_str("\n<!-- END GENERATED COMMANDS -->\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn clap_paths() -> std::collections::BTreeSet<Vec<String>> {
        fn walk(
            command: &clap::Command,
            prefix: &mut Vec<String>,
            out: &mut std::collections::BTreeSet<Vec<String>>,
        ) {
            let subcommands: Vec<_> = command.get_subcommands().collect();
            // A node is an invocable command when it has no subcommands or
            // its subcommand is optional (`remote issues`, `migrate`).
            let leaf = subcommands.is_empty() || !command.is_subcommand_required_set();
            if leaf {
                out.insert(prefix.clone());
            }
            for sub in subcommands {
                if sub.get_name() == "help" {
                    continue;
                }
                prefix.push(sub.get_name().to_string());
                walk(sub, prefix, out);
                prefix.pop();
            }
        }
        let mut out = std::collections::BTreeSet::new();
        walk(&crate::cli::Cli::command(), &mut Vec::new(), &mut out);
        out
    }

    #[test]
    fn registry_covers_every_clap_subcommand() {
        let clap = clap_paths();
        let registry: std::collections::BTreeSet<Vec<String>> = COMMANDS
            .iter()
            .map(|entry| entry.path.iter().map(|s| s.to_string()).collect())
            .collect();
        assert_eq!(
            clap, registry,
            "clap subcommands and registry COMMANDS diverged"
        );
    }

    #[test]
    fn every_command_has_a_tool_or_an_exemption() {
        for entry in COMMANDS {
            assert!(
                entry.tool.is_some() != entry.exempt.is_some(),
                "{} must set exactly one of tool/exempt",
                command_path(entry)
            );
            if let Some(name) = entry.tool {
                assert!(
                    find_tool(name).is_some(),
                    "{} references unknown MCP tool `{name}`",
                    command_path(entry)
                );
            }
        }
    }

    #[test]
    fn every_tool_is_referenced_and_dispatchable() {
        for tool in TOOLS {
            assert!(
                COMMANDS.iter().any(|entry| entry.tool == Some(tool.name)),
                "MCP tool `{}` is not referenced by any command",
                tool.name
            );
            if tool.custom {
                assert!(
                    crate::mcp::custom_tool_names().contains(&tool.name),
                    "custom MCP tool `{}` is missing from mcp::custom_tool_names()",
                    tool.name
                );
            } else {
                assert!(
                    !crate::mcp::custom_tool_names().contains(&tool.name),
                    "registry-driven MCP tool `{}` is listed as custom",
                    tool.name
                );
                // Every required argument must be reachable.
                for arg in tool.args {
                    if arg.required {
                        assert!(
                            arg.cli.is_some(),
                            "required argument `{}` of `{}` has no CLI mapping",
                            arg.name,
                            tool.name
                        );
                    }
                }
            }
        }
        // And the other direction: every hand-dispatched tool is registered.
        for name in crate::mcp::custom_tool_names() {
            let tool = find_tool(name)
                .unwrap_or_else(|| panic!("custom MCP tool `{name}` is not in TOOLS"));
            assert!(tool.custom, "MCP tool `{name}` must be marked custom");
        }
    }

    #[test]
    fn registry_tool_argv_paths_match_commands() {
        let paths: std::collections::BTreeSet<Vec<String>> = COMMANDS
            .iter()
            .map(|entry| entry.path.iter().map(|s| s.to_string()).collect())
            .collect();
        for tool in TOOLS.iter().filter(|tool| !tool.custom) {
            let argv: Vec<String> = tool.argv.iter().map(|s| s.to_string()).collect();
            assert!(
                paths.contains(&argv),
                "tool `{}` argv {:?} is not a registered command path",
                tool.name,
                argv
            );
        }
    }

    #[test]
    fn tool_argv_builds_expected_commands() {
        let cases: &[(&str, Value, &[&str])] = &[
            ("tree", json!({"id": "orb-1"}), &["tree", "orb-1"]),
            (
                "prime",
                json!({"by": "devin", "labels": ["a", "b"], "hook_json": true}),
                &[
                    "prime",
                    "--by",
                    "devin",
                    "--label",
                    "a",
                    "--label",
                    "b",
                    "--hook-json",
                ],
            ),
            (
                "slug_migrate",
                json!({"slug": "atlas", "dry_run": true}),
                &["slug", "migrate", "atlas", "--dry-run"],
            ),
            (
                "remote_issues",
                json!({"state": "open", "labels": ["bug"], "linked": true}),
                &[
                    "remote", "issues", "--state", "open", "--label", "bug", "--linked",
                ],
            ),
            (
                "remote_import",
                json!({"issues": ["github:o/r#1", "github:o/r#2"], "apply": true}),
                &[
                    "remote",
                    "import",
                    "github:o/r#1",
                    "github:o/r#2",
                    "--apply",
                ],
            ),
            (
                "board_sync",
                json!({"resolutions": ["title=local"]}),
                &["board", "sync", "--resolve", "title=local"],
            ),
            (
                "remote_auto_push",
                json!({"state": "off"}),
                &["remote", "auto-push", "off"],
            ),
            (
                "migrate_model",
                json!({"apply": true}),
                &["migrate", "model", "--apply"],
            ),
        ];
        for (name, arguments, expected) in cases {
            let argv = tool_argv(name, arguments).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(&argv, expected, "{name}");
        }
    }

    #[test]
    fn tool_argv_rejects_missing_required_and_unknown_args() {
        assert!(tool_argv("tree", &json!({})).is_err());
        assert!(tool_argv("tree", &json!({"id": "x", "bogus": 1})).is_err());
        assert!(tool_argv("ready", &json!({})).is_err()); // custom tool
        assert!(tool_argv("not_a_tool", &json!({})).is_err());
        assert!(
            tool_argv("remote_issues", &json!({"state": "bogus"})).is_ok(),
            "enum values are schema-level; argv passes strings through"
        );
    }

    #[test]
    fn docs_commands_md_generated_section_is_current() {
        let docs =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/commands.md"))
                .expect("read docs/commands.md");
        let begin = "<!-- BEGIN GENERATED COMMANDS";
        let end = "<!-- END GENERATED COMMANDS -->";
        let start = docs.find(begin).expect("missing generated begin marker");
        let stop = docs.find(end).expect("missing generated end marker");
        let generated = &docs[start..stop + end.len()];
        assert_eq!(
            generated,
            commands_markdown().trim_end(),
            "docs/commands.md generated section is stale; run `orbit schema --docs` and update the markers"
        );
    }

    #[test]
    fn schema_document_lists_every_command_and_tool() {
        let document = schema_document();
        assert_eq!(
            document["commands"].as_array().unwrap().len(),
            COMMANDS.len()
        );
        assert_eq!(document["tools"].as_array().unwrap().len(), TOOLS.len());
        assert!(
            document["commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["command"] == "orbit ready")
        );
        let ready = schema_lookup("ready").unwrap();
        assert_eq!(ready["command"]["command"], "orbit ready");
        let create = schema_lookup("create").unwrap();
        assert!(create["tool"]["name"] == "create");
        assert!(schema_lookup("no-such-command").is_err());
    }
}
