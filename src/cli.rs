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
    /// Initialize a board in the current repository, or in --dir.
    Init,

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

    /// Write project-local MCP server configuration.
    #[arg(long)]
    pub mcp: bool,

    /// Preview the files that would change without writing them.
    #[arg(long)]
    pub print: bool,

    /// Remove Orbit-managed sections, files, and hooks.
    #[arg(long)]
    pub remove: bool,
}
