use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::GroveError;
use crate::state::TaskEntry;
use crate::tmux;

const STATE_FILE: &str = "/tmp/claude-panes.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeState {
    Active,
    Waiting,
    #[serde(other)]
    NotRunning,
}

impl fmt::Display for ClaudeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Waiting => write!(f, "waiting"),
            Self::NotRunning => write!(f, "not running"),
        }
    }
}

#[derive(Deserialize)]
struct PaneStateEntry {
    state: ClaudeState,
}

/// Read the external hook's state file and return Claude state per pane ID.
/// Missing file returns an empty map (not an error).
pub fn read_state_file() -> Result<HashMap<String, ClaudeState>, GroveError> {
    read_state_file_from(Path::new(STATE_FILE))
}

fn read_state_file_from(path: &Path) -> Result<HashMap<String, ClaudeState>, GroveError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let raw: HashMap<String, PaneStateEntry> = serde_json::from_str(&contents)?;
            Ok(raw.into_iter().map(|(id, e)| (id, e.state)).collect())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e.into()),
    }
}

/// Launch Claude in a tmux pane by sending the command as keystrokes.
pub fn launch_in_pane(target: &str, claude_command: &str, verbose: bool) -> Result<(), GroveError> {
    tmux::send_keys(target, claude_command, verbose)
}

/// Resolve live tmux state for a task: re-query pane ID and check Claude state.
/// Returns (tmux_alive, claude_state).
pub fn resolve_task_state(
    task: &TaskEntry,
    claude_states: &HashMap<String, ClaudeState>,
    verbose: bool,
) -> (bool, ClaudeState) {
    let Some(ref target) = task.tmux_window else {
        return (false, ClaudeState::NotRunning);
    };

    match tmux::get_pane_id(target, verbose) {
        Ok(live_pane_id) => {
            let state = claude_states
                .get(&live_pane_id)
                .cloned()
                .unwrap_or(ClaudeState::NotRunning);
            (true, state)
        }
        Err(_) => (false, ClaudeState::NotRunning),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_missing_state_file() {
        let path = Path::new("/tmp/grove-test-nonexistent-state.json");
        let result = read_state_file_from(path).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_state_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            r#"{ "%42": { "state": "waiting" }, "%55": { "state": "active" } }"#,
        )
        .unwrap();

        let states = read_state_file_from(tmp.path()).unwrap();
        assert_eq!(states.get("%42"), Some(&ClaudeState::Waiting));
        assert_eq!(states.get("%55"), Some(&ClaudeState::Active));
        assert_eq!(states.get("%99"), None);
    }

    #[test]
    fn test_unknown_state_maps_to_not_running() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), r#"{ "%10": { "state": "unknown_value" } }"#).unwrap();

        let states = read_state_file_from(tmp.path()).unwrap();
        assert_eq!(states.get("%10"), Some(&ClaudeState::NotRunning));
    }

    #[test]
    fn test_claude_state_display() {
        assert_eq!(ClaudeState::Active.to_string(), "active");
        assert_eq!(ClaudeState::Waiting.to_string(), "waiting");
        assert_eq!(ClaudeState::NotRunning.to_string(), "not running");
    }
}
