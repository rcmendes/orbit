use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::help;
use crate::setup::{SetupClient, SetupScope};
use crate::task::{Status, TaskArea, TaskType};

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

    /// Orbit folder (DIR) override. Also read from ORBIT_DIR.
    #[arg(global = true, long, env = "ORBIT_DIR", value_name = "PATH")]
    pub dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create the Board and wire coding-agent harnesses.
    #[command(long_about = help::INIT_LONG, after_long_help = help::INIT_EXAMPLES)]
    Init(InitArgs),

    /// Point this repository at a shared Board.
    #[command(long_about = help::LINK_LONG, after_long_help = help::LINK_EXAMPLES)]
    Link(LinkArgs),

    /// Return this repository to its own Board.
    #[command(long_about = help::UNLINK_LONG, after_long_help = help::UNLINK_EXAMPLES)]
    Unlink,

    /// Show or migrate the Board's Task-ID slug.
    #[command(long_about = help::SLUG_LONG, after_long_help = help::SLUG_EXAMPLES)]
    Slug(SlugArgs),

    /// Create a task.
    #[command(long_about = help::ADD_LONG, after_long_help = help::ADD_EXAMPLES)]
    Add(AddArgs),

    /// List tasks with filters. Closed work is hidden unless --all.
    #[command(long_about = help::LIST_LONG, after_long_help = help::LIST_EXAMPLES)]
    List(ListArgs),

    /// List or atomically claim the next ready task.
    #[command(long_about = help::READY_LONG, after_long_help = help::READY_EXAMPLES)]
    Ready(ReadyArgs),

    /// Show a task with its body, dependencies, and comments.
    #[command(long_about = help::SHOW_LONG, after_long_help = help::SHOW_EXAMPLES)]
    Show(ShowArgs),

    /// Show a Parent Task, its SubTasks, and blocking edges.
    #[command(long_about = help::TREE_LONG, after_long_help = help::TREE_EXAMPLES)]
    Tree(TreeArgs),

    /// Claim a specific task and start its lease.
    #[command(long_about = help::CLAIM_LONG, after_long_help = help::CLAIM_EXAMPLES)]
    Claim(ClaimArgs),

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

    /// Edit a task's title, body, priority, or external blocker.
    #[command(long_about = help::UPDATE_LONG, after_long_help = help::UPDATE_EXAMPLES)]
    Update(UpdateArgs),

    /// Add, remove, or list task labels.
    #[command(long_about = help::LABEL_LONG, after_long_help = help::LABEL_EXAMPLES)]
    Label(LabelArgs),

    /// Add to or read a task's comment thread.
    #[command(long_about = help::COMMENT_LONG, after_long_help = help::COMMENT_EXAMPLES)]
    Comment(CommentArgs),

    /// Make one task wait for another.
    #[command(long_about = help::DEPEND_LONG, after_long_help = help::DEPEND_EXAMPLES)]
    Depend(DependArgs),

    /// Remove a dependency edge between two tasks.
    #[command(long_about = help::UNDEPEND_LONG, after_long_help = help::UNDEPEND_EXAMPLES)]
    Undepend(UndependArgs),

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

    /// Link GitHub repositories and synchronize Issues with Tasks.
    #[command(name = "github", long_about = help::GITHUB_LONG, after_long_help = help::GITHUB_EXAMPLES)]
    GitHub(GitHubArgs),

    /// Review and apply agent triage proposals for GitHub-linked Tasks.
    #[command(long_about = help::TRIAGE_LONG, after_long_help = help::TRIAGE_EXAMPLES)]
    Triage(TriageArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Orbit folder (DIR) to create or reuse. Defaults to ./.orbit.
    #[arg(value_name = "DIR")]
    pub target: Option<PathBuf>,

    /// Create or reuse the global Board at ~/.orbit.
    #[arg(long, conflicts_with = "target")]
    pub global: bool,

    /// Which harnesses to wire: auto, all, none, ask, or a comma-separated list.
    #[arg(long, value_name = "SELECTOR", conflicts_with = "no_agents")]
    pub agents: Option<String>,

    /// Do not write any harness integration.
    #[arg(long = "no-agents", conflicts_with = "agents")]
    pub no_agents: bool,

    /// Install the Claude Code SessionStart hook.
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

    /// Preview the files that would change without writing them; harness
    /// selection does not prompt unless --agents ask is given.
    #[arg(long)]
    pub print: bool,
}

#[derive(Debug, Args)]
pub struct LinkArgs {
    /// Orbit folder (DIR), or a legacy Board or workspace directory.
    #[arg(value_name = "PATH")]
    pub path: PathBuf,

