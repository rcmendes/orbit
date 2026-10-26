use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::comment::{self, Comment};
use crate::error::{OrbitError, Result, code};
use crate::git::{self, CommitOutcome};
use crate::lock::{self, BoardLock};
use crate::slug;
use crate::task;

pub const LEGACY_LAYOUT: &str = "LEGACY_LAYOUT";
pub const AMBIGUOUS_BOARD: &str = "AMBIGUOUS_BOARD";
pub const ORPHAN_WORKTREE: &str = "ORPHAN_WORKTREE";
pub const STORAGE_MISMATCH: &str = "STORAGE_MISMATCH";
pub const ABSOLUTE_BOARD_PATH: &str = "ABSOLUTE_BOARD_PATH";

/// One candidate in the Board-resolution order. `doctor` prints the whole
/// trace so a missing Board names every place it looked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    Dir,
    Link,
    Nearest,
    MainCheckout,
    LegacyLocal,
    Global,
    None,
}

impl DiscoverySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dir => "dir",
            Self::Link => "link",
            Self::Nearest => "nearest",
            Self::MainCheckout => "main_checkout",
            Self::LegacyLocal => "legacy_local",
            Self::Global => "global",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryStep {
    pub source: DiscoverySource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub found: bool,
    pub selected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl DiscoveryStep {
    fn new(source: DiscoverySource, path: Option<PathBuf>) -> Self {
        Self {
            source,
            found: path.as_deref().is_some_and(Board::is_board),
            path,
            selected: false,
            note: None,
        }
    }

    fn missing(source: DiscoverySource) -> Self {
        Self {
            source,
            path: None,
            found: false,
            selected: false,
            note: None,
        }
    }
}

/// The full resolution result. Commands only need `root`; `doctor` also uses
/// the trace and the layout findings without turning a missing Board into an
/// error.
#[derive(Debug)]
pub struct Discovery {
    pub root: Option<PathBuf>,
    pub source: DiscoverySource,
    pub steps: Vec<DiscoveryStep>,
    pub warnings: Vec<String>,
    pub legacy: Option<String>,
    pub ambiguous: Option<(PathBuf, PathBuf)>,
    pub error: Option<OrbitError>,
}

impl Default for Discovery {
    fn default() -> Self {
        Self {
            root: None,
            source: DiscoverySource::None,
            steps: Vec::new(),
            warnings: Vec::new(),
            legacy: None,
            ambiguous: None,
            error: None,
        }
    }
}

/// The repository facts shared by discovery, orphan repair, and migration.
#[derive(Debug, Clone)]
pub struct RepoContext {
    pub cwd: PathBuf,
    pub repo_root: PathBuf,
    pub common_dir: PathBuf,
    pub main_checkout: PathBuf,
    pub legacy_local_board: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub board: BoardConfig,
    #[serde(default)]
    pub id: IdConfig,
}

impl Config {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join("config.toml");
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(OrbitError::board_not_found());
            }
            Err(error) => {
                return Err(OrbitError::io(format!("read {}", path.display()), error));
            }
        };

        let config: Config = toml::from_str(&content)
            .map_err(|error| OrbitError::config(format!("invalid {}: {error}", path.display())))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.board.lease_minutes <= 0 {
            return Err(OrbitError::config("board.lease_minutes must be positive"));
        }
        if self.board.default_priority > 3 {
            return Err(OrbitError::config(
                "board.default_priority must be between 0 and 3",
            ));
        }
        for label in &self.board.non_ready_labels {
            if !task::is_valid_label(label) {
                return Err(OrbitError::config(format!(
                    "board.non_ready_labels contains invalid label `{label}`"
                )));
            }
        }
        if !(6..=8).contains(&self.id.length) {
            return Err(OrbitError::config("id.length must be between 6 and 8"));
        }
        self.id.validate()?;
        if self.board.branch.trim().is_empty() {
            return Err(OrbitError::config("board.branch must be non-empty"));
        }
        Ok(())
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self)
            .map_err(|error| OrbitError::config(format!("serialize config: {error}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageMode {
    Worktree,
    Repository,
}

impl StorageMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Worktree => "worktree",
            Self::Repository => "repository",
        }
    }

    /// Infers the mode from a Board root that predates the `storage` field.
    pub fn detect(root: &Path) -> Self {
        if root.join(".git").is_file() {
            Self::Worktree
        } else {
            Self::Repository
        }
    }

    pub fn for_new_dir(root: &Path) -> Self {
        if git::toplevel_containing(root).is_some() {
            Self::Worktree
        } else {
            Self::Repository
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardConfig {
    #[serde(default = "default_branch")]
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageMode>,
    #[serde(default = "default_lease")]
    pub lease_minutes: i64,
    #[serde(default = "task::default_priority")]
    pub default_priority: u8,
    #[serde(default = "default_non_ready_labels")]
    pub non_ready_labels: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_commit: bool,
    #[serde(default = "default_lock_timeout")]
    pub lock_timeout_ms: u64,
}

impl Default for BoardConfig {
    fn default() -> Self {
        Self {
            branch: default_branch(),
            storage: None,
            lease_minutes: default_lease(),
            default_priority: task::default_priority(),
            non_ready_labels: default_non_ready_labels(),
            auto_commit: true,
            lock_timeout_ms: default_lock_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdConfig {
    #[serde(default = "default_id_length")]
    pub length: usize,
    #[serde(default = "default_prefix")]
    pub prefix: String,
}

impl Default for IdConfig {
    fn default() -> Self {
        Self {
            length: default_id_length(),
            prefix: default_prefix(),
        }
    }
}

impl IdConfig {
    pub fn validate(&self) -> Result<()> {
        let prefix = &self.prefix;
        if prefix.is_empty() {
            return Err(OrbitError::config("id.prefix must be non-empty"));
        }
        if prefix.contains('.')
            || !prefix.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            })
        {
            return Err(OrbitError::config(
                "id.prefix must contain only lowercase letters, digits, and dashes (no dots)",
            ));
        }

        if let Some(slug) = prefix.strip_suffix('-') {
            if !slug::is_valid(slug) {
                return Err(OrbitError::config(
                    "a trailing-dash id.prefix requires a 1-20 character slug with alphanumeric ends and no `--`",
                ));
            }
        } else if prefix.len() > slug::MAX_LEN {
            return Err(OrbitError::config(
                "a legacy id.prefix without a trailing dash must be at most 20 characters",
            ));
        }

        Ok(())
    }
}

fn default_branch() -> String {
    "orbit".to_string()
}

fn default_lease() -> i64 {
    30
}

fn default_non_ready_labels() -> Vec<String> {
    vec![
        "needs-triage".to_string(),
        "needs-info".to_string(),
        "ready-for-human".to_string(),
    ]
}

fn default_true() -> bool {
    true
}

fn default_lock_timeout() -> u64 {
    2000
}

fn default_id_length() -> usize {
    task::DEFAULT_ID_LENGTH
}

fn default_prefix() -> String {
    task::DEFAULT_PREFIX.to_string()
}

#[derive(Debug, Clone)]
pub struct InitResult {
    pub board: PathBuf,
    pub branch: String,
    pub external: bool,
    pub prefix: String,
    pub storage: StorageMode,
}

pub struct Board {
    pub root: PathBuf,
    pub lock_path: PathBuf,
    pub config: Config,
    pub warnings: Vec<String>,
}

impl Board {
    pub fn discover(dir_override: Option<&Path>) -> Result<Self> {
        let mut discovery = Self::discovery(dir_override);
        // A forced working-tree cleanup can delete DIR and leave the `orbit`
        // worktree record behind; the branch still holds every Task, so
        // re-attach it and resolve again instead of failing.
        if dir_override.is_none()
            && discovery.root.is_none()
            && discovery
                .error
                .as_ref()
                .is_none_or(|error| error.code == code::BOARD_NOT_FOUND)
            && let Some(context) = Self::repo_context()
            && let Ok(fixed) = crate::doctor::reattach_orphans(&context)
            && !fixed.is_empty()
        {
            discovery = Self::discovery(dir_override);
            for note in fixed {
                push_warning(&mut discovery.warnings, format!("repaired: {note}"));
            }
        }
        if let Some(error) = discovery.error {
            return Err(error);
        }
        let Some(root) = discovery.root else {
            return Err(OrbitError::board_not_found());
        };
        let linked = matches!(discovery.source, DiscoverySource::Link);
        let mut board = Self::at_root(root, discovery.warnings)?;
        if linked && board.config.board.storage.is_none() {
            push_warning(
                &mut board.warnings,
                format!(
                    "{LEGACY_LAYOUT}: linked Board at {} predates board.storage",
                    board.root.display()
                ),
            );
        }
        Ok(board)
    }

    /// Loads a Board whose root is already known. The caller decides how the
    /// path was found; discovery warnings are carried through unchanged.
    pub fn at_root(root: PathBuf, warnings: Vec<String>) -> Result<Self> {
        let config = Config::load(&root)?;
        let lock_path = if config.board.storage.is_none() {
            legacy_local_lock_for(&root).unwrap_or_else(|| root.join(".lock"))
        } else {
            root.join(".lock")
        };
        Ok(Self {
            root,
            lock_path,
            config,
            warnings,
        })
    }

    /// Resolves the Board without failing when none exists. Every candidate is
    /// recorded in order so `doctor` can print the trace.
    pub fn discovery(dir_override: Option<&Path>) -> Discovery {
        let mut discovery = Discovery::default();
        if let Some(dir) = dir_override {
            resolve_explicit(dir, &mut discovery);
            return discovery;
        }
        resolve_from_cwd(&mut discovery);
        discovery
    }

    /// Repository facts for the current directory, if it is inside a working
    /// tree. Used by `doctor` and migration when there is no Board to resolve.
    pub fn repo_context() -> Option<RepoContext> {
        let cwd = std::env::current_dir().ok()?;
        let cwd = dunce::canonicalize(&cwd).unwrap_or(cwd);
        Self::repo_context_at(&cwd).ok().flatten()
    }

    pub fn repo_context_at(start: &Path) -> Result<Option<RepoContext>> {
        let Some(repo_root) = git::toplevel_containing(start) else {
            return Ok(None);
        };
        let repo_root = dunce::canonicalize(&repo_root).unwrap_or(repo_root);
        let common_dir = git::common_dir_at(&repo_root)?;
        let main_checkout = main_checkout_for(&repo_root, &common_dir);
        Ok(Some(RepoContext {
            cwd: start.to_path_buf(),
            legacy_local_board: legacy_local_path(&common_dir),
            common_dir,
            main_checkout,
            repo_root,
        }))
    }

    pub fn local_root() -> Result<PathBuf> {
        Ok(slug::main_checkout_anchor()?.join(".orbit"))
    }

    pub fn storage_mode(&self) -> StorageMode {
        self.config
            .board
            .storage
            .unwrap_or_else(|| StorageMode::detect(&self.root))
    }

    pub fn exclude_entry(root: &Path) -> Result<Option<String>> {
        let Some((_, entry)) = worktree_exclusion(root)? else {
            return Ok(None);
        };
        Ok(Some(entry))
    }

    pub fn ensure_excluded(root: &Path) -> Result<Option<String>> {
        let Some((repo, entry)) = worktree_exclusion(root)? else {
            return Ok(None);
        };
        git::ensure_exclude_entry(&repo, root)?;
        Ok(Some(entry))
    }

    pub fn resolve_target(path: &Path) -> Result<(PathBuf, bool)> {
        let root = resolve_external(path)?;
        if Self::is_board(&root) {
            return Ok((root, false));
        }
        let nested = root.join("board");
        if Self::is_board(&nested) {
            return Ok((nested, true));
        }
        Err(link_not_found(&root))
    }

    pub fn link_path() -> Result<PathBuf> {
        Ok(git::common_dir()?.join("orbit").join("link"))
    }

    pub fn legacy_local_root() -> Result<PathBuf> {
        Ok(git::common_dir()?.join("orbit").join("board"))
    }

    pub fn read_link() -> Result<Option<PathBuf>> {
        let path = Self::link_path()?;
        read_link_file(&path)
    }

    pub fn write_link(root: &Path) -> Result<()> {
        let path = Self::link_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
        }
        crate::atomic::write_atomic(&path, format!("{}\n", root.display()).as_bytes(), true)
    }

    pub fn clear_link() -> Result<bool> {
        let path = Self::link_path()?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(OrbitError::io(format!("remove {}", path.display()), error)),
        }
    }

    pub fn is_board(root: &Path) -> bool {
        root.join("config.toml").exists()
    }

    pub fn task_ids(&self) -> Result<Vec<String>> {
        let directory = self.tasks_dir();
        let entries = std::fs::read_dir(&directory).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                OrbitError::board_corrupt(
                    "the board is missing its tasks directory",
                    serde_json::json!({ "path": directory }),
                )
            } else {
                OrbitError::io(format!("read {}", directory.display()), error)
            }
        })?;

        let mut ids = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if stem.starts_with('.') {
                continue;
            }
            ids.push(stem.to_string());
        }
        ids.sort();
        Ok(ids)
    }

    pub fn task_path(&self, id: &str) -> PathBuf {
        self.tasks_dir().join(format!("{id}.md"))
    }

    pub fn tasks_dir(&self) -> PathBuf {
        self.root.join("tasks")
    }

    pub fn comments_dir(&self) -> PathBuf {
        self.root.join("comments")
    }

    pub fn comment_path(&self, id: &str) -> PathBuf {
        self.comments_dir().join(format!("{id}.md"))
    }

    pub fn comment_ids(&self) -> Result<Vec<String>> {
        let directory = self.comments_dir();
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(OrbitError::io(
                    format!("read {}", directory.display()),
                    error,
                ));
            }
        };

        let mut ids = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if stem.starts_with('.') {
                continue;
            }
            ids.push(stem.to_string());
        }
        ids.sort();
        Ok(ids)
    }

    pub fn read_comments(&self, id: &str) -> Result<Vec<Comment>> {
        let path = self.comment_path(id);
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(OrbitError::io(format!("read {}", path.display()), error));
            }
        };

        comment::parse_comments(&content).map_err(|error| {
            OrbitError::new(
                code::CORRUPT_COMMENT,
                format!("{}: {}", path.display(), error.message),
            )
        })
    }

    pub fn write_comments(&self, id: &str, comments: &[Comment]) -> Result<()> {
        let path = self.comment_path(id);
        let content = comment::render_comments_file(id, comments);
        crate::atomic::write_atomic(&path, content.as_bytes(), true)
    }

    pub fn lock_exclusive(&self) -> Result<BoardLock> {
        lock::BoardLock::acquire(&self.lock_path, self.config.board.lock_timeout_ms, true)
    }

    pub fn lock_shared(&self) -> Result<BoardLock> {
        lock::BoardLock::acquire(&self.lock_path, self.config.board.lock_timeout_ms, false)
    }

    pub fn commit(&self, message: &str, force: bool) -> Result<CommitOutcome> {
        if !force && !self.config.board.auto_commit {
            return Ok(CommitOutcome::Skipped);
        }
        match git::commit(&self.root, message) {
            Ok(outcome) => Ok(outcome),
            Err(error) if !force => Ok(CommitOutcome::Failed(error.message)),
            Err(error) => Err(error),
        }
    }

    pub fn init_dir(dir: &Path, slug: Option<&str>) -> Result<InitResult> {
        let root = resolve_external(dir)?;
        if let Some(existing) = Self::conflicting_board_for(&root, false)? {
            return Err(already_initialized(&existing));
        }
        if git::toplevel_containing(&root).is_some() {
            init_worktree_at(&root, slug)
        } else {
            init_external(&root, slug)
        }
    }

    pub fn init_repository(dir: &Path, slug: Option<&str>) -> Result<InitResult> {
        let root = resolve_external(dir)?;
        init_external(&root, slug)
    }

    /// Returns the repository-local Board that would compete with `dir`.
    /// A global Board is exempt: it is only an opt-in discovery fallback.
    pub fn conflicting_board_for(dir: &Path, global: bool) -> Result<Option<PathBuf>> {
        if global || Self::is_board(dir) {
            return Ok(None);
        }
        let Some(repo_root) = git::toplevel_containing(dir) else {
            return Ok(None);
        };
        let repo_root = dunce::canonicalize(&repo_root).unwrap_or(repo_root);
        let common = git::common_dir_at(&repo_root)?;

        if let Some(target) = read_link_file(&common.join("orbit").join("link"))? {
            let (linked, _) = classify_link_target(&target)?;
            if linked != dir {
                return Ok(Some(linked));
            }
        }

        let main = main_checkout_for(&repo_root, &common).join(".orbit");
        if main != dir && Self::is_board(&main) && !is_global_board(&main) {
            return Ok(Some(main));
        }

        let legacy = legacy_local_path(&common);
        if legacy != dir && Self::is_board(&legacy) {
            return Ok(Some(legacy));
        }

        let mut current = dir.parent();
        while let Some(parent) = current {
            let candidate = parent.join(".orbit");
            if candidate != dir && Self::is_board(&candidate) && !is_global_board(&candidate) {
                return Ok(Some(candidate));
            }
            if parent == repo_root {
                break;
            }
            current = parent.parent();
        }

        for path in git::worktree_paths(&repo_root)? {
            let path = dunce::canonicalize(&path).unwrap_or(path);
            if path != dir && Self::is_board(&path) && !is_global_board(&path) {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    pub fn set_storage_mode(&self, storage: StorageMode) -> Result<CommitOutcome> {
        let _lock = self.lock_exclusive()?;
        let mut config = Config::load(&self.root)?;
        if config.board.storage == Some(storage) {
            return Ok(CommitOutcome::NoChanges);
        }
        config.board.storage = Some(storage);
        config.validate()?;
        let toml = config.to_toml()?;
        crate::atomic::write_atomic(&self.root.join("config.toml"), toml.as_bytes(), true)?;
        self.commit("orbit: migrate board storage", true)
    }

    /// Changes the Task-ID prefix on an empty Board. The caller must have
    /// decided that the change is wanted; this method re-checks and locks.
    pub fn set_prefix(&self, prefix: &str) -> Result<CommitOutcome> {
        let _lock = self.lock_exclusive()?;
        let mut config = Config::load(&self.root)?;
        if slug::slug_of_prefix(&config.id.prefix) == slug::slug_of_prefix(prefix) {
            return Ok(CommitOutcome::NoChanges);
        }

        let tasks = self.task_ids()?.len();
        if tasks > 0 {
            return Err(slug_locked_error(&config.id.prefix, prefix, tasks));
        }

        config.id.prefix = prefix.to_string();
        config.validate()?;
        let toml = config.to_toml()?;
        crate::atomic::write_atomic(&self.root.join("config.toml"), toml.as_bytes(), true)?;
        self.commit(&format!("orbit: set task prefix to {prefix}"), false)
    }
}

pub(crate) fn slug_locked_error(current: &str, requested: &str, tasks: usize) -> OrbitError {
    let requested_slug = slug::slug_of_prefix(requested);
    OrbitError::new(
        code::SLUG_LOCKED,
        format!(
            "board prefix is `{current}`; cannot change it to `{requested}` because {tasks} task(s) exist; run `orbit slug migrate {requested_slug}` to rewrite all IDs, or keep the current prefix"
        ),
    )
    .with_details(serde_json::json!({
        "current": current,
        "requested": requested,
        "tasks": tasks,
    }))
}

fn resolve_prefix(slug: Option<&str>) -> Result<String> {
    match slug {
        Some(slug) => slug::explicit_prefix(slug),
        None => Ok(task::DEFAULT_PREFIX.to_string()),
    }
}

fn init_worktree_at(board_root: &Path, slug: Option<&str>) -> Result<InitResult> {
    let repo = git::toplevel_containing(board_root).ok_or_else(|| {
        OrbitError::new(
            code::NOT_A_REPOSITORY,
            format!(
                "{} is not inside a Git working tree; use a standalone Board directory",
                board_root.display()
            ),
        )
    })?;
    let repo = dunce::canonicalize(&repo).unwrap_or(repo);
    let prefix = resolve_prefix(slug)?;

    if board_root.join("config.toml").exists() {
        return Err(already_initialized(board_root));
    }

    let common = git::common_dir_at(&repo)?;
    let bootstrap = common.join("orbit");
    std::fs::create_dir_all(&bootstrap)
        .map_err(|error| OrbitError::io(format!("create {}", bootstrap.display()), error))?;
    let _init_lock = BoardLock::acquire(&bootstrap.join("init.lock"), 2000, true)?;

    git::ensure_identity(&repo)?;

    let branch = "orbit";
    let branch_existed = git::branch_exists(&repo, branch)?;
    if branch_existed && !git::branch_contains_file(&repo, branch, "config.toml")? {
        return Err(OrbitError::config(
            "branch 'orbit' already exists and is not an Orbit board; rename it before running `orbit init`",
        ));
    }

    let created_branch = !branch_existed;
    if created_branch {
        let tree = git::empty_tree(&repo)?;
        let commit = git::commit_tree(&repo, &tree, "orbit: create board branch")?;
        git::create_branch(&repo, branch, &commit)?;
    }

    git::worktree_prune(&repo)?;
    if let Err(error) = git::worktree_add(&repo, board_root, branch) {
        if created_branch {
            let _ = git::delete_branch(&repo, branch);
        }
        return Err(error);
    }

    if let Err(error) = git::ensure_exclude_entry(&repo, board_root) {
        let _ = git::worktree_remove(&repo, board_root);
        if created_branch {
            let _ = git::delete_branch(&repo, branch);
        }
        return Err(error);
    }

    if board_root.join("config.toml").exists() {
        let config = Config::load(board_root)?;
        return Ok(InitResult {
            board: board_root.to_path_buf(),
            branch: branch.to_string(),
            external: false,
            prefix: config.id.prefix,
            storage: config.board.storage.unwrap_or(StorageMode::Worktree),
        });
    }

    let result = initialize_board_files(board_root, branch, false, &prefix, StorageMode::Worktree);
    if result.is_err() {
        let _ = git::worktree_remove(&repo, board_root);
        if created_branch {
            let _ = git::delete_branch(&repo, branch);
        }
    }
    result
}

fn init_external(dir: &Path, slug: Option<&str>) -> Result<InitResult> {
    let root = resolve_external(dir)?;
    let prefix = resolve_prefix(slug)?;

    if root.join("config.toml").exists() {
        return Err(already_initialized(&root));
    }

    std::fs::create_dir_all(&root)
        .map_err(|error| OrbitError::io(format!("create {}", root.display()), error))?;

    let _init_lock = BoardLock::acquire(&root.join(".init.lock"), 2000, true)?;

    if !root.join(".git").exists() {
        git::init_repo(&root)?;
    }
    if git::has_head(&root)? {
        return Err(OrbitError::config(format!(
            "{} is a Git repository with commits; use an empty directory for an external board",
            root.display()
        )));
    }

    for entry in std::fs::read_dir(&root)
        .map_err(|error| OrbitError::io(format!("read {}", root.display()), error))?
        .flatten()
    {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".git" || name == ".init.lock" {
            continue;
        }
        return Err(OrbitError::config(format!(
            "external board directory {} must be empty",
            root.display()
        )));
    }

    git::ensure_identity(&root)?;
    git::set_head(&root, "orbit")?;
    initialize_board_files(&root, "orbit", true, &prefix, StorageMode::Repository)
}

fn initialize_board_files(
    root: &Path,
    branch: &str,
    external: bool,
    prefix: &str,
    storage: StorageMode,
) -> Result<InitResult> {
    let mut config = Config::default();
    config.id.prefix = prefix.to_string();
    config.board.storage = Some(storage);
    config.validate()?;
    let config_toml = config.to_toml()?;

    write_file(&root.join("config.toml"), config_toml.as_bytes())?;
    write_file(
        &root.join(".gitignore"),
        b".lock\n.init.lock\n.tmp-orbit-*\ntasks/.tmp-orbit-*\ncomments/.tmp-orbit-*\n",
    )?;
    std::fs::create_dir_all(root.join("tasks")).map_err(|error| {
        OrbitError::io(format!("create {}", root.join("tasks").display()), error)
    })?;
    write_file(&root.join("tasks").join(".gitkeep"), b"")?;
    std::fs::create_dir_all(root.join("comments")).map_err(|error| {
        OrbitError::io(format!("create {}", root.join("comments").display()), error)
    })?;
    write_file(&root.join("comments").join(".gitkeep"), b"")?;

    git::commit(root, "orbit: initialize board")?;

    Ok(InitResult {
        board: root.to_path_buf(),
        branch: branch.to_string(),
        external,
        prefix: prefix.to_string(),
        storage,
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)
        .map_err(|error| OrbitError::io(format!("write {}", path.display()), error))
}

fn resolve_explicit(dir: &Path, discovery: &mut Discovery) {
    let root = match resolve_external(dir) {
        Ok(root) => root,
        Err(error) => {
            discovery
                .steps
                .push(DiscoveryStep::missing(DiscoverySource::Dir));
            discovery.error = Some(error);
            return;
        }
    };
    let mut step = DiscoveryStep::new(DiscoverySource::Dir, Some(root.clone()));
    if !step.found {
        discovery.steps.push(step);
        discovery.error = Some(OrbitError::board_not_found());
        return;
    }

    step.selected = true;
    discovery.root = Some(root.clone());
    discovery.source = DiscoverySource::Dir;
    match legacy_local_board_for(&root) {
        Some(legacy) if legacy == root => {
            let warning = format!("{LEGACY_LAYOUT}: using legacy Board at {}", root.display());
            discovery.legacy = Some(warning.clone());
            push_warning(&mut discovery.warnings, warning);
        }
        Some(legacy) => {
            discovery.ambiguous = Some((root.clone(), legacy.clone()));
            warn_if_ambiguous(&mut discovery.warnings, &root, &legacy);
        }
        None => {}
    }
    discovery.steps.push(step);
}

fn resolve_from_cwd(discovery: &mut Discovery) {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => dunce::canonicalize(&cwd).unwrap_or(cwd),
        Err(error) => {
            discovery.error = Some(OrbitError::io("resolve current directory", error));
            return;
        }
    };
    let home = home_dir();

    if let Some(repo_root) = git::toplevel_containing(&cwd) {
        let repo_root = dunce::canonicalize(&repo_root).unwrap_or(repo_root);
        let common = match git::common_dir_at(&repo_root) {
            Ok(common) => common,
            Err(error) => {
                discovery.error = Some(error);
                return;
            }
        };
        let link = common.join("orbit").join("link");
        let legacy_local = legacy_local_path(&common);
        let nearest = nearest_orbit(&cwd, &repo_root, home.as_deref());
        let main = main_checkout_for(&repo_root, &common).join(".orbit");
        let global = home.as_deref().and_then(global_board);

        let mut link_root = None;
        let mut link_nested = false;
        match read_link_file(&link) {
            Ok(Some(target)) => match classify_link_target(&target) {
                Ok((root, nested)) => {
                    link_root = Some(root);
                    link_nested = nested;
                }
                Err(error) => discovery.error = Some(error),
            },
            Ok(None) => {}
            Err(error) => discovery.error = Some(error),
        }

        discovery.steps.push(DiscoveryStep::new(
            DiscoverySource::Link,
            Some(link.clone()),
        ));
        let link_step = discovery.steps.len() - 1;
        discovery.steps[link_step].found = link_root.is_some();
        discovery.steps.push(DiscoveryStep::new(
            DiscoverySource::Nearest,
            nearest.clone(),
        ));
        discovery.steps.push(DiscoveryStep::new(
            DiscoverySource::MainCheckout,
            Some(main.clone()),
        ));
        discovery.steps.push(DiscoveryStep::new(
            DiscoverySource::LegacyLocal,
            Some(legacy_local.clone()),
        ));
        discovery
            .steps
            .push(DiscoveryStep::missing(DiscoverySource::Global));
        if let Some(global) = &global {
            let index = discovery.steps.len() - 1;
            discovery.steps[index] =
                DiscoveryStep::new(DiscoverySource::Global, Some(global.clone()));
        }

        let selected = if discovery.error.is_some() {
            None
        } else if let Some(root) = link_root {
            if link_nested {
                let warning = format!(
                    "{LEGACY_LAYOUT}: linked Board is nested at {}",
                    root.display()
                );
                discovery.legacy = Some(warning.clone());
                push_warning(&mut discovery.warnings, warning);
            }
            Some((DiscoverySource::Link, root))
        } else if let Some(root) = nearest {
            warn_if_ambiguous(&mut discovery.warnings, &root, &legacy_local);
            Some((DiscoverySource::Nearest, root))
        } else if Board::is_board(&main) {
            warn_if_ambiguous(&mut discovery.warnings, &main, &legacy_local);
            Some((DiscoverySource::MainCheckout, main.clone()))
        } else if Board::is_board(&legacy_local) {
            let warning = format!(
                "{LEGACY_LAYOUT}: using legacy Board at {}",
                legacy_local.display()
            );
            discovery.legacy = Some(warning.clone());
            push_warning(&mut discovery.warnings, warning);
            Some((DiscoverySource::LegacyLocal, legacy_local.clone()))
        } else {
            global.map(|global| (DiscoverySource::Global, global))
        };

        if Board::is_board(&legacy_local) {
            let new_local = [&main].into_iter().find(|path| Board::is_board(path));
            if let Some(new_local) = new_local
                && new_local != &legacy_local
            {
                discovery.ambiguous = Some((new_local.clone(), legacy_local.clone()));
            }
        }

        match selected {
            Some((source, root)) => {
                discovery.source = source;
                discovery.root = Some(root);
                for step in &mut discovery.steps {
                    if step.source == source {
                        step.selected = true;
                        break;
                    }
                }
            }
            None if discovery.error.is_none() => {
                discovery.error = Some(OrbitError::board_not_found());
            }
            None => {}
        }
        return;
    }

    let stop = home
        .as_deref()
        .filter(|home| cwd.starts_with(home))
        .unwrap_or_else(|| Path::new("/"));
    let nearest = nearest_orbit(&cwd, stop, home.as_deref());
    let global = home.as_deref().and_then(global_board);
    discovery.steps.push(DiscoveryStep::new(
        DiscoverySource::Nearest,
        nearest.clone(),
    ));
    discovery
        .steps
        .push(DiscoveryStep::new(DiscoverySource::Global, global.clone()));

    if let Some(root) = nearest {
        discovery.source = DiscoverySource::Nearest;
        discovery.root = Some(root);
        discovery.steps[0].selected = true;
    } else if let Some(root) = global {
        discovery.source = DiscoverySource::Global;
        discovery.root = Some(root);
        discovery.steps[1].selected = true;
    } else {
        discovery.error = Some(OrbitError::board_not_found());
    }
}

pub(crate) fn resolve_external(dir: &Path) -> Result<PathBuf> {
    let root = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| OrbitError::io("resolve current directory", error))?
            .join(dir)
    };
    if let Ok(canonical) = dunce::canonicalize(&root) {
        return Ok(canonical);
    }

    let mut missing = Vec::new();
    let mut existing = root.as_path();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            return Ok(root);
        };
        missing.push(name.to_os_string());
        let Some(parent) = existing.parent() else {
            return Ok(root);
        };
        existing = parent;
    }
    let Ok(mut canonical) = dunce::canonicalize(existing) else {
        return Ok(root);
    };
    for component in missing.iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .and_then(|home| dunce::canonicalize(home).ok())
}

