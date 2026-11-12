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
    Workspace,
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
            Self::Workspace => "workspace",
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
    /// The `.orbit` folder the resolved Board hangs off, when known.
    pub orbit_dir: Option<PathBuf>,
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
            orbit_dir: None,
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
    /// Legacy GitHub provider section; `orbit migrate` rewrites it to `remote`.
    #[serde(default)]
    pub github: crate::github::GitHubConfig,
    #[serde(default, skip_serializing_if = "crate::remote::RemoteConfig::is_empty")]
    pub remote: crate::remote::RemoteConfig,
    #[serde(
        default,
        skip_serializing_if = "crate::workspace::WorkspaceConfig::is_empty"
    )]
    pub workspace: crate::workspace::WorkspaceConfig,
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
        #[cfg(not(feature = "github-provider"))]
        if config.github.is_enabled() {
            return Err(OrbitError::new(
                code::GITHUB_UNSUPPORTED,
                format!(
                    "this Orbit binary does not include GitHub sync support; upgrade to Orbit {} or newer, or reinstall/build with the default `github-provider` feature",
                    crate::github::config::MIN_GITHUB_VERSION
                ),
            ));
        }
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
        for area in &self.board.areas {
            if !crate::task::is_valid_area(area) {
                return Err(OrbitError::config(format!(
                    "board.areas contains invalid area `{area}`"
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
        if let Some(host) = &self.board.host
            && host.trim().is_empty()
        {
            return Err(OrbitError::config("board.host must be non-empty"));
        }
        self.github.validate()?;
        self.workspace.validate()?;
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
    /// Retired by ADR 0019: every Board folder is a standalone repository.
    /// Still parsed so `orbit migrate` can read pre-migration Boards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageMode>,
    /// Optional per-Board Area vocabulary. When empty, any short lowercase
    /// Area name is accepted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<String>,
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
    /// The Board host: a member Repository name/path or another git URL whose
    /// remote stores this Board's `orbit` branch (ADR 0019). `None` keeps the
    /// history local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// When the host is synchronized: `manual` only on `orbit board sync`,
    /// `auto` also at `orbit prime` and after commits.
    #[serde(default, skip_serializing_if = "HostSyncMode::is_manual")]
    pub host_sync: HostSyncMode,
}

/// Opt-in frequency for pushing the Board branch to its host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostSyncMode {
    /// Only `orbit board sync` talks to the host.
    #[default]
    Manual,
    /// `orbit prime` and every committed mutation also sync the host.
    Auto,
}

impl HostSyncMode {
    fn is_manual(&self) -> bool {
        *self == Self::Manual
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
        }
    }
}

impl Default for BoardConfig {
    fn default() -> Self {
        Self {
            branch: default_branch(),
            storage: None,
            areas: Vec::new(),
            lease_minutes: default_lease(),
            default_priority: task::default_priority(),
            non_ready_labels: default_non_ready_labels(),
            auto_commit: true,
            lock_timeout_ms: default_lock_timeout(),
            host: None,
            host_sync: HostSyncMode::Manual,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdConfig {
    #[serde(default = "default_id_length")]
    pub length: usize,
    #[serde(default = "default_prefix")]
    pub prefix: String,
    /// Additional legacy prefixes accepted for imported Tasks. New IDs always
    /// use `prefix`; this list lets a combined Board retain source IDs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_prefixes: Vec<String>,
}

impl Default for IdConfig {
    fn default() -> Self {
        Self {
            length: default_id_length(),
            prefix: default_prefix(),
            accepted_prefixes: Vec::new(),
        }
    }
}

impl IdConfig {
    pub fn validate(&self) -> Result<()> {
        validate_id_prefix(&self.prefix)?;
        let mut seen = std::collections::BTreeSet::from([self.prefix.as_str()]);
        for prefix in &self.accepted_prefixes {
            validate_id_prefix(prefix)?;
            if !seen.insert(prefix) {
                return Err(OrbitError::config(format!(
                    "id.accepted_prefixes contains duplicate prefix `{prefix}`"
                )));
            }
        }
        Ok(())
    }

    pub fn is_valid_task_id(&self, id: &str) -> bool {
        self.prefix_for_id(id).is_some()
    }

    pub fn prefix_for_id<'a>(&'a self, id: &str) -> Option<&'a str> {
        std::iter::once(self.prefix.as_str())
            .chain(self.accepted_prefixes.iter().map(String::as_str))
            .find(|prefix| task::is_valid_id(id, prefix))
    }

    pub fn parent_id(&self, id: &str) -> Option<String> {
        self.prefix_for_id(id)
            .and_then(|prefix| task::parent_id_from_id(id, prefix))
    }
}

fn validate_id_prefix(prefix: &str) -> Result<()> {
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
    /// The `.orbit` folder anchoring this Board when it resolved through one:
    /// the parent of a hidden `.orbit/board`, or the folder holding the
    /// pointer file for a visible Board folder. `None` for legacy, explicit,
    /// or otherwise unanchored Boards.
    pub orbit_dir: Option<PathBuf>,
    /// Set when a non-forced commit failed during this process.
    pub(crate) commit_failed: std::cell::Cell<bool>,
}

/// Opening policy includes discovery side effects and lock lifetime.
#[derive(Clone, Copy, Debug)]
pub enum AccessMode {
    /// Ordinary reads retain automatic orphan-worktree recovery.
    Read,
    Write,
    /// Diagnostics resolve storage without repairing it.
    Inspect,
}

/// A Board and its command-scoped lock. Keep this value alive while using it.
pub struct BoardAccess {
    board: Board,
    _lock: crate::lock::BoardLock,
}

impl BoardAccess {
    pub fn open(dir: Option<&Path>, mode: AccessMode) -> Result<Self> {
        let (board, discovery) = if matches!(mode, AccessMode::Inspect) {
            let mut discovery = Board::discovery(dir);
            if let Some(error) = discovery.error {
                return Err(error);
            }
            let orbit_dir = discovery.orbit_dir.clone();
            let root = discovery
                .root
                .clone()
                .ok_or_else(OrbitError::board_not_found)?;
            let warnings = std::mem::take(&mut discovery.warnings);
            let mut board = Board::at_root(root, warnings)?;
            board.orbit_dir = orbit_dir.or_else(|| discovery_orbit_dir(&board.root));
            (board, Some(discovery))
        } else {
            let board = Board::discover(dir)?;
            // `discover` re-resolves after orphan repair; refresh the trace so
            // the migration check sees the post-repair layout.
            let discovery = Board::discovery(dir);
            (board, Some(discovery))
        };
        if board.needs_migration(discovery.as_ref()) {
            return Err(OrbitError::new(
                code::BOARD_NOT_MIGRATED,
                format!(
                    "the Board at {} uses a pre-ADR-0019 layout; run `orbit migrate` to convert it to a standalone Board folder",
                    board.root.display()
                ),
            ));
        }
        let lock = if matches!(mode, AccessMode::Write) {
            board.lock_exclusive()?
        } else {
            board.lock_shared()?
        };
        Ok(Self { board, _lock: lock })
    }

    pub fn board(&self) -> &Board {
        &self.board
    }
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
        let orbit_dir = discovery.orbit_dir.clone();
        let mut board = Self::at_root(root, discovery.warnings)?;
        board.orbit_dir = orbit_dir.or_else(|| discovery_orbit_dir(&board.root));
        if linked && board.needs_migration(None) {
            push_warning(
                &mut board.warnings,
                format!(
                    "{LEGACY_LAYOUT}: linked Board at {} uses a pre-ADR-0019 layout; run `orbit migrate`",
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
            orbit_dir: None,
            commit_failed: Default::default(),
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

    /// The default local Board folder: `<main-checkout>/.orbit/board`.
    pub fn local_root() -> Result<PathBuf> {
        Ok(slug::main_checkout_anchor()?.join(".orbit").join("board"))
    }

    /// The `.orbit` folder anchoring this Board when it resolved through one:
    /// the parent of a hidden `.orbit/board`, or the folder holding the
    /// pointer file for a visible Board folder. `None` for legacy, explicit,
    /// or otherwise unanchored Boards.
    pub fn resolved_orbit_dir(&self) -> Option<PathBuf> {
        self.orbit_dir
            .clone()
            .or_else(|| discovery_orbit_dir(&self.root))
    }

    /// A Workspace Board lives internally at `<workspace>/.orbit/board`; the
    /// Workspace root is the user-facing directory. A repository-local Board
    /// at `<repo>/.orbit/board` has the same shape, so the Orbit folder's
    /// parent must sit outside every Git working tree to count as a Workspace.
    pub fn is_workspace_board(&self) -> bool {
        let Some(orbit) = self.resolved_orbit_dir() else {
            return false;
        };
        let Some(workspace) = orbit.parent() else {
            return false;
        };
        git::toplevel_containing(workspace).is_none()
    }

    /// Whether the Board still uses a pre-ADR-0019 layout or configuration
    /// that `orbit migrate` must convert before Board commands may run.
    pub fn needs_migration(&self, discovery: Option<&Discovery>) -> bool {
        // `board.storage` only existed to pick a worktree or in-repo layout.
        if self.config.board.storage.is_some() {
            return true;
        }
        // A Board backed by a Git worktree of another repository.
        if self.root.join(".git").is_file() {
            return true;
        }
        // Every Board is a standalone repository. A Board folder without its
        // own `.git` — a plain directory, or the legacy `.git/orbit/board`
        // store reached through an explicit `--dir` — predates ADR 0019.
        if !self.root.join(".git").exists() {
            return true;
        }
        // A Board folder sitting directly in an Orbit folder, or in the
        // retired `.board` folder: both predate `.orbit/board`.
        if self.root.file_name().and_then(|name| name.to_str()) == Some(ORBIT_DIR)
            || self
                .root
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                == Some(LEGACY_BOARD_DIR)
        {
            return true;
        }
        // A legacy `<workspace>/board` folder: named `board`, outside any Git
        // working tree, and not anchored by an Orbit folder. Anchored Boards
        // (`.orbit/board` hidden or a `board` pointer) are already covered by
        // the checks above.
        let anchored = self
            .orbit_dir
            .as_ref()
            .is_some_and(|orbit| *orbit != self.root)
            || discovery
                .and_then(|discovery| discovery.orbit_dir.as_ref())
                .is_some_and(|orbit| *orbit != self.root)
            || self
                .root
                .parent()
                .is_some_and(|parent| parent.join(ORBIT_DIR).exists());
        if !anchored
            && self.root.file_name().and_then(|name| name.to_str()) == Some(BOARD_DIR)
            && self
                .root
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                != Some(ORBIT_DIR)
            && self
                .root
                .parent()
                .is_some_and(|parent| !crate::git::is_git_repo(parent))
        {
            return true;
        }
        // Discovery flagged a legacy layout. `ambiguous` alone does not gate:
        // the selected Board may already be standalone while a shadowed
        // legacy Board remains for `doctor` to report.
        if let Some(discovery) = discovery
            && discovery.legacy.is_some()
        {
            return true;
        }
        false
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
        // An Orbit folder named directly resolves its hidden `board/` or the
        // `board` pointer rather than looking for a nested `.orbit`.
        if root
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new(ORBIT_DIR))
            && let Some(board) = resolve_orbit_dir(&root)
        {
            return Ok((board.root().to_path_buf(), board.is_legacy()));
        }
        // An Orbit folder resolves its hidden `board/` or the `board` pointer.
        let orbit = root.join(".orbit");
        if let Some(board) = resolve_orbit_dir(&orbit) {
            return Ok((board.root().to_path_buf(), board.is_legacy()));
        }
        let legacy_orbit = root.join(".orbit");
        if Self::is_board(&legacy_orbit) {
            return Ok((legacy_orbit, true));
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
            Err(error) if !force => {
                self.commit_failed.set(true);
                Ok(CommitOutcome::Failed(error.message))
            }
            Err(error) => Err(error),
        }
    }

    /// Whether any non-forced commit on this Board failed in this process.
    pub fn commit_failed(&self) -> bool {
        self.commit_failed.get()
    }

    /// Creates a standalone Board folder at `dir`: its own Git repository on
    /// the `orbit` branch (ADR 0019). `dir` is the Board folder itself —
    /// `<anchor>/.orbit/board` for the hidden layout or a visible folder the
    /// caller anchors with an Orbit-folder pointer.
    pub fn init_dir(dir: &Path, slug: Option<&str>) -> Result<InitResult> {
        let root = resolve_external(dir)?;
        if let Some(existing) = Self::conflicting_board_for(&root, false)? {
            return Err(already_initialized(&existing));
        }
        init_external(&root, slug)
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
        if let Some(board) = resolve_orbit_dir(&main)
            && board.root() != dir
            && !is_global_board(board.root())
        {
            return Ok(Some(board.root().to_path_buf()));
        }
        if Self::is_board(&main) && main != dir && !is_global_board(&main) {
            return Ok(Some(main));
        }

        let legacy = legacy_local_path(&common);
        if legacy != dir && Self::is_board(&legacy) {
            return Ok(Some(legacy));
        }

        for parent in dir
            .parent()
            .into_iter()
            .flat_map(|parent| git::ancestors_through(parent, Some(repo_root.as_path())))
        {
            let candidate = parent.join(".orbit");
            if let Some(board) = resolve_orbit_dir(&candidate)
                && board.root() != dir
                && !is_global_board(board.root())
            {
                return Ok(Some(board.root().to_path_buf()));
            }
        }

        for path in git::worktree_paths(&repo_root)? {
            let path = dunce::canonicalize(&path).unwrap_or(path);
            if path == dir || is_global_board(&path) {
                continue;
            }
            if Self::is_board(&path) {
                return Ok(Some(path));
            }
            if let Some(board) = resolve_orbit_dir(&path.join(".orbit"))
                && board.root() != dir
                && !is_global_board(board.root())
            {
                return Ok(Some(board.root().to_path_buf()));
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
        self.write_config(&config)?;
        self.commit("orbit: migrate board storage", true)
    }

    pub fn save_config<F>(&self, message: &str, mutator: F) -> Result<CommitOutcome>
    where
        F: FnOnce(&mut Config) -> Result<()>,
    {
        let mut config = Config::load(&self.root)?;
        mutator(&mut config)?;
        self.write_config(&config)?;
        self.commit(message, false)
    }

    fn write_config(&self, config: &Config) -> Result<()> {
        config.validate()?;
        let toml = config.to_toml()?;
        crate::atomic::write_atomic(&self.root.join("config.toml"), toml.as_bytes(), true)
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
    initialize_board_files(&root, "orbit", true, &prefix)
}

fn initialize_board_files(
    root: &Path,
    branch: &str,
    external: bool,
    prefix: &str,
) -> Result<InitResult> {
    let mut config = Config::default();
    config.id.prefix = prefix.to_string();
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
        storage: StorageMode::Repository,
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
    // An Orbit folder passed directly resolves its Board (`board/` or the
    // `board` pointer); a Board folder passed directly is used as-is.
    let orbit_board = if root.file_name().and_then(|name| name.to_str()) == Some(ORBIT_DIR) {
        resolve_orbit_dir(&root)
    } else {
        resolve_orbit_dir(&root.join(ORBIT_DIR))
    };
    let legacy_nested = root.join(BOARD_DIR);
    let legacy_board = root.join(LEGACY_BOARD_DIR);
    let (selected_root, workspace, legacy, orbit_dir) = if Board::is_board(&root) {
        (root.clone(), false, false, None)
    } else if let Some(board) = orbit_board {
        let legacy = board.is_legacy();
        (
            board.root().to_path_buf(),
            !legacy,
            legacy,
            Some(board.orbit().to_path_buf()),
        )
    } else if Board::is_board(&legacy_nested) {
        (legacy_nested, true, true, None)
    } else if Board::is_board(&legacy_board) {
        (legacy_board, true, true, None)
    } else {
        discovery
            .steps
            .push(DiscoveryStep::new(DiscoverySource::Dir, Some(root.clone())));
        discovery.error = Some(OrbitError::board_not_found());
        return;
    };
    let mut step = DiscoveryStep::new(DiscoverySource::Dir, Some(root.clone()));
    step.found = true;
    step.selected = true;
    discovery.root = Some(selected_root.clone());
    discovery.orbit_dir = orbit_dir;
    discovery.source = DiscoverySource::Dir;
    if legacy {
        let warning = format!(
            "{LEGACY_LAYOUT}: using legacy Workspace Board at {}",
            selected_root.display()
        );
        discovery.legacy = Some(warning.clone());
        push_warning(&mut discovery.warnings, warning);
    }
    let legacy_orbit = root.join(ORBIT_DIR);
    if workspace && selected_root != legacy_orbit && Board::is_board(&legacy_orbit) {
        discovery.ambiguous = Some((selected_root.clone(), legacy_orbit.clone()));
        push_warning(
            &mut discovery.warnings,
            format!(
                "{AMBIGUOUS_BOARD}: both Workspace Board {} and legacy Board {} exist; using {}",
                selected_root.display(),
                legacy_orbit.display(),
                selected_root.display()
            ),
        );
    }
    if workspace {
        match workspace_child_board(&root) {
            Ok(Some(local)) if local != selected_root => {
                discovery.ambiguous = Some((selected_root.clone(), local.clone()));
                push_warning(
                    &mut discovery.warnings,
                    format!(
                        "{AMBIGUOUS_BOARD}: Workspace Board {} and child-repository Board {} both exist; using the Workspace Board",
                        selected_root.display(),
                        local.display()
                    ),
                );
            }
            Ok(_) => {}
            Err(error) => discovery.error = Some(error),
        }
    }
    if !workspace {
        match legacy_local_board_for(&selected_root) {
            Some(other) if other == selected_root => {
                let warning = format!(
                    "{LEGACY_LAYOUT}: using legacy Board at {}",
                    selected_root.display()
                );
                discovery.legacy = Some(warning.clone());
                push_warning(&mut discovery.warnings, warning);
            }
            Some(other) => {
                discovery.ambiguous = Some((selected_root.clone(), other.clone()));
                warn_if_ambiguous(&mut discovery.warnings, &selected_root, &other);
            }
            None => {}
        }
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
        let workspace = nearest_workspace_board(&cwd, home.as_deref());
        let main = resolve_orbit_dir(&main_checkout_for(&repo_root, &common).join(".orbit"));
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
            DiscoverySource::Workspace,
            workspace.as_ref().map(orbit_root),
        ));
        discovery.steps.push(DiscoveryStep::new(
            DiscoverySource::Nearest,
            nearest.as_ref().map(orbit_root),
        ));
        discovery.steps.push(DiscoveryStep::new(
            DiscoverySource::MainCheckout,
            main.as_ref().map(orbit_root),
        ));
        discovery.steps.push(DiscoveryStep::new(
            DiscoverySource::LegacyLocal,
            Some(legacy_local.clone()),
        ));
        discovery.steps.push(DiscoveryStep::new(
            DiscoverySource::Global,
            global.as_ref().map(orbit_root),
        ));

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
        } else if let Some(board) = workspace {
            let (root, _) = orbit_selection(&board);
            discovery.orbit_dir = Some(board.orbit().to_path_buf());
            if let Some(local) = nearest
                .as_ref()
                .map(|local| orbit_selection(local).0)
                .filter(|local| *local != root)
            {
                discovery.ambiguous = Some((root.clone(), local.clone()));
                push_warning(
                    &mut discovery.warnings,
                    format!(
                        "{AMBIGUOUS_BOARD}: Workspace Board {} and child-repository Board {} both exist; using the Workspace Board",
                        root.display(),
                        local.display()
                    ),
                );
            }
            Some((DiscoverySource::Workspace, root))
        } else if let Some(board) = nearest {
            discovery.orbit_dir = Some(board.orbit().to_path_buf());
            let (root, legacy) = orbit_selection(&board);
            if legacy {
                let warning = format!(
                    "{LEGACY_LAYOUT}: using a legacy Board directly at {}",
                    root.display()
                );
                discovery.legacy = Some(warning.clone());
                push_warning(&mut discovery.warnings, warning);
            }
            warn_if_ambiguous(&mut discovery.warnings, &root, &legacy_local);
            Some((DiscoverySource::Nearest, root))
        } else if let Some(board) = main {
            discovery.orbit_dir = Some(board.orbit().to_path_buf());
            let (root, legacy) = orbit_selection(&board);
            if legacy {
                let warning = format!(
                    "{LEGACY_LAYOUT}: using a legacy Board at {}",
                    root.display()
                );
                discovery.legacy = Some(warning.clone());
                push_warning(&mut discovery.warnings, warning);
            }
            warn_if_ambiguous(&mut discovery.warnings, &root, &legacy_local);
            Some((DiscoverySource::MainCheckout, root))
        } else if Board::is_board(&legacy_local) {
            let warning = format!(
                "{LEGACY_LAYOUT}: using legacy Board at {}",
                legacy_local.display()
            );
            discovery.legacy = Some(warning.clone());
            push_warning(&mut discovery.warnings, warning);
            Some((DiscoverySource::LegacyLocal, legacy_local.clone()))
        } else {
            global.map(|board| {
                discovery.orbit_dir = Some(board.orbit().to_path_buf());
                let (root, _) = orbit_selection(&board);
                (DiscoverySource::Global, root)
            })
        };

        if Board::is_board(&legacy_local)
            && let Some((_, root)) = &selected
            && root != &legacy_local
        {
            discovery.ambiguous = Some((root.clone(), legacy_local.clone()));
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
    let workspace = nearest_workspace_board(&cwd, home.as_deref());
    let global = home.as_deref().and_then(global_board);
    discovery.steps.push(DiscoveryStep::new(
        DiscoverySource::Workspace,
        workspace.as_ref().map(orbit_root),
    ));
    discovery.steps.push(DiscoveryStep::new(
        DiscoverySource::Nearest,
        nearest.as_ref().map(orbit_root),
    ));
    discovery.steps.push(DiscoveryStep::new(
        DiscoverySource::Global,
        global.as_ref().map(orbit_root),
    ));

    if let Some(board) = workspace {
        discovery.orbit_dir = Some(board.orbit().to_path_buf());
        let (root, _) = orbit_selection(&board);
        if let Some(legacy) = nearest
            .as_ref()
            .map(|board| orbit_selection(board).0)
            .filter(|path| *path != root)
        {
            discovery.ambiguous = Some((root.clone(), legacy.clone()));
            push_warning(
                &mut discovery.warnings,
                format!(
                    "{AMBIGUOUS_BOARD}: Workspace Board {} and legacy Board {} both exist; using the Workspace Board",
                    root.display(),
                    legacy.display()
                ),
            );
        }
        discovery.source = DiscoverySource::Workspace;
        discovery.root = Some(root);
        discovery.steps[0].selected = true;
    } else if let Some(board) = nearest {
        discovery.orbit_dir = Some(board.orbit().to_path_buf());
        discovery.source = DiscoverySource::Nearest;
        let (root, legacy) = orbit_selection(&board);
        if legacy || root.file_name().and_then(|name| name.to_str()) == Some(".orbit") {
            let warning = format!(
                "{LEGACY_LAYOUT}: using a legacy Board directly at {}",
                root.display()
            );
            discovery.legacy = Some(warning.clone());
            push_warning(&mut discovery.warnings, warning);
        }
        discovery.root = Some(root);
        discovery.steps[1].selected = true;
    } else if let Some(board) = global {
        discovery.orbit_dir = Some(board.orbit().to_path_buf());
        let (root, legacy) = orbit_selection(&board);
        if legacy {
            let warning = format!(
                "{LEGACY_LAYOUT}: using a legacy Board directly at {}",
                root.display()
            );
            discovery.legacy = Some(warning.clone());
            push_warning(&mut discovery.warnings, warning);
        }
        discovery.source = DiscoverySource::Global;
        discovery.root = Some(root);
        discovery.steps[2].selected = true;
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
    git::home_dir().and_then(|home| dunce::canonicalize(home).ok())
}

fn global_board(home: &Path) -> Option<OrbitBoard> {
    resolve_orbit_dir(&home.join(".orbit"))
}

fn is_global_board(path: &Path) -> bool {
    let Some(home) = home_dir() else {
        return false;
    };
    let orbit = home.join(ORBIT_DIR);
    // The global Board folder is `~/.orbit` itself (legacy) or the Board the
    // Orbit folder resolves to — `~/.orbit/board` hidden or a visible folder.
    path == orbit || resolve_orbit_dir(&orbit).is_some_and(|board| board.root() == path)
}

/// The Board folder an Orbit folder resolves to, plus whether that resolution
/// is a pre-ADR-0019 layout `orbit migrate` must convert.
fn orbit_selection(board: &OrbitBoard) -> (PathBuf, bool) {
    (board.root().to_path_buf(), board.is_legacy())
}

/// The Board folder an `OrbitBoard` resolves to, for discovery steps.
fn orbit_root(board: &OrbitBoard) -> PathBuf {
    board.root().to_path_buf()
}

fn nearest_orbit(start: &Path, stop: &Path, skip: Option<&Path>) -> Option<OrbitBoard> {
    for dir in git::ancestors_through(start, Some(stop)) {
        if skip != Some(dir)
            && let Some(board) = resolve_orbit_dir(&dir.join(".orbit"))
        {
            return Some(board);
        }
    }
    None
}

/// The nearest Workspace-level Orbit folder walking up across Git boundaries.
/// Orbit folders inside a Git working tree belong to that repository and are
/// skipped here; the caller's repository-local walk handles them.
fn nearest_workspace_board(start: &Path, skip: Option<&Path>) -> Option<OrbitBoard> {
    for directory in git::ancestors_through(start, None) {
        if skip == Some(directory) {
            continue;
        }
        if git::toplevel_containing(directory).is_some() {
            continue;
        }
        match resolve_orbit_dir(&directory.join(".orbit")) {
            Some(board)
                if matches!(
                    board,
                    OrbitBoard::Hidden { .. } | OrbitBoard::Visible { .. }
                ) =>
            {
                return Some(board);
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn workspace_child_board(workspace: &Path) -> Result<Option<PathBuf>> {
    Ok(workspace_child_boards(workspace)?.into_iter().next())
}

/// Directory names that locate a Board relative to a Workspace or repository.
pub(crate) const ORBIT_DIR: &str = ".orbit";
pub(crate) const BOARD_DIR: &str = "board";
pub(crate) const LEGACY_BOARD_DIR: &str = ".board";
pub(crate) const CACHE_DIR: &str = "cache";

/// How an Orbit folder `.orbit` resolves to a Board folder (ADR 0019):
/// `board/` is the hidden default, a `board` pointer file names a visible
/// Board folder, and a Board directly inside `.orbit` is a legacy layout.
/// Every variant carries the `.orbit` folder and the resolved Board root.
#[derive(Debug, Clone)]
pub(crate) enum OrbitBoard {
    /// `.orbit/board/` is itself the Board folder.
    Hidden { orbit: PathBuf, root: PathBuf },
    /// `.orbit/board` is a pointer file to a visible Board folder.
    Visible { orbit: PathBuf, root: PathBuf },
    /// `.orbit` is itself a Board: a pre-ADR-0019 layout that `orbit migrate`
    /// must convert.
    Legacy { orbit: PathBuf, root: PathBuf },
}

impl OrbitBoard {
    /// The resolved Board folder.
    pub(crate) fn root(&self) -> &Path {
        match self {
            Self::Hidden { root, .. } | Self::Visible { root, .. } | Self::Legacy { root, .. } => {
                root
            }
        }
    }

    /// The `.orbit` folder the resolution went through.
    pub(crate) fn orbit(&self) -> &Path {
        match self {
            Self::Hidden { orbit, .. }
            | Self::Visible { orbit, .. }
            | Self::Legacy { orbit, .. } => orbit,
        }
    }

    /// Whether the resolution is a pre-ADR-0019 layout.
    pub(crate) fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy { .. })
    }
}

/// Resolves an Orbit folder to its Board folder, if it anchors one.
pub(crate) fn resolve_orbit_dir(orbit: &Path) -> Option<OrbitBoard> {
    let hidden = orbit.join(BOARD_DIR);
    if hidden.is_dir() && Board::is_board(&hidden) {
        return Some(OrbitBoard::Hidden {
            orbit: orbit.to_path_buf(),
            root: hidden,
        });
    }
    if hidden.is_file() {
        if let Ok(Some(target)) = read_pointer_file(&hidden)
            && Board::is_board(&target)
        {
            return Some(OrbitBoard::Visible {
                orbit: orbit.to_path_buf(),
                root: target,
            });
        }
        return None;
    }
    if Board::is_board(orbit) {
        return Some(OrbitBoard::Legacy {
            orbit: orbit.to_path_buf(),
            root: orbit.to_path_buf(),
        });
    }
    None
}

/// The `.orbit` folder anchoring `root` when the Board sits at the canonical
/// `.orbit/board` path.
fn discovery_orbit_dir(root: &Path) -> Option<PathBuf> {
    let parent = root.parent()?;
    (parent.file_name().and_then(|name| name.to_str()) == Some(ORBIT_DIR))
        .then(|| parent.to_path_buf())
}

/// Reads the `.orbit/board` pointer file naming a visible Board folder.
pub(crate) fn read_pointer_file(path: &Path) -> Result<Option<PathBuf>> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(OrbitError::io(format!("read {}", path.display()), error)),
    };
    let value = content.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let candidate = PathBuf::from(value);
    if !candidate.is_absolute() {
        return Err(OrbitError::config(format!(
            "{} must contain an absolute path; fix it or re-run `orbit migrate`",
            path.display()
        )));
    }
    Ok(Some(candidate))
}

/// Writes the `.orbit/board` pointer file naming a visible Board folder.
pub(crate) fn write_pointer_file(orbit: &Path, board: &Path) -> Result<()> {
    let path = orbit.join(BOARD_DIR);
    crate::atomic::write_atomic(&path, format!("{}\n", board.display()).as_bytes(), true)
}

/// The Board cache directory beside the Board folder: `<orbit>/cache` when
/// the Orbit folder is known, else `<root>/../cache` for `.orbit/board`.
pub fn board_cache_dir(board: &Board) -> PathBuf {
    if let Some(orbit) = &board.orbit_dir {
        return orbit.join(CACHE_DIR);
    }
    board
        .root
        .parent()
        .filter(|parent| parent.file_name().and_then(|name| name.to_str()) == Some(ORBIT_DIR))
        .map(|parent| parent.join(CACHE_DIR))
        .unwrap_or_else(|| board.root.join(CACHE_DIR))
}

/// Every Board that sits beside or under `workspace` other than its Workspace
/// Board: the workspace-level legacy layouts plus child-repository Boards.
/// Canonical, sorted, and deduplicated.
pub(crate) fn workspace_local_boards(workspace: &Path) -> Result<Vec<PathBuf>> {
    let mut found: Vec<PathBuf> = [LEGACY_BOARD_DIR, BOARD_DIR, ORBIT_DIR]
        .iter()
        .map(|name| workspace.join(name))
        .filter(|candidate| Board::is_board(candidate))
        .collect();
    found.extend(workspace_child_boards(workspace)?);
    let mut found: Vec<PathBuf> = found
        .into_iter()
        .map(|path| dunce::canonicalize(&path).unwrap_or(path))
        .collect();
    found.sort();
    found.dedup();
    Ok(found)
}

/// Enumerates every Board inside Git repositories nested under `workspace`,
/// depth-first. Skips `.git`, `.orbit`, and `.board` directories and does not
/// descend into nested repositories; for each child repository it reports the
/// `.orbit` Board first, then the legacy common-dir `orbit/board`.
pub(crate) fn workspace_child_boards(workspace: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    if !workspace.exists() {
        return Ok(found);
    }
    let mut pending = vec![workspace.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for path in git::child_directories(&directory, false)? {
            let path = path?;
            let name = path.file_name();
            if name == Some(std::ffi::OsStr::new(ORBIT_DIR))
                || name == Some(std::ffi::OsStr::new(".git"))
                || name == Some(std::ffi::OsStr::new(LEGACY_BOARD_DIR))
            {
                continue;
            }
            if path.join(".git").exists() {
                let local = path.join(ORBIT_DIR);
                if let Some(board) = resolve_orbit_dir(&local) {
                    found.push(board.root().to_path_buf());
                }
                if let Ok(common) = git::common_dir_at(&path) {
                    let legacy = common.join("orbit").join(BOARD_DIR);
                    if Board::is_board(&legacy) && !found.contains(&legacy) {
                        found.push(legacy);
                    }
                }
                continue;
            }
            pending.push(path);
        }
    }
    Ok(found)
}

/// The `.git/info/exclude` entries needed to hide an Orbit folder — and an
/// optional visible Board folder — inside a Git working tree. Reports only;
/// use `ensure_orbit_excluded` to write them. `.gitignore` is never touched.
pub fn orbit_exclude_entries(orbit: &Path, visible_board: Option<&Path>) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    if let Some((_, entry)) = worktree_exclusion(orbit)? {
        entries.push(entry);
    }
    if let Some(board) = visible_board
        && let Some((_, entry)) = worktree_exclusion(board)?
        && !entries.contains(&entry)
    {
        entries.push(entry);
    }
    Ok(entries)
}

/// Writes the `.git/info/exclude` entries hiding an Orbit folder and an
/// optional visible Board folder inside a Git working tree (ADR 0019).
pub fn ensure_orbit_excluded(orbit: &Path, visible_board: Option<&Path>) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    if let Some((repo, entry)) = worktree_exclusion(orbit)? {
        git::ensure_exclude_entry(&repo, orbit)?;
        entries.push(entry);
    }
    if let Some(board) = visible_board
        && let Some((repo, entry)) = worktree_exclusion(board)?
    {
        git::ensure_exclude_entry(&repo, board)?;
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

pub(crate) fn main_checkout_for(repo_root: &Path, common: &Path) -> PathBuf {
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
    // A standalone Board folder is its own Git repository; when it sits inside
    // a code repository's working tree, the legacy Board belongs to that
    // enclosing repository's common dir, so look one level out as well.
    let repo = git::toplevel_containing(root)
        .filter(|toplevel| {
            dunce::canonicalize(toplevel).unwrap_or_else(|_| toplevel.clone()) != root
        })
        .or_else(|| root.parent().and_then(git::toplevel_containing));
    if let Some(repo) = repo
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
    if root != legacy && Board::is_board(root) && Board::is_board(legacy) {
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
    fn workspace_local_boards_finds_legacy_and_nested_child_boards() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let board = |path: PathBuf| {
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("config.toml"), "").unwrap();
        };
        board(workspace.join(LEGACY_BOARD_DIR));
        for repo in ["alpha", "group/beta"] {
            std::fs::create_dir_all(workspace.join(repo).join(".git")).unwrap();
            board(workspace.join(repo).join(ORBIT_DIR));
        }
        // The Workspace Board itself and a repository without a Board are not sources.
        board(workspace.join(ORBIT_DIR).join(BOARD_DIR));
        std::fs::create_dir_all(workspace.join("gamma/.git")).unwrap();

        let found = workspace_local_boards(workspace).unwrap();
        let root = dunce::canonicalize(workspace).unwrap();
        let mut expected = vec![
            root.join(LEGACY_BOARD_DIR),
            root.join("alpha").join(ORBIT_DIR),
            root.join("group/beta").join(ORBIT_DIR),
        ];
        expected.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn failed_commit_sets_commit_failed() {
        // A directory outside any Git repository makes `git status` fail.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("task.md"), "x").unwrap();
        let board = Board {
            root: dir.path().to_path_buf(),
            lock_path: dir.path().join(".lock"),
            config: Config::default(),
            warnings: Vec::new(),
            orbit_dir: None,
            commit_failed: Default::default(),
        };
        assert!(!board.commit_failed());
        assert!(matches!(
            board.commit("orbit: test", false).unwrap(),
            CommitOutcome::Failed(_)
        ));
        assert!(board.commit_failed());
    }

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

    #[test]
    fn accepted_id_prefixes_round_trip_and_validate() {
        let mut config = Config::default();
        config.id.accepted_prefixes = vec!["legacy-".to_string()];
        config.validate().unwrap();
        assert!(config.id.is_valid_task_id("legacy-a7b2c9"));
        assert_eq!(
            config.id.parent_id("legacy-a7b2c9.d4b3df"),
            Some("legacy-a7b2c9".to_string())
        );

        let serialized = config.to_toml().unwrap();
        let restored: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.id.accepted_prefixes, vec!["legacy-"]);

        config.id.accepted_prefixes.push("legacy-".to_string());
        assert_eq!(config.validate().unwrap_err().code, code::CONFIG_ERROR);
    }
}