    /// Do not refresh existing agent configuration.
    #[arg(long = "no-refresh")]
    pub no_refresh: bool,
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

    /// Priority from 0 (highest) to 3 (lowest).
    #[arg(short = 'p', long, value_name = "N")]
    pub priority: Option<u8>,

    /// Work shape: task, epic, spike, chore, or decision.
    #[arg(long = "type", value_name = "TYPE", value_enum)]
    pub task_type: Option<TaskType>,

    /// Make the new task wait for an existing task. Repeatable.
    #[arg(long = "depends-on", value_name = "ID", action = clap::ArgAction::Append)]
    pub depends_on: Vec<String>,

    /// Create a SubTask under this Parent Task. SubTasks are one level deep.
    #[arg(long, value_name = "ID")]
    pub parent: Option<String>,

    /// Record a blocker that lives outside the Board.
    #[arg(long, value_name = "TEXT")]
    pub waiting_on: Option<String>,

    /// Initial labels. Repeatable.
    #[arg(long, value_name = "LABEL", action = clap::ArgAction::Append)]
    pub label: Vec<String>,

    /// Stable work area: board, cli, mcp, setup, docs, or release.
    #[arg(long, value_name = "AREA")]
    pub area: Option<String>,

    /// Concise verification checks or commands for agents.
    #[arg(long, value_name = "TEXT")]
    pub verification: Option<String>,

    /// Task body. Use "-" to read stdin.
    #[arg(long, value_name = "TEXT", conflicts_with = "body_file")]
    pub body: Option<String>,

    /// Read the task body from a file. Use "-" to read stdin.
    #[arg(long = "body-file", value_name = "PATH", conflicts_with = "body")]
    pub body_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Filter by status. Requesting done or cancelled includes closed work.
    #[arg(long, value_enum)]
    pub status: Option<Status>,

    /// Filter by priority (0-3).
    #[arg(long, value_name = "N")]
    pub priority: Option<u8>,

    /// Filter by claim holder.
    #[arg(long = "claimed-by", value_name = "NAME")]
    pub claimed_by: Option<String>,

    /// Only tasks waiting on an external blocker.
    #[arg(long)]
    pub waiting: bool,

    /// Filter by work shape. Repeatable; matches any listed type.
    #[arg(long = "type", value_name = "TYPE", value_enum, action = clap::ArgAction::Append)]
    pub task_type: Vec<TaskType>,

    /// Exclude work shapes. Repeatable; excludes any listed type.
    #[arg(long = "exclude-type", value_name = "TYPE", value_enum, action = clap::ArgAction::Append)]
    pub exclude_type: Vec<TaskType>,

    /// Filter by work area. Repeatable; matches any listed area.
    #[arg(long, value_name = "AREA", value_enum, action = clap::ArgAction::Append)]
    pub area: Vec<TaskArea>,

    /// Exclude tasks with any listed area.
    #[arg(long = "exclude-area", value_name = "AREA", value_enum, action = clap::ArgAction::Append)]
    pub exclude_area: Vec<TaskArea>,

    /// Filter by label. Repeatable; every label must match.
    #[arg(long, value_name = "LABEL", action = clap::ArgAction::Append)]
    pub label: Vec<String>,

    /// Exclude tasks carrying any of these labels. Repeatable.
    #[arg(long = "exclude-label", value_name = "LABEL", action = clap::ArgAction::Append)]
    pub exclude_label: Vec<String>,

    /// Filter by repository label. Shorthand for --label repo:NAME.
    #[arg(long, value_name = "NAME")]
    pub repo: Option<String>,

    /// Only tasks with no labels.
    #[arg(long, conflicts_with_all = ["label", "repo"])]
    pub unlabeled: bool,

    /// Filter to SubTasks of this Parent Task.
    #[arg(long, value_name = "ID")]
    pub parent: Option<String>,

    /// Group tasks under their Parent Task and render the grouped table.
    #[arg(long)]
    pub group: bool,