fn global_board(home: &Path) -> Option<PathBuf> {
    let root = home.join(".orbit");
    Board::is_board(&root).then_some(root)
}

fn is_global_board(path: &Path) -> bool {
    home_dir().is_some_and(|home| path == home.join(".orbit"))
}

fn nearest_orbit(start: &Path, stop: &Path, skip: Option<&Path>) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if skip != Some(dir) {
            let candidate = dir.join(".orbit");
            if Board::is_board(&candidate) {
                return Some(candidate);
            }
        }
        if dir == stop {
            break;
        }
        current = dir.parent();
    }
    None
}

fn main_checkout_for(repo_root: &Path, common: &Path) -> PathBuf {
    if common.file_name().and_then(|name| name.to_str()) == Some(".git")
        && let Some(parent) = common.parent()
    {
        return parent.to_path_buf();
    }
    repo_root.to_path_buf()
}

fn worktree_exclusion(root: &Path) -> Result<Option<(PathBuf, String)>> {
    let mut anchor = root;
    if let Some(repo_root) = git::toplevel_containing(root) {
        let repo_root = dunce::canonicalize(&repo_root).unwrap_or(repo_root);
        if repo_root != root {
            let entry = git::exclude_entry_for(&repo_root, root)?;
            return Ok(Some((repo_root, entry)));
        }
        if let Some(parent) = root.parent() {
            anchor = parent;
        }
    }
    let Some(repo_root) = git::toplevel_containing(anchor) else {
        return Ok(None);
    };
    let repo_root = dunce::canonicalize(&repo_root).unwrap_or(repo_root);
    let entry = git::exclude_entry_for(&repo_root, root)?;
    Ok(Some((repo_root, entry)))
}

