use serde_json::Value;

pub type Result<T> = std::result::Result<T, OrbitError>;

pub mod code {
    pub const NOT_FOUND: &str = "NOT_FOUND";
    pub const CORRUPT_TASK: &str = "CORRUPT_TASK";
    pub const DANGLING_DEP: &str = "DANGLING_DEP";
    pub const DANGLING_PARENT: &str = "DANGLING_PARENT";
    pub const NESTED_SUBTASK: &str = "NESTED_SUBTASK";
    pub const PARENT_MISMATCH: &str = "PARENT_MISMATCH";
    pub const CYCLE_DETECTED: &str = "CYCLE_DETECTED";
    pub const DEPENDENCY_BLOCKED: &str = "DEPENDENCY_BLOCKED";
    pub const SUBTASKS_OPEN: &str = "SUBTASKS_OPEN";
    pub const SUBTASKS_EXIST: &str = "SUBTASKS_EXIST";
    pub const ALREADY_CLAIMED: &str = "ALREADY_CLAIMED";
    pub const CLAIM_STALE: &str = "CLAIM_STALE";
    pub const NOT_CLAIM_HOLDER: &str = "NOT_CLAIM_HOLDER";
    pub const WAITING_EXTERNAL: &str = "WAITING_EXTERNAL";
    pub const INVALID_TRANSITION: &str = "INVALID_TRANSITION";
    pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
    pub const INVALID_TYPE: &str = "INVALID_TYPE";
    pub const INVALID_LABEL: &str = "INVALID_LABEL";
    pub const LABEL_BLOCKED: &str = "LABEL_BLOCKED";
    pub const CORRUPT_COMMENT: &str = "CORRUPT_COMMENT";
    pub const ORPHAN_COMMENT: &str = "ORPHAN_COMMENT";
    pub const LOCKED: &str = "LOCKED";
    pub const ALREADY_INITIALIZED: &str = "ALREADY_INITIALIZED";
    pub const BOARD_NOT_FOUND: &str = "BOARD_NOT_FOUND";
    pub const BOARD_CORRUPT: &str = "BOARD_CORRUPT";
    pub const GIT_ERROR: &str = "GIT_ERROR";
    pub const IO_ERROR: &str = "IO_ERROR";
    pub const CONFIG_ERROR: &str = "CONFIG_ERROR";
    pub const ID_COLLISION: &str = "ID_COLLISION";
    pub const NOT_A_REPOSITORY: &str = "NOT_A_REPOSITORY";
    pub const ALREADY_LINKED: &str = "ALREADY_LINKED";
    pub const LINK_NOT_FOUND: &str = "LINK_NOT_FOUND";
    pub const WORKSPACE_INSIDE_REPO: &str = "WORKSPACE_INSIDE_REPO";
    pub const EPHEMERAL_BIN: &str = "EPHEMERAL_BIN";
}

#[derive(Debug, Clone)]
pub struct OrbitError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
    pub details: Option<Value>,
}

impl OrbitError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    pub fn not_found(id: &str) -> Self {
        Self::new(code::NOT_FOUND, format!("task {id} does not exist"))
            .with_details(serde_json::json!({ "task": id }))
    }

    pub fn board_not_found() -> Self {
        Self::new(
            code::BOARD_NOT_FOUND,
            "no Orbit board found; run `orbit init` first",
        )
    }

    pub fn locked() -> Self {
        Self::new(
            code::LOCKED,
            "the board is locked by another Orbit process; retry",
        )
        .retryable()
    }

    pub fn io(context: impl AsRef<str>, error: std::io::Error) -> Self {
        Self::new(code::IO_ERROR, format!("{}: {error}", context.as_ref()))
    }

    pub fn git(message: impl Into<String>) -> Self {
        Self::new(code::GIT_ERROR, message)
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::new(code::CONFIG_ERROR, message)
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(code::INVALID_ARGUMENT, message)
    }

    pub fn corrupt_task(message: impl Into<String>) -> Self {
        Self::new(code::CORRUPT_TASK, message)
    }

    pub fn board_corrupt(message: impl Into<String>, details: Value) -> Self {
        Self::new(code::BOARD_CORRUPT, message).with_details(details)
    }
}

impl std::fmt::Display for OrbitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for OrbitError {}
