use std::path::PathBuf;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};

use crate::help;
use crate::setup::SetupScope;
use crate::task::IdentityKind;

#[derive(Debug, Parser)]
#[command(
    name = "orbit",
    version,
    about = "A Git-native issue tracker and atomic work queue for coding agents",
    long_about = help::ROOT_LONG,
    after_long_help = help::ROOT_EXAMPLES
)]
pub struct Cli {
    /// Emit exactly one JSON object on stdout, including errors.
    #[arg(global = true, long)]
    pub json: bool,

    /// Board anchor or Orbit Board directory override. Also read from ORBIT_DIR.
    #[arg(global = true, long, env = "ORBIT_DIR", value_name = "PATH")]
    pub dir: Option<PathBuf>,

    /// Do not auto-push task changes to the Remote Tracker or sync the Board
    /// host for this command. Hidden from help; still functional.
    #[arg(global = true, long = "no-sync", hide = true)]
    pub no_sync: bool,

    /// Run the mutation only when the Task's version still matches VERSION —
    /// the `updated_at` value `orbit show <id>` reports. A stale version
    /// fails with a retryable VERSION_CONFLICT. Applies to Task mutations
    /// (`update`, `claim`, `done`, `link add`, ...); `orbit batch` takes a
    /// per-operation `if_version` instead.
    #[arg(global = true, long = "if-version", value_name = "VERSION")]
    pub if_version: Option<String>,

    /// Idempotency key for this write: repeating the call with the same key
    /// returns the first recorded result instead of running again.
    #[arg(global = true, long = "idempotency-key", value_name = "KEY")]
    pub idempotency_key: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create the Board and wire coding-agent harnesses.
    #[command(long_about = help::INIT_LONG, after_long_help = help::INIT_EXAMPLES)]
    Init(InitArgs),

    /// Manage the Board's member Repositories.
    #[command(long_about = help::REPO_LONG, after_long_help = help::REPO_EXAMPLES)]
    Repo(RepoArgs),

    /// Show or migrate the Board's Task-ID slug.
    #[command(long_about = help::SLUG_LONG, after_long_help = help::SLUG_EXAMPLES)]
    Slug(SlugArgs),

    /// Convert a pre-ADR-0019 Board into a standalone Board folder.
    ///
    /// Previews the migration by default; pass --apply to convert the Board
    /// into its own Git repository on branch `orbit` under `.orbit/board`
    /// (or a visible folder anchored by `.orbit`), rewriting `github:` links
    /// to `remote:`, `[github]` to remote project entries, `repo:*` labels to
    /// the `repository` field, and seeding `board.components` — all in one
    /// commit.
    ///
    /// The `model` subcommand instead converts a pre-ADR-0022 Board's Tasks
    /// to the Issue model: workflow statuses, named priorities, typed links,
    /// properties, components/repository fields, Issue types, and project
    /// keys — also previewed by default and committed once with --apply.
    #[command(long_about = help::BOARD_MIGRATE_LONG, after_long_help = help::BOARD_MIGRATE_EXAMPLES)]
    Migrate(MigrateArgs),

    /// Create a task.
    #[command(long_about = help::ADD_LONG, after_long_help = help::ADD_EXAMPLES)]
    Add(AddArgs),

