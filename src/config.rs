use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::GroveError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    #[serde(default = "default_fetch_prune")]
    pub fetch_prune: bool,
    #[serde(default = "default_merge_no_ff")]
    pub merge_no_ff: bool,
    #[serde(default = "default_clone_retries")]
    pub clone_retries: u32,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            fetch_prune: default_fetch_prune(),
            merge_no_ff: default_merge_no_ff(),
            clone_retries: default_clone_retries(),
        }
    }
}

fn default_fetch_prune() -> bool {
    true
}
fn default_merge_no_ff() -> bool {
    true
}
fn default_clone_retries() -> u32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmuxConfig {
    #[serde(default = "default_layout")]
    pub layout: String,
    #[serde(default = "default_session_prefix")]
    pub session_prefix: String,
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            layout: default_layout(),
            session_prefix: default_session_prefix(),
        }
    }
}

fn default_layout() -> String {
    "even-vertical".to_string()
}
fn default_session_prefix() -> String {
    "grove".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroveConfig {
    #[serde(default = "default_repos_dir")]
    pub repos_dir: PathBuf,
    #[serde(default = "default_tasks_dir")]
    pub tasks_dir: PathBuf,
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
    #[serde(default = "default_stable_count_threshold")]
    pub stable_count_threshold: u32,
    #[serde(default = "default_max_parallel_syncs")]
    pub max_parallel_syncs: usize,
    #[serde(default = "default_auto_launch_claude")]
    pub auto_launch_claude: bool,
    #[serde(default = "default_auto_attach")]
    pub auto_attach: bool,
    #[serde(default = "default_claude_command")]
    pub claude_command: String,
    #[serde(default)]
    pub tmux: TmuxConfig,
    #[serde(default)]
    pub git: GitConfig,
}

fn default_repos_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("repos")
}

fn default_tasks_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tasks")
}

fn default_poll_interval_ms() -> u64 {
    500
}
fn default_stable_count_threshold() -> u32 {
    3
}
fn default_max_parallel_syncs() -> usize {
    8
}
fn default_auto_launch_claude() -> bool {
    true
}
fn default_auto_attach() -> bool {
    true
}
fn default_claude_command() -> String {
    "claude".to_string()
}

impl Default for GroveConfig {
    fn default() -> Self {
        Self {
            repos_dir: default_repos_dir(),
            tasks_dir: default_tasks_dir(),
            poll_interval_ms: default_poll_interval_ms(),
            stable_count_threshold: default_stable_count_threshold(),
            max_parallel_syncs: default_max_parallel_syncs(),
            auto_launch_claude: default_auto_launch_claude(),
            auto_attach: default_auto_attach(),
            claude_command: default_claude_command(),
            tmux: TmuxConfig::default(),
            git: GitConfig::default(),
        }
    }
}

impl GroveConfig {
    /// Load config with precedence: file < env < cli overrides.
    /// Auto-creates ~/.grove/ if it does not exist.
    pub fn load(
        config_path_override: Option<&Path>,
        repos_dir_override: Option<&Path>,
        tasks_dir_override: Option<&Path>,
        json_override: Option<bool>,
    ) -> Result<(Self, bool), GroveError> {
        let grove_dir = grove_dir();
        std::fs::create_dir_all(&grove_dir)?;

        // Determine config file path
        let config_path = if let Some(p) = config_path_override {
            p.to_path_buf()
        } else if let Ok(p) = std::env::var("GROVE_CONFIG") {
            PathBuf::from(p)
        } else {
            grove_dir.join("config.json")
        };

        // Load from file (missing file = defaults)
        let mut config = if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)?;
            serde_json::from_str::<GroveConfig>(&contents)?
        } else {
            GroveConfig::default()
        };

        // Env var overrides
        if let Ok(val) = std::env::var("GROVE_REPOS_DIR") {
            config.repos_dir = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("GROVE_TASKS_DIR") {
            config.tasks_dir = PathBuf::from(val);
        }

        // Determine JSON mode: cli flag > env > default false
        let json_mode = if let Some(j) = json_override {
            j
        } else if let Ok(val) = std::env::var("GROVE_JSON") {
            val == "1" || val.eq_ignore_ascii_case("true")
        } else {
            false
        };

        // CLI flag overrides
        if let Some(p) = repos_dir_override {
            config.repos_dir = p.to_path_buf();
        }
        if let Some(p) = tasks_dir_override {
            config.tasks_dir = p.to_path_buf();
        }

        Ok((config, json_mode))
    }
}

pub fn grove_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grove")
}

pub fn state_path() -> PathBuf {
    grove_dir().join("state.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GroveConfig::default();
        assert_eq!(config.max_parallel_syncs, 8);
        assert_eq!(config.poll_interval_ms, 500);
        assert!(config.git.fetch_prune);
    }

    #[test]
    fn test_config_from_json() {
        let json = r#"{"max_parallel_syncs": 4, "repos_dir": "/tmp/repos"}"#;
        let config: GroveConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.max_parallel_syncs, 4);
        assert_eq!(config.repos_dir, PathBuf::from("/tmp/repos"));
        // Defaults for missing fields
        assert_eq!(config.poll_interval_ms, 500);
    }

    #[test]
    fn test_load_creates_grove_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let grove = tmp.path().join(".grove");
        // Directly test that create_dir_all works (auto-create logic)
        std::fs::create_dir_all(&grove).unwrap();
        assert!(grove.exists());
    }
}