    /// Include done and cancelled tasks.
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct ReadyArgs {
    /// Maximum number of tasks to list.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,

    /// Only Ready SubTasks of this Parent Task.
    #[arg(long, value_name = "ID")]
    pub parent: Option<String>,

    /// Only Ready tasks with this repository label. Shorthand for --label repo:NAME.
    #[arg(long, value_name = "NAME")]
    pub repo: Option<String>,

    /// Only Ready tasks with one of these work shapes. Repeatable.
    #[arg(long = "type", value_name = "TYPE", value_enum, action = clap::ArgAction::Append)]
    pub task_type: Vec<TaskType>,

    /// Exclude Ready tasks with any of these work shapes. Repeatable.
    #[arg(long = "exclude-type", value_name = "TYPE", value_enum, action = clap::ArgAction::Append)]
    pub exclude_type: Vec<TaskType>,

    /// Only Ready tasks with any listed work area. Repeatable.
    #[arg(long, value_name = "AREA", value_enum, action = clap::ArgAction::Append)]
    pub area: Vec<TaskArea>,

    /// Exclude Ready tasks with any listed work area. Repeatable.
    #[arg(long = "exclude-area", value_name = "AREA", value_enum, action = clap::ArgAction::Append)]
    pub exclude_area: Vec<TaskArea>,

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
    /// Task ID, for example orb-a7b2c9.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct TreeArgs {
    /// Parent Task ID, for example orb-a7b2c9.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct ClaimArgs {
    /// Task ID, for example orb-a7b2c9.
    pub id: String,

    /// Agent identity. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,

    /// Take over another agent's claim, even while its lease is live.
    #[arg(long)]
    pub steal: bool,
}

#[derive(Debug, Args)]
pub struct ReleaseArgs {
    /// Task ID, for example orb-a7b2c9.
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
    /// Task ID, for example orb-a7b2c9.
    pub id: String,

    /// Agent identity. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,
}

#[derive(Debug, Args)]
pub struct CheckpointArgs {
    /// Task ID, for example orb-a7b2c9.
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
    /// Task ID, for example orb-a7b2c9.
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
    /// Task ID, for example orb-a7b2c9.
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
    /// Task ID, for example orb-a7b2c9.
    pub id: String,

    /// Reopen even when a dependent task is already done.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Task ID, for example orb-a7b2c9.
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

    /// New priority (0-3).
    #[arg(long, value_name = "N")]
    pub priority: Option<u8>,

    /// New work shape: task, epic, spike, chore, or decision.
    #[arg(long = "type", value_name = "TYPE", value_enum)]
    pub task_type: Option<TaskType>,

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

    /// New work area: board, cli, mcp, setup, docs, or release.
    #[arg(long, value_name = "AREA", conflicts_with = "clear_area")]
    pub area: Option<String>,

    /// Remove the work area classification.
    #[arg(long = "clear-area")]
    pub clear_area: bool,

    /// New verification checks or commands for agents.
    #[arg(
        long,
        value_name = "TEXT",
        conflicts_with = "clear_verification"
    )]
    pub verification: Option<String>,

    /// Remove task-level verification guidance.
    #[arg(long = "clear-verification")]
    pub clear_verification: bool,
}

#[derive(Debug, Args)]
pub struct DependArgs {
    /// Task ID, for example orb-a7b2c9.
    pub id: String,

    /// The task that must finish first.
    #[arg(value_name = "DEP_ID")]
    pub dep_id: String,
}

#[derive(Debug, Args)]
pub struct UndependArgs {
    /// Task ID, for example orb-a7b2c9.
    pub id: String,

    /// The dependency to remove.
    #[arg(value_name = "DEP_ID")]
    pub dep_id: String,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Task ID, for example orb-a7b2c9.
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
    /// Task ID, for example orb-a7b2c9.
    pub id: String,

    /// One or more lowercase labels.
    #[arg(required = true, num_args = 1.., value_name = "LABEL")]
    pub labels: Vec<String>,
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
    /// Task ID, for example orb-a7b2c9.
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
    /// Task ID, for example orb-a7b2c9.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Remove stale temp files and re-attach an orphaned orbit worktree.
    #[arg(long, conflicts_with = "migrate")]
    pub fix: bool,

    /// Move a legacy Board to the Orbit folder. Resumable.
    #[arg(long, conflicts_with = "fix")]
    pub migrate: bool,
}

#[derive(Debug, Args)]
pub struct PrimeArgs {
    /// Emit Claude Code's SessionStart hook envelope.
    #[arg(long = "hook-json")]
    pub hook_json: bool,

    /// Agent identity used in the context. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,

    /// Only Ready tasks with this repository label. Shorthand for --label repo:NAME.
    #[arg(long, value_name = "NAME")]
    pub repo: Option<String>,

    /// Only Ready tasks carrying every label. Repeatable.
    #[arg(long, value_name = "LABEL", action = clap::ArgAction::Append)]
    pub label: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Which agent integration to install.
    #[arg(value_enum)]
    pub client: SetupClient,