    /// List tasks with filters. Closed work is hidden unless --include-closed.
    /// The default help assumes Remote Tracker material; `parse` swaps in the
    /// local-only variant for Boards without it.
    #[command(
        long_about = help::list_long(true),
        after_long_help = help::list_examples(true)
    )]
    List(ListArgs),

    /// List or atomically claim the next ready task.
    #[command(long_about = help::READY_LONG, after_long_help = help::READY_EXAMPLES)]
    Ready(ReadyArgs),

    /// Show a task with its body, dependencies, and comments.
    #[command(long_about = help::SHOW_LONG, after_long_help = help::SHOW_EXAMPLES)]
    Show(ShowArgs),

    /// Show a Task, its SubTasks at every level, and blocking edges.
    #[command(long_about = help::TREE_LONG, after_long_help = help::TREE_EXAMPLES)]
    Tree(TreeArgs),

    /// Claim a specific task and start its lease.
    #[command(long_about = help::CLAIM_LONG, after_long_help = help::CLAIM_EXAMPLES)]
    Claim(ClaimArgs),

    /// Assign a task to a person or agent without a lease.
    #[command(long_about = help::ASSIGN_LONG, after_long_help = help::ASSIGN_EXAMPLES)]
    Assign(AssignArgs),

    /// Release a claim and return the task to todo.
    #[command(long_about = help::RELEASE_LONG, after_long_help = help::RELEASE_EXAMPLES)]
    Release(ReleaseArgs),

    /// Extend the lease on a claimed task.
    #[command(long_about = help::HEARTBEAT_LONG, after_long_help = help::HEARTBEAT_EXAMPLES)]
    Heartbeat(HeartbeatArgs),

    /// Record or update the active work checkpoint.
    #[command(long_about = help::CHECKPOINT_LONG, after_long_help = help::CHECKPOINT_EXAMPLES)]
    Checkpoint(CheckpointArgs),

    /// Complete a claimed task.
    #[command(long_about = help::DONE_LONG, after_long_help = help::DONE_EXAMPLES)]
    Done(DoneArgs),

    /// Cancel a task without completing it.
    #[command(long_about = help::CANCEL_LONG, after_long_help = help::CANCEL_EXAMPLES)]
    Cancel(CancelArgs),

    /// Reopen a done or cancelled task.
    #[command(long_about = help::REOPEN_LONG, after_long_help = help::REOPEN_EXAMPLES)]
    Reopen(ReopenArgs),

    /// Run a named workflow transition on a task.
    #[command(long_about = help::TRANSITION_LONG, after_long_help = help::TRANSITION_EXAMPLES)]
    Transition(TransitionArgs),

    /// Edit a task's title, body, priority, or external blocker.
    #[command(long_about = help::UPDATE_LONG, after_long_help = help::UPDATE_EXAMPLES)]
    Update(UpdateArgs),

    /// Add, remove, or list task labels.
    #[command(long_about = help::LABEL_LONG, after_long_help = help::LABEL_EXAMPLES)]
    Label(LabelArgs),

    /// Apply many task operations in one commit, all or none.
    #[command(long_about = help::BATCH_LONG, after_long_help = help::BATCH_EXAMPLES)]
    Batch(BatchArgs),

    /// Set or remove a tool's Property on a task.
    #[command(long_about = help::PROPERTY_LONG, after_long_help = help::PROPERTY_EXAMPLES)]
    Property(PropertyArgs),

    /// Add to or read a task's comment thread.
    #[command(long_about = help::COMMENT_LONG, after_long_help = help::COMMENT_EXAMPLES)]
    Comment(CommentArgs),

    /// Make one task wait for another.
    #[command(long_about = help::DEPEND_LONG, after_long_help = help::DEPEND_EXAMPLES)]
    Depend(DependArgs),

    /// Remove a dependency edge between two tasks.
    #[command(long_about = help::UNDEPEND_LONG, after_long_help = help::UNDEPEND_EXAMPLES)]
    Undepend(UndependArgs),

    /// Manage typed links to other tasks or external references.
    #[command(long_about = help::LINK_LONG, after_long_help = help::LINK_EXAMPLES)]
    Link(LinkArgs),

    /// Permanently delete a task and its comments.
    #[command(long_about = help::DELETE_LONG, after_long_help = help::DELETE_EXAMPLES)]
    Delete(DeleteArgs),

    /// Print starting context for an agent session.
    #[command(long_about = help::PRIME_LONG, after_long_help = help::PRIME_EXAMPLES)]
    Prime(PrimeArgs),

    /// Export a deterministic JSON snapshot of the Board.
    #[command(long_about = help::EXPORT_LONG, after_long_help = help::EXPORT_EXAMPLES)]
    Export,

    /// Install, check, or remove one agent integration.
    #[command(long_about = help::SETUP_LONG, after_long_help = help::SETUP_EXAMPLES)]
    Setup(SetupArgs),

    /// Serve the MCP tools and resources over stdio.
    #[command(long_about = help::MCP_LONG, after_long_help = help::MCP_EXAMPLES)]
    Mcp,

    /// Check the Board for integrity issues.
    #[command(long_about = help::DOCTOR_LONG, after_long_help = help::DOCTOR_EXAMPLES)]
    Doctor(DoctorArgs),

    /// Commit pending lease-only changes to the board branch.
    #[command(long_about = help::SYNC_LONG, after_long_help = help::SYNC_EXAMPLES)]
    Sync,

    /// Report the Board's contract handshake: contract version, modules, and
    /// workflow. Answers even on a Board whose contract is newer than this
    /// binary's, so consumers can negotiate before other commands refuse.
    #[command(long_about = help::CAPABILITIES_LONG, after_long_help = help::CAPABILITIES_EXAMPLES)]
    Capabilities,

    /// Print the JSON Schema of the command registry.
    ///
    /// Describes every public command once — CLI path, summary, MCP tool,
    /// argument schema, and safety annotations — as published by the command
    /// registry in `src/registry.rs`. With COMMAND, prints just that command's
    /// or tool's entry; --docs prints the generated docs/commands.md section.
    #[command(long_about = help::SCHEMA_LONG, after_long_help = help::SCHEMA_EXAMPLES)]
    Schema(SchemaArgs),

    /// Configure and synchronize the Board's host repository.
    #[command(long_about = help::BOARD_LONG, after_long_help = help::BOARD_EXAMPLES)]
    Board(BoardArgs),

    /// Connect Remote Trackers and synchronize Remote Issues with Tasks.
    #[command(name = "remote", long_about = help::REMOTE_LONG, after_long_help = help::REMOTE_EXAMPLES)]
    Remote(RemoteArgs),

    /// Review and apply agent triage proposals for Tasks.
    #[command(long_about = help::TRIAGE_LONG, after_long_help = help::TRIAGE_EXAMPLES)]
    Triage(TriageArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Orbit folder (DIR) to create or reuse. Defaults to ./.orbit.
    ///
    /// A path that is an existing Git working-tree root, such as `.`, initializes
    /// that checkout's ./.orbit instead of using the root as the Board directory.
    #[arg(value_name = "DIR")]
    pub target: Option<PathBuf>,

    /// Which harnesses to wire: auto, all, none, ask, or a comma-separated list.
    #[arg(long, value_name = "SELECTOR", conflicts_with = "no_agents")]
    pub agents: Option<String>,

    /// Do not write any harness integration.
    #[arg(long = "no-agents", conflicts_with = "agents")]
    pub no_agents: bool,

    /// Install the SessionStart hook for Claude and Devin.
    #[arg(long)]
    pub hooks: bool,

    /// Orbit executable to record in agent configs. Defaults to the running binary.
    ///
    /// Prefer --portable-mcp when you want the bare `orbit` command.
    #[arg(long = "mcp-bin", value_name = "PATH")]
    pub mcp_bin: Option<PathBuf>,

    /// Write MCP configs with the portable `orbit` command instead of an absolute path.
    ///
    /// The editor or agent that launches the MCP server must be able to find
    /// `orbit` on PATH. In a terminal, init also offers this as a choice.
    #[arg(long = "portable-mcp", conflicts_with = "mcp_bin")]
    pub portable_mcp: bool,

    /// Board slug used to prefix Task IDs. Bare word: teramigo, not teramigo-.
    #[arg(long, value_name = "SLUG")]
    pub slug: Option<String>,

    /// Use a visible Board folder instead of the hidden .orbit/board layout.
    ///
    /// Bare `--visible` names the folder `issues`; `--visible=NAME` picks the
    /// name. Only applies when the target resolves to a repository or Board
    /// anchor — an explicit directory is already a visible Board folder.
    #[arg(
        long,
        value_name = "NAME",
        num_args = 0..=1,
        require_equals = true
    )]
    pub visible: Option<Option<String>>,

    /// Set the Board host to a member Repository name/path or git URL whose
    /// remote stores the Board's `orbit` branch.
    ///
    /// Bare `--host` prompts in a terminal; `--host=REPO|URL` sets it without
    /// prompting. An existing host branch with a different Slug is refused.
    #[arg(long, value_name = "REPO|URL", num_args = 0..=1, require_equals = true)]
    pub host: Option<Option<String>>,

    /// Preview the files that would change without writing them; harness
    /// selection does not prompt unless --agents ask is given.
    #[arg(long)]
    pub print: bool,
}

#[derive(Debug, Args)]
pub struct RepoArgs {
    #[command(subcommand)]
    pub command: RepoCommand,
}

#[derive(Debug, Subcommand)]
pub enum RepoCommand {
    /// Add a Repository as an explicit member of this Board.
    #[command(after_long_help = help::REPO_EXAMPLES)]
    Add(RepoAddArgs),
    /// Remove an explicitly added member Repository.
    #[command(after_long_help = help::REPO_EXAMPLES)]
    Remove(RepoRemoveArgs),
    /// List the Board's member Repositories.
    #[command(after_long_help = help::REPO_EXAMPLES)]
    List,
    /// Rescan child Repositories and rebuild the Repository inventory.
    #[command(after_long_help = help::REPO_EXAMPLES)]
    Refresh,
}

