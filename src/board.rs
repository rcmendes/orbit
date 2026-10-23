use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{OrbitError, Result, code};
use crate::git::{self, CommitOutcome};
use crate::lock::{self, BoardLock};
use crate::task;

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
        if !(6..=8).contains(&self.id.length) {
            return Err(OrbitError::config("id.length must be between 6 and 8"));
        }
        if self.id.prefix.trim().is_empty() || self.id.prefix.chars().any(char::is_whitespace) {
            return Err(OrbitError::config(
                "id.prefix must be non-empty and contain no whitespace",
            ));
        }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardConfig {
    #[serde(default = "default_branch")]
    pub branch: String,
    #[serde(default = "default_lease")]
    pub lease_minutes: i64,
    #[serde(default = "task::default_priority")]
    pub default_priority: u8,
    #[serde(default = "default_true")]
    pub auto_commit: bool,
    #[serde(default = "default_lock_timeout")]
    pub lock_timeout_ms: u64,
}

impl Default for BoardConfig {
    fn default() -> Self {
        Self {
            branch: default_branch(),
            lease_minutes: default_lease(),
            default_priority: task::default_priority(),
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

fn default_branch() -> String {
    "orbit".to_string()
}

fn default_lease() -> i64 {
    30
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
}

pub struct Board {
    pub root: PathBuf,
    pub lock_path: PathBuf,
    pub config: Config,
}

impl Board {
    pub fn discover(dir_override: Option<&Path>) -> Result<Self> {
        if let Some(dir) = dir_override {
            let root = dunce::canonicalize(dir).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    OrbitError::board_not_found()
                } else {
                    OrbitError::io(format!("resolve {}", dir.display()), error)
                }
            })?;
            let config = Config::load(&root)?;
            let lock_path = root.join(".lock");
            return Ok(Self {
                root,
                lock_path,
                config,
            });
        }

        let common = git::common_dir()?;
        let root = common.join("orbit").join("board");
        if !root.join("config.toml").exists() {
            return Err(OrbitError::board_not_found());
        }
        let config = Config::load(&root)?;
        let lock_path = common.join("orbit").join("lock");
        Ok(Self {
            root,
            lock_path,
            config,
        })
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

    pub fn init(dir_override: Option<&Path>) -> Result<InitResult> {
        match dir_override {
            Some(dir) => init_external(dir),
            None => init_default(),
        }
    }
}

fn init_default() -> Result<InitResult> {
    let common = git::common_dir()?;
    let repo = git::repo_root()?;
    let orbit_dir = common.join("orbit");
    std::fs::create_dir_all(&orbit_dir)
        .map_err(|error| OrbitError::io(format!("create {}", orbit_dir.display()), error))?;

    let _init_lock = BoardLock::acquire(&orbit_dir.join("init.lock"), 2000, true)?;

    let board_root = orbit_dir.join("board");
    if board_root.join("config.toml").exists() {
        return Err(already_initialized(&board_root));
    }

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

    if let Err(error) = git::worktree_add(&repo, &board_root, branch) {
        if created_branch {
            let _ = git::delete_branch(&repo, branch);
        }
        return Err(error);
    }

    if board_root.join("config.toml").exists() {
        return Ok(InitResult {
            board: board_root,
            branch: branch.to_string(),
            external: false,
        });
    }

    let result = initialize_board_files(&board_root, branch, false);
    if result.is_err() {
        let _ = git::worktree_remove(&repo, &board_root);
        if created_branch {
            let _ = git::delete_branch(&repo, branch);
        }
    }
    result
}

fn init_external(dir: &Path) -> Result<InitResult> {
    let root = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| OrbitError::io("resolve current directory", error))?
            .join(dir)
    };

    if root.join("config.toml").exists() {
        return Err(already_initialized(&root));
    }

    std::fs::create_dir_all(&root)
        .map_err(|error| OrbitError::io(format!("create {}", root.display()), error))?;

    let _init_lock = BoardLock::acquire(&root.join(".init.lock"), 2000, true)?;

    if !git::is_git_repo(&root) {
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
    initialize_board_files(&root, "orbit", true)
}

fn initialize_board_files(root: &Path, branch: &str, external: bool) -> Result<InitResult> {
    let config = Config::default();
    let config_toml = config.to_toml()?;

    write_file(&root.join("config.toml"), config_toml.as_bytes())?;
    write_file(
        &root.join(".gitignore"),
        b".lock\n.init.lock\n.tmp-orbit-*\ntasks/.tmp-orbit-*\n",
    )?;
    std::fs::create_dir_all(root.join("tasks")).map_err(|error| {
        OrbitError::io(format!("create {}", root.join("tasks").display()), error)
    })?;
    write_file(&root.join("tasks").join(".gitkeep"), b"")?;

    git::commit(root, "orbit: initialize board")?;

    Ok(InitResult {
        board: root.to_path_buf(),
        branch: branch.to_string(),
        external,
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)
        .map_err(|error| OrbitError::io(format!("write {}", path.display()), error))
}

fn already_initialized(path: &Path) -> OrbitError {
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
    }
}