    /// Where to write the integration: the repository or the user's home.
    #[arg(long, value_enum, default_value_t = SetupScope::Project)]
    pub scope: SetupScope,

    /// Install the Claude Code SessionStart hook. Other clients ignore it.
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
    /// Validate a proposal JSON file and preview Orbit metadata changes.
    #[command(after_long_help = help::TRIAGE_REVIEW_EXAMPLES)]
    Review(TriageReviewArgs),
    /// Apply an approved proposal to Orbit (not GitHub).
    #[command(after_long_help = help::TRIAGE_APPLY_EXAMPLES)]
    Apply(TriageApplyArgs),
}

#[derive(Debug, Args)]
pub struct TriageReviewArgs {
    #[arg(value_name = "TASK")]
    pub task: String,
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
pub struct GitHubArgs {
    #[command(subcommand)]
    pub command: GitHubCommand,
}

#[derive(Debug, Subcommand)]
pub enum GitHubCommand {
    /// Detect the origin repository and configure GitHub for this Board.
    #[command(after_long_help = help::GITHUB_CONNECT_EXAMPLES)]
    Connect,
    /// Validate org Issue Fields, optionally create missing fields, and persist their ids.
    #[command(after_long_help = help::GITHUB_SETUP_EXAMPLES)]
    Setup(GitHubSetupArgs),
    /// Manage explicitly linked GitHub repositories.
    #[command(subcommand, after_long_help = help::GITHUB_REPO_LINK_EXAMPLES)]
    Repo(GitHubRepoCommand),
    /// Adopt an existing GitHub Issue for a Task.
    #[command(after_long_help = help::GITHUB_ADOPT_EXAMPLES)]
    Adopt(GitHubAdoptArgs),
    /// Create a GitHub Issue for a linked Task.
    #[command(after_long_help = help::GITHUB_CREATE_EXAMPLES)]
    Create(GitHubCreateArgs),
    /// Preview or apply bidirectional sync for linked Tasks.
    #[command(after_long_help = help::GITHUB_SYNC_CMD_EXAMPLES)]
    Sync(GitHubSyncArgs),
}

#[derive(Debug, Subcommand)]
pub enum GitHubRepoCommand {
    /// Link a repository to this Board.
    #[command(after_long_help = help::GITHUB_REPO_LINK_EXAMPLES)]
    Link(GitHubRepoLinkArgs),
    /// Remove a linked repository.
    #[command(after_long_help = help::GITHUB_REPO_LINK_EXAMPLES)]
    Unlink(GitHubRepoLinkArgs),
    /// List linked repositories.
    #[command(after_long_help = help::GITHUB_REPO_LINK_EXAMPLES)]
    List,
}

#[derive(Debug, Args)]
pub struct GitHubSetupArgs {
    /// Validate configuration without writing field ids.
    #[arg(long, conflicts_with = "create_fields")]
    pub validate_only: bool,
    /// Create missing organization Issue Fields (requires organization admin access).
    #[arg(long)]
    pub create_fields: bool,
}

#[derive(Debug, Args)]
pub struct GitHubRepoLinkArgs {
    #[arg(value_name = "OWNER/REPO")]
    pub repository: String,
}

#[derive(Debug, Args)]
pub struct GitHubAdoptArgs {
    #[arg(value_name = "TASK")]
    pub task: String,
    /// Standard GitHub Issue URL, for example https://github.com/acme/widget/issues/42.
    #[arg(value_name = "GITHUB_ISSUE_URL")]
    pub issue_url: Option<String>,
    /// Repository for the legacy --repo/--number form, or to confirm the URL.
    #[arg(long, value_name = "OWNER/REPO")]
    pub repo: Option<String>,
    /// Issue number for the legacy --repo/--number form, or to confirm the URL.
    #[arg(long)]
    pub number: Option<u64>,
}

#[derive(Debug, Args)]
pub struct GitHubCreateArgs {
    #[arg(value_name = "TASK")]
    pub task: String,
    #[arg(long, value_name = "OWNER/REPO")]
    pub repo: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum GitHubSyncDirection {
    #[default]
    Both,
    Pull,
    Push,
}

#[derive(Debug, Args)]
pub struct GitHubSyncArgs {
    #[arg(value_name = "TASK")]
    pub task: Option<String>,
    #[arg(long, value_enum, default_value_t = GitHubSyncDirection::Both)]
    pub direction: GitHubSyncDirection,
    /// Apply the planned sync. Without this flag the command is a dry run.
    #[arg(long)]
    pub apply: bool,
}