#[derive(Debug, Args)]
pub struct RepoAddArgs {
    /// Repository path or its main checkout. Defaults to the current directory.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Member name Tasks record in their `repo` field. Defaults to the
    /// repository directory name.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct RepoRemoveArgs {
    /// Member name or repository path recorded by `orbit repo add`.
    #[arg(value_name = "REPOSITORY")]
    pub repository: String,
}

#[derive(Debug, Args)]
pub struct SlugArgs {
    #[command(subcommand)]
    pub command: SlugCommand,
}

#[derive(Debug, Subcommand)]
pub enum SlugCommand {
    /// Show the Board's slug, literal prefix, and task counts.
    #[command(
        long_about = help::SLUG_SHOW_LONG,
        after_long_help = help::SLUG_SHOW_EXAMPLES
    )]
    Show,

    /// Rewrite every Task and SubTask ID to a new slug.
    #[command(
        long_about = help::SLUG_MIGRATE_LONG,
        after_long_help = help::SLUG_MIGRATE_EXAMPLES
    )]
    Migrate(SlugMigrateArgs),
}

#[derive(Debug, Args)]
pub struct MigrateArgs {
    /// Apply the migration. Without this flag the command only previews.
    #[arg(long, conflicts_with = "dry_run")]
    pub apply: bool,

    /// Preview the migration without writing anything (the default).
    #[arg(long)]
    pub dry_run: bool,

    /// Model conversions for Boards written before the Issue model.
    #[command(subcommand)]
    pub command: Option<MigrateCommand>,
}

#[derive(Debug, Subcommand)]
pub enum MigrateCommand {
    /// Rewrite a pre-ADR-0022 Board's Tasks to the Issue model: workflow
    /// statuses, named priorities, typed links, properties,
    /// components/repository fields, Issue types, and project keys.
    ///
    /// Previews by default; --apply rewrites config.toml, every Task file,
    /// and keys.toml in a single validated commit. Run `orbit migrate`
    /// first when the Board still uses a pre-ADR-0019 layout.
    #[command(long_about = help::MODEL_MIGRATE_LONG, after_long_help = help::MODEL_MIGRATE_EXAMPLES)]
    Model(ModelMigrateArgs),
}

#[derive(Debug, Args)]
pub struct ModelMigrateArgs {
    /// Apply the migration. Without this flag the command only previews.
    #[arg(long, conflicts_with = "dry_run")]
    pub apply: bool,

    /// Preview the migration without writing anything (the default).
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct SlugMigrateArgs {
    /// New slug. Bare word: atlas, not atlas-.
    pub slug: String,

    /// Print the migration plan without writing any files.
    #[arg(long)]
    pub dry_run: bool,

    /// Migrate even while tasks have live claims, then list the affected holders.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// Task title. Required and must not be empty.
    pub title: String,

    /// Priority name from the Board's `board.priorities` list.
    #[arg(short = 'p', long, value_name = "NAME")]
    pub priority: Option<String>,

    /// Issue type name from the Board's `board.types`
    /// (epic, story, task, bug, sub-task by default).
    #[arg(long = "type", value_name = "TYPE")]
    pub task_type: Option<String>,

    /// Make the new task wait for an existing task. Repeatable.
    #[arg(long = "depends-on", value_name = "ID", action = clap::ArgAction::Append)]
    pub depends_on: Vec<String>,

    /// Nest the new Task under this parent. The Board's `board.hierarchy`
    /// decides which types may parent which; `--type` picks the child's type
    /// (the leaf type, `sub-task` by default, when omitted).
    #[arg(long, value_name = "ID")]
    pub parent: Option<String>,

    /// Record a blocker that lives outside the Board.
    #[arg(long, value_name = "TEXT")]
    pub waiting_on: Option<String>,

    /// Initial labels. Repeatable.
    #[arg(long, value_name = "LABEL", action = clap::ArgAction::Append)]
    pub label: Vec<String>,

    /// Optional Component slug. Restricted to the Board's `board.components`
    /// when the Board configures them; any lowercase slug otherwise.
    #[arg(long, value_name = "COMPONENT")]
    pub component: Option<String>,

    /// The Task's identity in an external system; unique per Board.
    #[arg(long = "external-id", value_name = "ID")]
    pub external_id: Option<String>,

    /// Concise verification checks or commands for agents.
    #[arg(long, value_name = "TEXT")]
    pub verification: Option<String>,

    /// Task body. Use "-" to read stdin.
    #[arg(long, value_name = "TEXT", conflicts_with = "body_file")]
    pub body: Option<String>,

    /// Read the task body from a file. Use "-" to read stdin.
    #[arg(long = "body-file", value_name = "PATH", conflicts_with = "body")]
    pub body_file: Option<PathBuf>,

    /// Caller-chosen identity; an add whose Source Key exists returns that Task.
    #[arg(long = "source-key", value_name = "KEY")]
    pub source_key: Option<String>,

    /// Reporter recorded at creation. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub reporter: Option<String>,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Filter by workflow status name. Requesting a closed status includes
    /// closed work.
    #[arg(long, value_name = "STATUS")]
    pub status: Option<String>,

    /// Filter by priority name.
    #[arg(long, value_name = "NAME")]
    pub priority: Option<String>,

    /// Filter by assignee name.
    #[arg(long = "assignee", visible_alias = "claimed-by", value_name = "NAME")]
    pub assignee: Option<String>,

    /// Only tasks waiting on an external blocker.
    #[arg(long)]
    pub waiting: bool,

    /// Filter by Issue type. Repeatable; matches any listed type.
    #[arg(long = "type", value_name = "TYPE", action = clap::ArgAction::Append)]
    pub task_type: Vec<String>,

    /// Exclude Issue types. Repeatable; excludes any listed type.
    #[arg(long = "exclude-type", value_name = "TYPE", action = clap::ArgAction::Append)]
    pub exclude_type: Vec<String>,

    /// Filter by Component slug. Repeatable; matches any listed component.
    #[arg(long, value_name = "COMPONENT", action = clap::ArgAction::Append)]
    pub component: Vec<String>,

    /// Exclude tasks with any listed component.
    #[arg(long = "exclude-component", value_name = "COMPONENT", action = clap::ArgAction::Append)]
    pub exclude_component: Vec<String>,

    /// Filter by label. Repeatable; every label must match.
    #[arg(long, value_name = "LABEL", action = clap::ArgAction::Append)]
    pub label: Vec<String>,

    /// Exclude tasks carrying any of these labels. Repeatable.
    #[arg(long = "exclude-label", value_name = "LABEL", action = clap::ArgAction::Append)]
    pub exclude_label: Vec<String>,

    /// Filter by member Repository. Matches the `repository` field.
    #[arg(long, value_name = "NAME")]
    pub repository: Option<String>,

    /// Only tasks with no labels.
    #[arg(long, conflicts_with_all = ["label", "repository"])]
    pub unlabeled: bool,

    /// Filter to direct children of this parent Task.
    #[arg(long, value_name = "ID")]
    pub parent: Option<String>,

    /// Only Tasks with an active Remote Issue link.
    #[arg(long, conflicts_with = "local")]
    pub linked: bool,

    /// Only Tasks without an active Remote Issue link. Includes detached provenance.
    #[arg(long, conflicts_with = "linked")]
    pub local: bool,

    /// Group tasks under their Parent Task and render the grouped table.
    #[arg(long)]
    pub group: bool,

    /// Include Tasks from every Repository. Inside a member Repository or its
    /// worktree the list is scoped to that Repository plus Board-wide Tasks
    /// by default; --all widens it to the whole Board.
    #[arg(long)]
    pub all: bool,

    /// Include done and cancelled tasks.
    #[arg(long)]
    pub include_closed: bool,
}

#[derive(Debug, Args)]
pub struct ReadyArgs {
    /// Maximum number of tasks to list.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,

