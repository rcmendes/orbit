use std::path::PathBuf;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};

use crate::help;
use crate::setup::{SetupClient, SetupScope};
use crate::task::{Status, TaskType};

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

    /// Workspace root or Orbit Board directory override. Also read from ORBIT_DIR.
    #[arg(global = true, long, env = "ORBIT_DIR", value_name = "PATH")]
    pub dir: Option<PathBuf>,

    /// Do not auto-push task changes to the Remote Tracker or sync the Board
    /// host for this command. Hidden from help; still functional.
    #[arg(global = true, long = "no-sync", hide = true)]
    pub no_sync: bool,

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
    /// the `repo` field, and seeding `board.areas` — all in one commit.
    #[command(long_about = help::BOARD_MIGRATE_LONG, after_long_help = help::BOARD_MIGRATE_EXAMPLES)]
    Migrate(BoardMigrateArgs),

    /// Create a task.
    #[command(long_about = help::ADD_LONG, after_long_help = help::ADD_EXAMPLES)]
    Add(AddArgs),

    /// List tasks with filters. Closed work is hidden unless --include-closed.
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

    /// Configure and synchronize the Board's host repository.
    #[command(long_about = help::BOARD_LONG, after_long_help = help::BOARD_EXAMPLES)]
    Board(BoardArgs),

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
    ///
    /// A path that is an existing Git working-tree root, such as `.`, initializes
    /// that checkout's ./.orbit instead of using the root as the Board directory.
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
    /// Bare `--visible` names the folder `board`; `--visible=NAME` picks the
    /// name. Only applies when the target resolves to a repository or Workspace
    /// root — an explicit directory is already a visible Board folder.
    #[arg(
        long,
        value_name = "NAME",
        num_args = 0..=1,
        require_equals = true,
        conflicts_with = "global"
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
pub struct BoardMigrateArgs {
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

    /// Optional work-area slug. Restricted to the Board's `board.areas` when
    /// the Board configures them; any lowercase slug otherwise.
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

    /// Filter by work area slug. Repeatable; matches any listed area.
    #[arg(long, value_name = "AREA", action = clap::ArgAction::Append)]
    pub area: Vec<String>,

    /// Exclude tasks with any listed area.
    #[arg(long = "exclude-area", value_name = "AREA", action = clap::ArgAction::Append)]
    pub exclude_area: Vec<String>,

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

    /// Only Ready SubTasks of this Parent Task.
    #[arg(long, value_name = "ID")]
    pub parent: Option<String>,

    /// Only Ready tasks with this repository label. Shorthand for --label repo:NAME.
    #[arg(long, value_name = "NAME")]
    pub repo: Option<String>,

    /// Include Ready tasks from every Repository. Inside a member Repository
    /// or its worktree the queue is scoped to that Repository plus Board-wide
    /// Tasks by default; --all widens it to the whole Board.
    #[arg(long)]
    pub all: bool,

    /// Only Ready tasks with one of these work shapes. Repeatable.
    #[arg(long = "type", value_name = "TYPE", value_enum, action = clap::ArgAction::Append)]
    pub task_type: Vec<TaskType>,

    /// Exclude Ready tasks with any of these work shapes. Repeatable.
    #[arg(long = "exclude-type", value_name = "TYPE", value_enum, action = clap::ArgAction::Append)]
    pub exclude_type: Vec<TaskType>,

    /// Only Ready tasks with any listed work area. Repeatable.
    #[arg(long, value_name = "AREA", action = clap::ArgAction::Append)]
    pub area: Vec<String>,

    /// Exclude Ready tasks with any listed work area. Repeatable.
    #[arg(long = "exclude-area", value_name = "AREA", action = clap::ArgAction::Append)]
    pub exclude_area: Vec<String>,

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

    /// New work area slug. Restricted to the Board's `board.areas` when the
    /// Board configures them; any lowercase slug otherwise.
    #[arg(long, value_name = "AREA", conflicts_with = "clear_area")]
    pub area: Option<String>,

    /// Remove the work area classification.
    #[arg(long = "clear-area")]
    pub clear_area: bool,

    /// New verification checks or commands for agents.
    #[arg(long, value_name = "TEXT", conflicts_with = "clear_verification")]
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
    /// Validate and review a proposal JSON file or stdin without writing to GitHub.
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
pub struct GitHubArgs {
    #[command(subcommand)]
    pub command: GitHubCommand,
}

#[derive(Debug, Subcommand)]
pub enum GitHubCommand {
    /// Report GitHub integration health without changing the Board or GitHub.
    #[command(after_long_help = help::GITHUB_STATUS_EXAMPLES)]
    Status,
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
    /// Preview or create GitHub Issues for selected Local Tasks.
    #[command(after_long_help = help::GITHUB_CREATE_EXAMPLES)]
    Create(GitHubCreateArgs),
    /// Preview or apply bidirectional sync for linked Tasks.
    #[command(after_long_help = help::GITHUB_SYNC_CMD_EXAMPLES)]
    Sync(GitHubSyncArgs),
    /// Enable or disable automatic push of Task changes to linked Issues.
    #[command(after_long_help = help::GITHUB_AUTO_PUSH_EXAMPLES)]
    AutoPush(GitHubAutoPushArgs),
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
    #[arg(
        value_name = "TASK",
        required = true,
        num_args = 1..,
        help = "One or more Local Task IDs to export"
    )]
    pub tasks: Vec<String>,
    #[arg(
        long,
        value_name = "OWNER/REPO",
        help = "Choose an included repository (inferred only when one is available)"
    )]
    pub repo: Option<String>,
    /// Create Issues and link Tasks. Without this flag, show a read-only preview.
    #[arg(long)]
    pub apply: bool,
    /// Include done and cancelled Tasks in the export.
    #[arg(long, alias = "include-terminal")]
    pub include_closed: bool,
}

#[derive(Debug, Args)]
pub struct GitHubAutoPushArgs {
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
    /// Resolve a field conflict as `FIELD=orbit` or `FIELD=github`. Repeat as needed.
    #[arg(long = "resolve", value_name = "FIELD=orbit|github", action = clap::ArgAction::Append)]
    pub resolutions: Vec<String>,
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
                .long_about(help::LIST_LONG_UNCONNECTED)
                .after_long_help(help::LIST_EXAMPLES_UNCONNECTED)
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