fn legacy_local_path(common: &Path) -> PathBuf {
    common.join("orbit").join("board")
}

fn legacy_local_board_for(root: &Path) -> Option<PathBuf> {
    if let Some(repo) = git::toplevel_containing(root)
        && let Ok(common) = git::common_dir_at(&repo)
    {
        let legacy = legacy_local_path(&common);
        if Board::is_board(&legacy) {
            return Some(legacy);
        }
    }

    if root.file_name().and_then(|name| name.to_str()) == Some("board") {
        let orbit = root.parent()?;
        let common = orbit.parent()?;
        if orbit.file_name().and_then(|name| name.to_str()) == Some("orbit")
            && common.join("HEAD").is_file()
            && Board::is_board(root)
        {
            return Some(root.to_path_buf());
        }
    }
    None
}

fn legacy_local_lock_for(root: &Path) -> Option<PathBuf> {
    if let Some(repo) = git::toplevel_containing(root)
        && let Ok(common) = git::common_dir_at(&repo)
    {
        let legacy = legacy_local_path(&common);
        if root == legacy {
            return Some(common.join("orbit").join("lock"));
        }
    }

    if root.file_name().and_then(|name| name.to_str()) == Some("board") {
        let orbit = root.parent()?;
        let common = orbit.parent()?;
        if orbit.file_name().and_then(|name| name.to_str()) == Some("orbit")
            && common.join("HEAD").is_file()
        {
            return Some(orbit.join("lock"));
        }
    }
    None
}