    /// Only Ready direct children of this parent Task.
    #[arg(long, value_name = "ID")]
    pub parent: Option<String>,

    /// Only Ready tasks with this member Repository. Matches the `repository` field.
    #[arg(long, value_name = "NAME")]
    pub repository: Option<String>,

    /// Include Ready tasks from every Repository. Inside a member Repository
    /// or its worktree the queue is scoped to that Repository plus Board-wide
    /// Tasks by default; --all widens it to the whole Board.
    #[arg(long)]
    pub all: bool,

    /// Only Ready tasks with one of these Issue types. Repeatable.
    #[arg(long = "type", value_name = "TYPE", action = clap::ArgAction::Append)]
    pub task_type: Vec<String>,

    /// Exclude Ready tasks with any of these Issue types. Repeatable.
    #[arg(long = "exclude-type", value_name = "TYPE", action = clap::ArgAction::Append)]
    pub exclude_type: Vec<String>,

    /// Only Ready tasks with any listed Component. Repeatable.
    #[arg(long, value_name = "COMPONENT", action = clap::ArgAction::Append)]
    pub component: Vec<String>,

    /// Exclude Ready tasks with any listed Component. Repeatable.
    #[arg(long = "exclude-component", value_name = "COMPONENT", action = clap::ArgAction::Append)]
    pub exclude_component: Vec<String>,

    /// Only Ready tasks carrying every label. Repeatable.
    #[arg(long, value_name = "LABEL", action = clap::ArgAction::Append)]
    pub label: Vec<String>,

    /// Exclude Ready tasks carrying any of these labels. Repeatable.
    #[arg(long = "exclude-label", value_name = "LABEL", action = clap::ArgAction::Append)]
    pub exclude_label: Vec<String>,

    /// Claim the first ready task in the same transaction.
    #[arg(long)]
    pub claim: bool,

    /// Agent identity for --claim.
    #[arg(long, value_name = "NAME", requires = "claim")]
    pub by: Option<String>,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct TreeArgs {
    /// Parent Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct ClaimArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Agent identity. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,

    /// The assignee's kind.
    #[arg(long, value_enum, default_value_t = IdentityKind::Agent)]
    pub kind: IdentityKind,

    /// Take over another agent's claim, even while its lease is live.
    #[arg(long)]
    pub steal: bool,

    /// Create .worktrees/<id> on branch ticket/<id> in the Task's Repository.
    #[arg(long)]
    pub worktree: bool,
}

#[derive(Debug, Args)]
pub struct AssignArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// The assignee's name — a person or an agent.
    #[arg(long, value_name = "NAME")]
    pub to: String,

    /// The assignee's kind.
    #[arg(long, value_enum, default_value_t = IdentityKind::Human)]
    pub kind: IdentityKind,

    /// Replace a different assignee.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ReleaseArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Agent identity. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,

    /// Release another agent's claim.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct HeartbeatArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Agent identity. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,
}

#[derive(Debug, Args)]
pub struct CheckpointArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Current progress. Required when recording the first checkpoint.
    #[arg(long, value_name = "TEXT")]
    pub progress: Option<String>,

    /// The next useful action for the next claimant.
    #[arg(long, value_name = "TEXT", conflicts_with = "clear_next")]
    pub next: Option<String>,

    /// Remove the next action from the checkpoint.
    #[arg(long = "clear-next")]
    pub clear_next: bool,

    /// A blocker the next claimant must know about.
    #[arg(long, value_name = "TEXT", conflicts_with = "clear_blocker")]
    pub blocker: Option<String>,

    /// Remove the blocker from the checkpoint.
    #[arg(long = "clear-blocker")]
    pub clear_blocker: bool,

    /// Agent identity. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,
}

#[derive(Debug, Args)]
pub struct DoneArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Agent identity. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,

    /// Override a competing live claim. Dependencies are still enforced.
    #[arg(long)]
    pub force: bool,

    /// Completion note to record in the same transaction. Use "-" to read stdin.
    #[arg(long, value_name = "TEXT", conflicts_with = "comment_file")]
    pub comment: Option<String>,

    /// Read the completion note from a file. Use "-" to read stdin.
    #[arg(long = "comment-file", value_name = "PATH", conflicts_with = "comment")]
    pub comment_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct CancelArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Agent identity. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,

    /// Cancel another agent's live claim.
    #[arg(long)]
    pub force: bool,

    /// Cancellation note to record in the same transaction. Use "-" to read stdin.
    #[arg(long, value_name = "TEXT", conflicts_with = "comment_file")]
    pub comment: Option<String>,

    /// Read the cancellation note from a file. Use "-" to read stdin.
    #[arg(long = "comment-file", value_name = "PATH", conflicts_with = "comment")]
    pub comment_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ReopenArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Reopen even when a dependent task is already done.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct TransitionArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Workflow transition name, for example promote or block.
    pub name: String,

    /// Caller identity for Claim-holder checks.
    #[arg(long)]
    pub by: Option<String>,

    /// Run even when another holder's live Claim blocks the transition.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// New title.
    #[arg(long, value_name = "TEXT")]
    pub title: Option<String>,

    /// New body. Use "-" to read stdin.
    #[arg(long, value_name = "TEXT", conflicts_with = "body_file")]
    pub body: Option<String>,

    /// Read the new body from a file. Use "-" to read stdin.
    #[arg(long = "body-file", value_name = "PATH", conflicts_with = "body")]
    pub body_file: Option<PathBuf>,

    /// New priority name.
    #[arg(long, value_name = "NAME")]
    pub priority: Option<String>,

    /// New Issue type name from the Board's `board.types`.
    #[arg(long = "type", value_name = "TYPE")]
    pub task_type: Option<String>,

    /// New external blocker.
    #[arg(
        long = "waiting-on",
        value_name = "TEXT",
        conflicts_with = "clear_waiting"
    )]
    pub waiting_on: Option<String>,

    /// Remove the external blocker.
    #[arg(long = "clear-waiting")]
    pub clear_waiting: bool,

    /// New Component slug. Restricted to the Board's `board.components` when
    /// the Board configures them; any lowercase slug otherwise.
    #[arg(long, value_name = "COMPONENT", conflicts_with = "clear_component")]
    pub component: Option<String>,

    /// Remove the Component classification.
    #[arg(long = "clear-component")]
    pub clear_component: bool,

    /// New member Repository name. Absent means Board-wide.
    #[arg(long, value_name = "NAME", conflicts_with = "clear_repository")]
    pub repository: Option<String>,

    /// Remove the member Repository classification.
    #[arg(long = "clear-repository")]
    pub clear_repository: bool,

    /// New External ID; unique per Board.
    #[arg(
        long = "external-id",
        value_name = "ID",
        conflicts_with = "clear_external_id"
    )]
    pub external_id: Option<String>,

    /// Remove the External ID.
    #[arg(long = "clear-external-id")]
    pub clear_external_id: bool,

    /// New verification checks or commands for agents.
    #[arg(long, value_name = "TEXT", conflicts_with = "clear_verification")]
    pub verification: Option<String>,

    /// Remove task-level verification guidance.
    #[arg(long = "clear-verification")]
    pub clear_verification: bool,
}

