use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config;
use crate::error::GroveError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    pub name: String,
    pub url: String,
    pub path: PathBuf,
    pub default_branch: String,
    pub registered_at: DateTime<Utc>,
    pub last_synced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRepo {
    pub repo_name: String,
    pub worktree_path: PathBuf,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEntry {
    pub id: String,
    pub path: PathBuf,
    pub repos: Vec<TaskRepo>,
    pub created_at: DateTime<Utc>,
    /// Tmux window target (e.g. "mysession:grove-TASK-1")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_window: Option<String>,
    /// Pane ID of the task window (e.g. "%42")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
}

impl TaskEntry {
    /// Returns true if the task directory or any worktree path is missing on disk.
    pub fn is_stale(&self) -> bool {
        if !self.path.exists() {
            return true;
        }
        self.repos.iter().any(|r| !r.worktree_path.exists())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroveState {
    pub version: u32,
    pub generation: u64,
    pub repos: HashMap<String, RepoEntry>,
    pub tasks: HashMap<String, TaskEntry>,
    pub updated_at: DateTime<Utc>,
}

impl Default for GroveState {
    fn default() -> Self {
        Self {
            version: 1,
            generation: 0,
            repos: HashMap::new(),
            tasks: HashMap::new(),
            updated_at: Utc::now(),
        }
    }
}

impl GroveState {
    /// Load state from the default path. Missing file returns empty state.
    pub fn load() -> Result<Self, GroveError> {
        Self::load_from(&config::state_path())
    }

    /// Load state from a specific path. Missing file returns empty state.
    pub fn load_from(path: &Path) -> Result<Self, GroveError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)?;
        let state: GroveState = serde_json::from_str(&contents)?;
        Ok(state)
    }

    /// Save state atomically (write to temp file, then rename).
    /// Increments generation before writing.
    pub fn save(&mut self) -> Result<(), GroveError> {
        self.save_to(&config::state_path())
    }

    /// Save state atomically to a specific path.
    pub fn save_to(&mut self, path: &Path) -> Result<(), GroveError> {
        self.generation += 1;
        self.updated_at = Utc::now();

        let dir = path
            .parent()
            .ok_or_else(|| GroveError::General("invalid state path".to_string()))?;
        std::fs::create_dir_all(dir)?;

        // Write to temp file in the same directory, then rename for atomicity
        let temp_path = dir.join(format!(".state.{}.tmp", std::process::id()));
        let json = serde_json::to_string_pretty(&self)?;
        std::fs::write(&temp_path, &json)?;
        std::fs::rename(&temp_path, path)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_state() {
        let state = GroveState::default();
        assert_eq!(state.version, 1);
        assert_eq!(state.generation, 0);
        assert!(state.repos.is_empty());
        assert!(state.tasks.is_empty());
    }

    #[test]
    fn test_state_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");

        let mut state = GroveState::default();
        state.repos.insert(
            "test".to_string(),
            RepoEntry {
                name: "test".to_string(),
                url: "https://example.com/test.git".to_string(),
                path: PathBuf::from("/tmp/repos/test.git"),
                default_branch: "main".to_string(),
                registered_at: Utc::now(),
                last_synced_at: None,
            },
        );

        state.save_to(&path).unwrap();
        assert_eq!(state.generation, 1);

        let loaded = GroveState::load_from(&path).unwrap();
        assert_eq!(loaded.generation, 1);
        assert_eq!(loaded.repos.len(), 1);
        assert!(loaded.repos.contains_key("test"));
    }

    #[test]
    fn test_generation_increments() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");

        let mut state = GroveState::default();
        state.save_to(&path).unwrap();
        assert_eq!(state.generation, 1);

        state.save_to(&path).unwrap();
        assert_eq!(state.generation, 2);

        state.save_to(&path).unwrap();
        assert_eq!(state.generation, 3);
    }

    #[test]
    fn test_load_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.json");
        let state = GroveState::load_from(&path).unwrap();
        assert_eq!(state.generation, 0);
        assert!(state.repos.is_empty());
    }

    #[test]
    fn test_atomic_write() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");

        let mut state = GroveState::default();
        state.save_to(&path).unwrap();

        // Verify file exists and is valid JSON
        let contents = std::fs::read_to_string(&path).unwrap();
        let _: GroveState = serde_json::from_str(&contents).unwrap();

        // Verify no temp files left behind
        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name(), "state.json");
    }
}
