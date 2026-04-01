use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)]
#[non_exhaustive]
pub enum GroveError {
    #[error("{0}")]
    General(String),

    #[error("task not found: {0}")]
    TaskNotFound(String),

    #[error("repo not registered: {0}")]
    RepoNotRegistered(String),

    #[error("tmux not running: {0}")]
    TmuxNotRunning(String),

    #[error("uncommitted changes: {0}")]
    UncommittedChanges(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("{0}")]
    Tui(String),

    #[error("database error: {0}")]
    Database(String),
}

impl GroveError {
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            GroveError::General(_) => 1,
            GroveError::TaskNotFound(_) => 2,
            GroveError::RepoNotRegistered(_) => 3,
            GroveError::TmuxNotRunning(_) => 4,
            GroveError::UncommittedChanges(_) => 5,
            GroveError::Conflict(_) => 6,
            GroveError::Tui(_) => 7,
            GroveError::Database(_) => 8,
        }
    }

    #[must_use]
    pub fn variant_name(&self) -> &'static str {
        match self {
            GroveError::General(_) => "general",
            GroveError::TaskNotFound(_) => "task_not_found",
            GroveError::RepoNotRegistered(_) => "repo_not_registered",
            GroveError::TmuxNotRunning(_) => "tmux_not_running",
            GroveError::UncommittedChanges(_) => "uncommitted_changes",
            GroveError::Conflict(_) => "conflict",
            GroveError::Tui(_) => "tui",
            GroveError::Database(_) => "database",
        }
    }

    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "ok": false,
            "error": self.variant_name(),
            "message": self.to_string(),
            "exit_code": self.exit_code(),
        })
    }
}

impl From<std::io::Error> for GroveError {
    fn from(e: std::io::Error) -> Self {
        GroveError::General(e.to_string())
    }
}

impl From<serde_json::Error> for GroveError {
    fn from(e: serde_json::Error) -> Self {
        GroveError::General(format!("JSON error: {e}"))
    }
}

impl From<rusqlite::Error> for GroveError {
    fn from(e: rusqlite::Error) -> Self {
        GroveError::Database(e.to_string())
    }
}