#[derive(Debug, Args)]
pub struct DependArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// The task that must finish first.
    #[arg(value_name = "DEP_ID")]
    pub dep_id: String,
}

#[derive(Debug, Args)]
pub struct UndependArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// The dependency to remove.
    #[arg(value_name = "DEP_ID")]
    pub dep_id: String,
}

#[derive(Debug, Args)]
pub struct LinkArgs {
    #[command(subcommand)]
    pub command: LinkCommand,
}

#[derive(Debug, Subcommand)]
pub enum LinkCommand {
    /// Add a typed link from a task to another task or an external reference.
    #[command(
        long_about = help::LINK_ADD_LONG,
        after_long_help = help::LINK_ADD_EXAMPLES
    )]
    Add(LinkAddArgs),
    /// Remove links matching a type and target.
    #[command(
        long_about = help::LINK_REMOVE_LONG,
        after_long_help = help::LINK_REMOVE_EXAMPLES
    )]
    Remove(LinkRemoveArgs),
    /// List a task's links with their types, targets, and attributes.
    #[command(
        long_about = help::LINK_LIST_LONG,
        after_long_help = help::LINK_LIST_EXAMPLES
    )]
    List(LinkTargetArgs),
}

#[derive(Debug, Args)]
pub struct LinkAddArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12. The link is stored on this task.
    pub id: String,

    /// Link type: blocks, relates-to, duplicates, discovered-from, or a
    /// custom slug. Defaults to relates-to.
    #[arg(long = "type", value_name = "TYPE", default_value = "relates-to")]
    pub kind: String,

    /// Link target: an Orbit Task ID or project key.
    #[arg(
        long,
        value_name = "ID",
        conflicts_with = "external",
        required_unless_present = "external"
    )]
    pub to: Option<String>,

    /// Link target: an external reference as SYSTEM:ID, for example jira:PROJ-123.
    #[arg(long, value_name = "SYSTEM:ID")]
    pub external: Option<String>,

    /// Canonical URL for an external target.
    #[arg(long, value_name = "URL", requires = "external")]
    pub url: Option<String>,

    /// Mark the link as blocking (overrides the type's default).
    #[arg(long, conflicts_with = "no_blocking")]
    pub blocking: bool,

    /// Mark the link as non-blocking (overrides the type's default).
    #[arg(long = "no-blocking")]
    pub no_blocking: bool,

    /// Link attribute as KEY=VALUE. Repeatable; VALUE is parsed as YAML.
    #[arg(long, value_name = "KEY=VALUE", action = clap::ArgAction::Append)]
    pub attribute: Vec<String>,
}

#[derive(Debug, Args)]
pub struct LinkRemoveArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Only remove links of this type. Omit to match every type.
    #[arg(long = "type", value_name = "TYPE")]
    pub kind: Option<String>,

    /// Link target: an Orbit Task ID or project key.
    #[arg(
        long,
        value_name = "ID",
        conflicts_with = "external",
        required_unless_present = "external"
    )]
    pub to: Option<String>,

    /// Link target: an external reference as SYSTEM:ID.
    #[arg(long, value_name = "SYSTEM:ID")]
    pub external: Option<String>,
}

#[derive(Debug, Args)]
pub struct LinkTargetArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Agent identity. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,

    /// Delete even when dependents exist, or another agent holds a live claim.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct LabelArgs {
    #[command(subcommand)]
    pub command: LabelCommand,
}

#[derive(Debug, Subcommand)]
pub enum LabelCommand {
    /// Add one or more labels to a task.
    #[command(
        long_about = help::LABEL_ADD_LONG,
        after_long_help = help::LABEL_ADD_EXAMPLES
    )]
    Add(LabelMutationArgs),

    /// Remove one or more labels from a task.
    #[command(
        long_about = help::LABEL_REMOVE_LONG,
        after_long_help = help::LABEL_REMOVE_EXAMPLES
    )]
    Remove(LabelMutationArgs),

    /// List labels in use and their task counts.
    #[command(
        long_about = help::LABEL_LIST_LONG,
        after_long_help = help::LABEL_LIST_EXAMPLES
    )]
    List,
}

#[derive(Debug, Args)]
pub struct LabelMutationArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// One or more lowercase labels.
    #[arg(required = true, num_args = 1.., value_name = "LABEL")]
    pub labels: Vec<String>,
}

#[derive(Debug, Args)]
pub struct BatchArgs {
    /// JSON file with the operations; "-" reads stdin.
    #[arg(long, value_name = "PATH")]
    pub file: String,

    /// Validate every operation and report the result without writing.
    #[arg(long)]
    pub dry_run: bool,

    /// Default agent for operations without `by`. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,
}

#[derive(Debug, Args)]
pub struct PropertyArgs {
    #[command(subcommand)]
    pub command: PropertyCommand,
}

#[derive(Debug, Subcommand)]
pub enum PropertyCommand {
    /// Replace one owner's Property with a JSON object.
    #[command(after_long_help = help::PROPERTY_SET_EXAMPLES)]
    Set(PropertySetArgs),