fn classify_link_target(path: &Path) -> Result<(PathBuf, bool)> {
    Board::resolve_target(path).map_err(|_| {
        OrbitError::new(
            code::LINK_NOT_FOUND,
            format!(
                "the linked Board at {} is missing; run `orbit link <board>` to re-link or restore it",
                path.display()
            ),
        )
    })
}

fn link_not_found(path: &Path) -> OrbitError {
    OrbitError::new(
        code::LINK_NOT_FOUND,
        format!(
            "no Orbit Board found at {}; run `orbit init {}` to create one or pass an existing Orbit folder",
            path.display(),
            path.display()
        ),
    )
}

fn warn_if_ambiguous(warnings: &mut Vec<String>, root: &Path, legacy: &Path) {
    if root != legacy
        && root.file_name().and_then(|name| name.to_str()) == Some(".orbit")
        && Board::is_board(legacy)
    {
        push_warning(
            warnings,
            format!(
                "{AMBIGUOUS_BOARD}: both {} and legacy Board {} exist; using {}",
                root.display(),
                legacy.display(),
                root.display()
            ),
        );
    }
}

fn push_warning(warnings: &mut Vec<String>, warning: String) {
    if !warnings.iter().any(|existing| existing == &warning) {
        warnings.push(warning);
    }
}

