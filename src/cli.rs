use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::setup::SetupClient;
use crate::task::Status;

#[derive(Debug, Parser)]
#[command(
    name = "orbit",
    version,
    about = "A Markdown-native work queue for coding agents",
    long_about = None
)]
pub struct Cli {
    /// Emit a single JSON object on stdout.
    #[arg(global = true, long)]
    pub json: bool,

    /// Board root override. Also read from ORBIT_DIR.
    #[arg(global = true, long, env = "ORBIT_DIR", value_name = "PATH")]
    pub dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize a Board and wire coding-agent harnesses.
    Init(InitArgs),

    /// Point this repository at a shared Board.
    Link(LinkArgs),

    /// Return this repository to its own Board.
    Unlink,

    /// Create a task.
    Add(AddArgs),

    /// List tasks. Terminal tasks are hidden unless --all.
    List(ListArgs),

    /// List ready tasks, optionally claiming the first one atomically.
    Ready(ReadyArgs),

    /// Show a task, including its body.
    Show(ShowArgs),

    /// Claim a specific task.
    Claim(ClaimArgs),

    /// Release a claim and return the task to todo.
    Release(ReleaseArgs),

    /// Extend a claim's lease.
    Heartbeat(HeartbeatArgs),

    /// Complete a task.
    Done(DoneArgs),

    /// Cancel a task.
    Cancel(CancelArgs),

    /// Reopen a done or cancelled task.
    Reopen(ReopenArgs),

    /// Change a task's title, body, priority, or external wait.
    Update(UpdateArgs),

    /// Add, remove, or list task labels.
    Label(LabelArgs),

    /// Add or list task comments.
    Comment(CommentArgs),

    /// Add a dependency.
    Depend(DependArgs),

    /// Remove a dependency.
    Undepend(UndependArgs),

    /// Delete a task.
    Delete(DeleteArgs),

    /// Print agent workflow context.
    Prime(PrimeArgs),

    /// Install agent instructions, skills, and hooks.
    Setup(SetupArgs),

    /// Start the MCP stdio server.
    Mcp,

    /// Check the board for integrity issues.
    Doctor(DoctorArgs),

    /// Commit pending lease-only changes to the board branch.
    Sync,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Create or link a shared Board at PATH.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["local", "dir"])]
    pub board: Option<PathBuf>,

    /// Which harnesses to wire: auto, all, none, ask, or a comma-separated list.
    #[arg(long, value_name = "SELECTOR", conflicts_with = "no_agents")]
    pub agents: Option<String>,

    /// Do not write any harness integration.
    #[arg(long = "no-agents", conflicts_with = "agents")]
    pub no_agents: bool,

    /// Install the Claude Code SessionStart hook.
    #[arg(long)]
    pub hooks: bool,

    /// Create this repository's own Board even when it is linked.
    #[arg(long, conflicts_with = "board")]
    pub local: bool,

    /// Orbit executable to record in agent configs. Defaults to the running binary.
    #[arg(long = "mcp-bin", value_name = "PATH")]
    pub mcp_bin: Option<PathBuf>,

    /// Preview the files that would change without writing them.
    #[arg(long)]
    pub print: bool,
}

#[derive(Debug, Args)]
pub struct LinkArgs {
    /// Board directory to link this repository to.
    #[arg(value_name = "PATH")]
    pub path: PathBuf,

    /// Do not refresh existing agent configuration.
    #[arg(long = "no-refresh")]
    pub no_refresh: bool,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// Task title.
    pub title: String,

    /// Priority from 0 (highest) to 3 (lowest).
    #[arg(short = 'p', long, value_name = "N")]
    pub priority: Option<u8>,

    /// Make the new task depend on an existing task. Repeatable.
    #[arg(long = "depends-on", value_name = "ID", action = clap::ArgAction::Append)]
    pub depends_on: Vec<String>,

    /// Record an external blocker.
    #[arg(long, value_name = "TEXT")]
    pub waiting_on: Option<String>,

    /// Initial labels. Repeatable.
    #[arg(long, value_name = "LABEL", action = clap::ArgAction::Append)]
    pub label: Vec<String>,

    /// Task body. Use "-" to read stdin.
    #[arg(long, value_name = "TEXT", conflicts_with = "body_file")]
    pub body: Option<String>,

    /// Read the task body from a file. Use "-" to read stdin.
    #[arg(long = "body-file", value_name = "PATH", conflicts_with = "body")]
    pub body_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Filter by status.
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

    /// Filter by label.
    #[arg(long, value_name = "LABEL")]
    pub label: Option<String>,

    /// Only tasks with no labels.
    #[arg(long, conflicts_with = "label")]
    pub unlabeled: bool,

    /// Include done and cancelled tasks.
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct ReadyArgs {
    /// Maximum number of tasks to list.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,

    /// Claim the first ready task atomically.
    #[arg(long)]
    pub claim: bool,

    /// Agent identity for --claim.
    #[arg(long, value_name = "NAME", requires = "claim")]
    pub by: Option<String>,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    pub id: String,
}

#[derive(Debug, Args)]
pub struct ClaimArgs {
    pub id: String,

    /// Agent identity. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,

    /// Take over a live or stale claim.
    #[arg(long)]
    pub steal: bool,
}

#[derive(Debug, Args)]
pub struct ReleaseArgs {
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
    pub id: String,

    /// Agent identity. Defaults to ORBIT_AGENT, then USER.
    #[arg(long, value_name = "NAME")]
    pub by: Option<String>,
}

#[derive(Debug, Args)]
pub struct DoneArgs {
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
    pub id: String,

    /// Reopen even when a dependent task is already done.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
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
}

#[derive(Debug, Args)]
pub struct DependArgs {
    pub id: String,

    /// The task that must finish first.
    #[arg(value_name = "DEP_ID")]
    pub dep_id: String,
}

#[derive(Debug, Args)]
pub struct UndependArgs {
    pub id: String,

    #[arg(value_name = "DEP_ID")]
    pub dep_id: String,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
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
    Add(LabelMutationArgs),
    /// Remove one or more labels from a task.
    Remove(LabelMutationArgs),
    /// List labels in use and their task counts.
    List,
}

#[derive(Debug, Args)]
pub struct LabelMutationArgs {
    pub id: String,

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
    Add(CommentAddArgs),
    /// List a task's comments.
    List(CommentListArgs),
}

#[derive(Debug, Args)]
pub struct CommentAddArgs {
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
    pub id: String,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Remove stale temp files.
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
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Which agent integration to install.
    #[arg(value_enum)]
    pub client: SetupClient,

    /// Install the Claude Code SessionStart hook. Other clients ignore it.
    #[arg(long)]
    pub hooks: bool,

    /// Deprecated: MCP configuration is installed by default.
    #[arg(long, conflicts_with_all = ["no_mcp"])]
    pub mcp: bool,

    /// Skip MCP configuration and install instruction files only.
    #[arg(long = "no-mcp", conflicts_with_all = ["mcp", "mcp_bin"])]
    pub no_mcp: bool,

    /// Orbit executable to record in MCP configs. Defaults to the running binary.
    ///
    /// Pass `orbit` to write a portable command that relies on the editor's PATH.
    #[arg(long = "mcp-bin", value_name = "PATH")]
    pub mcp_bin: Option<PathBuf>,

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