    /// Remove one owner's Property.
    #[command(after_long_help = help::PROPERTY_REMOVE_EXAMPLES)]
    Remove(PropertyRemoveArgs),
}

#[derive(Debug, Args)]
pub struct PropertySetArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Owner: the lowercase slug naming the tool, for example ci.
    pub owner: String,

    /// The Property, a JSON object of at most 8192 bytes.
    #[arg(long, value_name = "JSON")]
    pub value: String,
}

#[derive(Debug, Args)]
pub struct PropertyRemoveArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Owner whose Property is removed.
    pub owner: String,
}

#[derive(Debug, Args)]
pub struct CommentArgs {
    #[command(subcommand)]
    pub command: CommentCommand,
}

#[derive(Debug, Subcommand)]
pub enum CommentCommand {
    /// Append a comment to a task.
    #[command(
        long_about = help::COMMENT_ADD_LONG,
        after_long_help = help::COMMENT_ADD_EXAMPLES
    )]
    Add(CommentAddArgs),

    /// List a task's comments.
    #[command(
        long_about = help::COMMENT_LIST_LONG,
        after_long_help = help::COMMENT_LIST_EXAMPLES
    )]
    List(CommentListArgs),
}

#[derive(Debug, Args)]
pub struct CommentAddArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,

    /// Comment body. Use "-" to read stdin.
    #[arg(long, value_name = "TEXT", conflicts_with = "body_file")]
    pub body: Option<String>,

    /// Read the comment body from a file. Use "-" to read stdin.
    #[arg(long = "body-file", value_name = "PATH", conflicts_with = "body")]
    pub body_file: Option<PathBuf>,

    /// Comment author. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,
}

#[derive(Debug, Args)]
pub struct CommentListArgs {
    /// Task ID or project key, for example orb-a7b2c9 or ORB-12.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Remove stale temp files and re-attach an orphaned orbit worktree.
    #[arg(long)]
    pub fix: bool,
}

#[derive(Debug, Args)]
pub struct PrimeArgs {
    /// Emit Claude Code's SessionStart hook envelope.
    #[arg(long = "hook-json")]
    pub hook_json: bool,

    /// Agent identity used in the context. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,

    /// Only Ready tasks with this member Repository. Matches the `repository` field.
    #[arg(long, value_name = "NAME")]
    pub repository: Option<String>,

    /// Only Ready tasks carrying every label. Repeatable.
    #[arg(long, value_name = "LABEL", action = clap::ArgAction::Append)]
    pub label: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Which agent integration or profile to install.
    ///
    /// Agent integrations: claude, cursor, codex, opencode, devin,
    /// mattpocock, all. Any other name is resolved as a profile: a
    /// `profiles/<name>.toml` shipped with Orbit, in the Board's Orbit
    /// folder, in the repository, or in `~/.config/orbit/profiles/`.
    #[arg(value_name = "TARGET")]
    pub client: String,

    /// Where to write the integration: the repository or the user's home.
    #[arg(long, value_enum, default_value_t = SetupScope::Project)]
    pub scope: SetupScope,

    /// Install the SessionStart hook for Claude and Devin. Other clients ignore it.
    #[arg(long)]
    pub hooks: bool,

    /// Deprecated: MCP configuration is installed by default.
    #[arg(long, conflicts_with_all = ["no_mcp"])]
    pub mcp: bool,

    /// Skip MCP configuration and install instruction files only.
    #[arg(long = "no-mcp", conflicts_with_all = ["mcp", "mcp_bin", "portable_mcp"])]
    pub no_mcp: bool,

    /// Orbit executable to record in MCP configs. Defaults to the running binary.
    ///
    /// Prefer --portable-mcp when you want the bare `orbit` command.
    #[arg(long = "mcp-bin", value_name = "PATH")]
    pub mcp_bin: Option<PathBuf>,

    /// Write MCP configs with the portable `orbit` command instead of an absolute path.
    ///
    /// The editor or agent that launches the MCP server must be able to find
    /// `orbit` on PATH.
    #[arg(
        long = "portable-mcp",
        conflicts_with_all = ["mcp_bin", "no_mcp", "remove"]
    )]
    pub portable_mcp: bool,

    /// Preview the files that would change without writing them.
    #[arg(long)]
    pub print: bool,

    /// Report whether the integration is installed, missing, or stale.
    #[arg(long, conflicts_with_all = ["print", "remove"])]
    pub check: bool,

    /// Remove Orbit-managed sections, files, and hooks.
    #[arg(long)]
    pub remove: bool,
}

#[derive(Debug, Args)]
pub struct TriageArgs {
    #[command(subcommand)]
    pub command: TriageCommand,
}

#[derive(Debug, Subcommand)]
pub enum TriageCommand {
    /// Load task context, project guidance, and allowed triage fields.
    #[command(after_long_help = help::TRIAGE_CONTEXT_EXAMPLES)]
    Context {
        #[arg(value_name = "TASK")]
        task: String,
    },
    /// Validate and review a proposal JSON file or stdin without publishing.
    #[command(after_long_help = help::TRIAGE_REVIEW_EXAMPLES)]
    Review(TriageReviewArgs),
    /// Apply an approved proposal to Orbit only.
    #[command(after_long_help = help::TRIAGE_APPLY_EXAMPLES)]
    Apply(TriageApplyArgs),
}

#[derive(Debug, Args)]
pub struct TriageReviewArgs {
    #[arg(value_name = "TASK")]
    pub task: String,
    /// Proposal JSON path, or `-` to read from stdin.
    #[arg(long, value_name = "PATH")]
    pub proposal_file: PathBuf,
}

#[derive(Debug, Args)]
pub struct TriageApplyArgs {
    #[arg(value_name = "TASK")]
    pub task: String,
    #[arg(long, value_name = "PATH")]
    pub proposal_file: PathBuf,
    /// Required after reviewing the preview; writes Orbit metadata only.
    #[arg(long)]
    pub approve: bool,
}