fn read_link_file(path: &Path) -> Result<Option<PathBuf>> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(OrbitError::io(format!("read {}", path.display()), error)),
    };
    let value = content.trim();
    if value.is_empty() {
        return Err(OrbitError::config(format!(
            "{} is empty; run `orbit unlink` and link the repository again",
            path.display()
        )));
    }
    let candidate = PathBuf::from(value);
    if !candidate.is_absolute() {
        return Err(OrbitError::config(format!(
            "{} must contain an absolute path; run `orbit link` again",
            path.display()
        )));
    }
    Ok(Some(candidate))
}

pub(crate) fn already_initialized(path: &Path) -> OrbitError {
    OrbitError::new(
        code::ALREADY_INITIALIZED,
        format!("an Orbit board already exists at {}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips() {
        let config = Config::default();
        let toml = config.to_toml().unwrap();
        let parsed: Config = toml::from_str(&toml).unwrap();

        assert_eq!(parsed.board.branch, "orbit");
        assert_eq!(parsed.board.lease_minutes, 30);
        assert_eq!(
            parsed.board.non_ready_labels,
            vec!["needs-triage", "needs-info", "ready-for-human"]
        );
        assert_eq!(parsed.id.length, 6);
        assert_eq!(parsed.id.prefix, "orb-");
    }

    #[test]
    fn config_rejects_invalid_values() {
        let mut config = Config::default();
        config.id.length = 3;
        assert_eq!(config.validate().unwrap_err().code, code::CONFIG_ERROR);

        let mut config = Config::default();
        config.board.default_priority = 9;
        assert_eq!(config.validate().unwrap_err().code, code::CONFIG_ERROR);

        let mut config = Config::default();
        config.board.non_ready_labels = vec!["Has Space".to_string()];
        assert_eq!(config.validate().unwrap_err().code, code::CONFIG_ERROR);

        let mut config = Config::default();
        config.id.prefix = "orb.v2-".to_string();
        assert_eq!(config.validate().unwrap_err().code, code::CONFIG_ERROR);

        let mut config = Config::default();
        config.id.prefix = "Orb-".to_string();
        assert_eq!(config.validate().unwrap_err().code, code::CONFIG_ERROR);
    }
}