#[derive(Debug, Args)]
pub struct RemoteArgs {
    #[command(subcommand)]
    pub command: RemoteCommand,
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Report Remote Tracker health without changing the Board or tracker.
    #[command(after_long_help = help::REMOTE_STATUS_EXAMPLES)]
    Status(RemoteStatusArgs),
    /// Detect the origin Remote Project and connect its Remote Tracker, or
    /// configure the connected tracker's mapped fields.
    #[command(after_long_help = help::REMOTE_CONNECT_EXAMPLES)]
    Connect(RemoteConnectArgs),
    /// Disconnect a Remote Project from this Board.
    #[command(after_long_help = help::REMOTE_DISCONNECT_EXAMPLES)]
    Disconnect(RemoteProjectArgs),
    /// List the Remote Projects connected to this Board.
    #[command(after_long_help = help::REMOTE_LIST_EXAMPLES)]
    List,
    /// Browse and refresh the Remote Issue catalog.
    #[command(after_long_help = help::REMOTE_ISSUES_EXAMPLES)]
    Issues(RemoteIssuesArgs),
    /// Preview selected Remote Issues or explicitly apply their import.
    #[command(after_long_help = help::REMOTE_IMPORT_EXAMPLES)]
    Import(RemoteImportArgs),
    /// Link a Local Task to a Remote Issue.
    #[command(after_long_help = help::REMOTE_LINK_EXAMPLES)]
    Link(RemoteLinkArgs),
    /// Preview or create Remote Issues for selected Local Tasks.
    #[command(after_long_help = help::REMOTE_CREATE_EXAMPLES)]
    Create(RemoteCreateArgs),
    /// Preview or detach a Linked Task from its Remote Issue.
    #[command(after_long_help = help::REMOTE_DETACH_EXAMPLES)]
    Detach(RemoteMutationArgs),
    /// Preview or reattach a Local Task by its inactive Remote provenance.
    #[command(after_long_help = help::REMOTE_REATTACH_EXAMPLES)]
    Reattach(RemoteMutationArgs),
    /// Preview or apply synchronization for Linked Tasks.
    #[command(after_long_help = help::REMOTE_SYNC_EXAMPLES)]
    Sync(RemoteSyncArgs),
    /// Enable or disable automatic push of Task changes to Remote Issues.
    #[command(after_long_help = help::REMOTE_AUTO_PUSH_EXAMPLES)]
    AutoPush(RemoteAutoPushArgs),
}

#[derive(Debug, Args)]
pub struct RemoteStatusArgs {
    /// Limit checks to Linked Tasks on one repository (`owner/repo`).
    /// Other repositories are skipped entirely, which keeps the report
    /// fast on multi-repository Boards.
    #[arg(long, value_name = "OWNER/REPO")]
    pub repo: Option<String>,
}

#[derive(Debug, Args)]
pub struct RemoteConnectArgs {
    /// Remote Project to connect: `TRACKER:PROJECT` or `PROJECT` when the
    /// tracker is unambiguous. Omit to detect the `origin` remote instead.
    #[arg(value_name = "REMOTE_PROJECT")]
    pub project: Option<String>,
    /// Configure the connected Remote Tracker's mapped fields instead of
    /// detecting `origin`; resolves and stores field ids.
    #[arg(long)]
    pub fields: bool,
    /// Validate configuration without writing field ids.
    #[arg(long, conflicts_with = "create_fields")]
    pub validate_only: bool,
    /// Create missing organization Issue Fields (requires organization admin access).
    #[arg(long)]
    pub create_fields: bool,
}

#[derive(Debug, Args)]
pub struct RemoteProjectArgs {
    /// Remote Project identity: `TRACKER:PROJECT` or `PROJECT` when the
    /// tracker is unambiguous (for GitHub, `owner/repo`).
    #[arg(value_name = "REMOTE_PROJECT")]
    pub project: String,
}

#[derive(Debug, Args)]
pub struct RemoteIssuesArgs {
    #[command(subcommand)]
    pub command: Option<RemoteIssuesCommand>,
    /// Filters for the default listing; `issues list` accepts the same
    /// flags directly.
    #[command(flatten)]
    pub filters: RemoteIssueFilters,
}

#[derive(Debug, Default, Args)]
pub struct RemoteIssueFilters {
    /// Filter by Remote Tracker.
    #[arg(long, value_name = "TRACKER")]
    pub tracker: Option<String>,
    /// Filter by Remote Project.
    #[arg(long, value_name = "PROJECT")]
    pub project: Option<String>,
    /// Filter by Remote Issue state.
    #[arg(long, value_parser = ["open", "closed"])]
    pub state: Option<String>,
    /// Require each label. Repeat to require more than one label.
    #[arg(long, value_name = "LABEL", action = clap::ArgAction::Append)]
    pub label: Vec<String>,
    /// Case-insensitive text search in issue titles.
    #[arg(long, value_name = "TEXT")]
    pub text: Option<String>,
    /// Show only Issues linked to Orbit Tasks.
    #[arg(long, conflicts_with = "unlinked")]
    pub linked: bool,
    /// Show only Issues without an Orbit Task link.
    #[arg(long)]
    pub unlinked: bool,
}

#[derive(Debug, Subcommand)]
pub enum RemoteIssuesCommand {
    /// Refresh every connected Remote Project's catalog snapshot.
    #[command(after_long_help = help::REMOTE_ISSUES_REFRESH_EXAMPLES)]
    Refresh,
    /// List cached Remote Issues without fetching their bodies.
    #[command(after_long_help = help::REMOTE_ISSUES_LIST_EXAMPLES)]
    List(RemoteIssueFilters),
    /// Show one Remote Issue, fetched live from its Remote Tracker.
    #[command(after_long_help = help::REMOTE_ISSUES_SHOW_EXAMPLES)]
    Show(RemoteIssueRefArgs),
}

#[derive(Debug, Args)]
pub struct RemoteIssueRefArgs {
    /// Remote Issue identity: `TRACKER:PROJECT#ID`, `PROJECT#ID`, or a
    /// Remote Tracker Issue URL (for GitHub, `OWNER/REPO#NUMBER` or
    /// `https://github.com/OWNER/REPO/issues/NUMBER`).
    #[arg(value_name = "REMOTE_ISSUE")]
    pub issue: String,
}

#[derive(Debug, Args)]
pub struct RemoteImportArgs {
    /// Explicit Remote Issue identities to review; repeat for a batch.
    #[arg(value_name = "REMOTE_ISSUE", required = true, num_args = 1..)]
    pub issues: Vec<String>,
    /// Link this selected Issue to a specific Local Task. Repeat as IDENTITY=TASK.
    #[arg(long, value_name = "REMOTE_ISSUE=TASK", action = clap::ArgAction::Append)]
    pub link: Vec<String>,
    /// Apply the reviewed selection to the Orbit Board. The tracker is never written.
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Args)]
pub struct RemoteLinkArgs {
    #[arg(value_name = "TASK")]
    pub task: String,
    /// Remote Issue identity or URL: `TRACKER:PROJECT#ID`, `PROJECT#ID`,
    /// or a Remote Tracker Issue URL.
    #[arg(value_name = "REMOTE_ISSUE")]
    pub issue: String,
}

#[derive(Debug, Args)]
pub struct RemoteCreateArgs {
    #[arg(
        value_name = "TASK",
        required = true,
        num_args = 1..,
        help = "One or more Local Task IDs to export"
    )]
    pub tasks: Vec<String>,
    #[arg(
        long,
        value_name = "REMOTE_PROJECT",
        help = "Choose a connected Remote Project (inferred only when one is available)"
    )]
    pub project: Option<String>,
    /// Create Issues and link Tasks. Without this flag, show a read-only preview.
    #[arg(long)]
    pub apply: bool,
    /// Include done and cancelled Tasks in the export.
    #[arg(long, alias = "include-terminal")]
    pub include_closed: bool,
}

#[derive(Debug, Args)]
pub struct RemoteMutationArgs {
    /// Remote Issue identity (`TRACKER:PROJECT#ID`, `PROJECT#ID`) or a Task
    /// id whose Remote link should be detached or reattached.
    #[arg(value_name = "REMOTE_ISSUE|TASK")]
    pub target: String,
    /// Apply the reviewed operation. Without this flag, nothing is written.
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum RemoteSyncDirection {
    #[default]
    Both,
    Pull,
    Push,
}

#[derive(Debug, Args)]
pub struct RemoteSyncArgs {
    #[arg(value_name = "TASK")]
    pub task: Option<String>,
    #[arg(long, value_enum, default_value_t = RemoteSyncDirection::Both)]
    pub direction: RemoteSyncDirection,
    /// Apply the planned sync. Without this flag the command is a dry run.
    #[arg(long)]
    pub apply: bool,
    /// Resolve a field conflict as `FIELD=orbit` or `FIELD=remote`. Repeat as needed.
    #[arg(long = "resolve", value_name = "FIELD=orbit|remote", action = clap::ArgAction::Append)]
    pub resolutions: Vec<String>,
}

#[derive(Debug, Args)]
pub struct RemoteAutoPushArgs {
    /// Turn automatic push on or off.
    #[arg(value_name = "STATE")]
    pub state: AutoPushState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AutoPushState {
    #[value(alias = "enable", alias = "true")]
    On,
    #[value(alias = "disable", alias = "false")]
    Off,
}

#[derive(Debug, Args)]
pub struct SchemaArgs {
    /// Command path or MCP tool name to describe instead of the whole registry.
    #[arg(value_name = "COMMAND")]
    pub command: Option<String>,

    /// Print the generated docs/commands.md reference section instead of JSON.
    #[arg(long)]
    pub docs: bool,
}

#[derive(Debug, Args)]
pub struct BoardArgs {
    #[command(subcommand)]
    pub command: BoardCommand,
}

#[derive(Debug, Subcommand)]
pub enum BoardCommand {
    /// Show, set, or clear the Board host repository.
    ///
    /// The host is a member Repository name/path or another git URL whose
    /// remote stores this Board's `orbit` branch. With no argument the current
    /// host is shown; `--none` clears it without deleting the remote branch.
    #[command(long_about = help::BOARD_HOST_LONG, after_long_help = help::BOARD_HOST_EXAMPLES)]
    Host(BoardHostArgs),

    /// Fetch the host, merge Orbit-aware, and push the `orbit` branch.
    ///
    /// Different Tasks and different fields merge; Comments union; the earlier
    /// Claim wins and the loser is told; a same-field conflict stops the sync
    /// until `--resolve FIELD=local|host` picks a side. History is merged,
    /// never rebased or force-pushed.
    #[command(long_about = help::BOARD_SYNC_LONG, after_long_help = help::BOARD_SYNC_EXAMPLES)]
    Sync(BoardSyncArgs),

    /// Move the Board folder to DIR and repoint every anchor at it.
    ///
    /// Rewrites the `.orbit/board` pointer (or restores the hidden layout when
    /// DIR is `.orbit/board`), hides a folder inside a Git working tree through
    /// `.git/info/exclude`, and repoints member Repository links. History moves
    /// with the folder.
    #[command(long_about = help::BOARD_MOVE_LONG, after_long_help = help::BOARD_MOVE_EXAMPLES)]
    Move(BoardMoveArgs),
}

#[derive(Debug, Args)]
pub struct BoardMoveArgs {
    /// New Board folder; must not exist or be an empty directory.
    #[arg(value_name = "DIR")]
    pub target: PathBuf,
}

#[derive(Debug, Args)]
pub struct BoardHostArgs {
    /// Member Repository name/path or git URL to host the Board.
    #[arg(value_name = "REPO|URL")]
    pub target: Option<String>,

    /// Clear the host without deleting the remote `orbit` branch.
    #[arg(long, conflicts_with = "target")]
    pub none: bool,
}

#[derive(Debug, Args)]
pub struct BoardSyncArgs {
    /// Resolve a field conflict as `FIELD=local` or `FIELD=host`. Repeat as needed.
    #[arg(long = "resolve", value_name = "FIELD=local|host", action = clap::ArgAction::Append)]
    pub resolutions: Vec<String>,
}

/// Parse the process arguments. `orbit list --help` (and `orbit help list`)
/// drops the Remote Tracker flags and remote column docs when the resolved
/// Board has no Remote material; the flags themselves keep working.
pub fn parse() -> Cli {
    let mut command = Cli::command();
    let (list_help, dir) = pre_scan();
    if list_help && !crate::remote::details_visible_for(dir.as_deref()) {
        command = command.mut_subcommand("list", |list| {
            list.mut_arg("linked", |arg| arg.hide(true))
                .mut_arg("local", |arg| arg.hide(true))
                .long_about(help::list_long(false))
                .after_long_help(help::list_examples(false))
        });
    }
    let matches = command.get_matches();
    Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit())
}

/// Best-effort scan of the raw arguments for a `list` help request and the
/// `--dir`/`ORBIT_DIR` override, so help can hide Remote Tracker material
/// before clap renders it. Anything clap would reject is left for clap to
/// report; a missed detection only means the full help is shown.
fn pre_scan() -> (bool, Option<PathBuf>) {
    let mut dir = std::env::var_os("ORBIT_DIR").map(PathBuf::from);
    let mut subcommand: Option<String> = None;
    let mut list_help = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy().into_owned();
        if arg == "--dir" {
            if let Some(value) = args.next() {
                dir = Some(PathBuf::from(value));
            }
            continue;
        }
        if let Some(value) = arg.strip_prefix("--dir=") {
            dir = Some(PathBuf::from(value));
            continue;
        }
        let Some(name) = subcommand.as_deref() else {
            if !arg.starts_with('-') {
                subcommand = Some(arg);
            }
            continue;
        };
        match (name, arg.as_str()) {
            ("list", "--help" | "-h") | ("help", "list") => list_help = true,
            _ => {}
        }
    }
    (list_help, dir)
}
