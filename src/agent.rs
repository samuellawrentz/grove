use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::GroveError;
use crate::state::TaskEntry;
use crate::tmux::{self, PaneInfo};

const STATE_FILE: &str = "/tmp/claude-panes.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum AgentKind {
    Claude,
    OpenCode,
    Codex,
    Cursor,
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Claude => write!(f, "claude"),
            Self::OpenCode => write!(f, "opencode"),
            Self::Codex => write!(f, "codex"),
            Self::Cursor => write!(f, "cursor"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum AgentState {
    Active,
    Waiting,
    #[serde(other)]
    NotRunning,
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Waiting => write!(f, "waiting"),
            Self::NotRunning => write!(f, "not running"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AgentFilter {
    All,
    AnyAgent,
}

#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub kind: AgentKind,
    pub state: AgentState,
}

#[allow(dead_code)]
pub enum DetectStrategy {
    StateFile {
        path: &'static str,
    },
    PaneScrape {
        active_re: Option<Regex>,
        waiting_re: Option<Regex>,
        approval_re: Regex,
    },
}

#[allow(dead_code)]
pub struct AgentDef {
    pub kind: AgentKind,
    pub command_names: &'static [&'static str],
    pub detect: DetectStrategy,
    pub icon: &'static str,
    pub accept_keys: &'static [&'static str],
    pub reject_keys: &'static [&'static str],
    pub default_command: &'static str,
    pub display_name: &'static str,
}

pub const TERMINAL_ICON: &str = "󰆍";

pub static AGENT_REGISTRY: LazyLock<Vec<AgentDef>> = LazyLock::new(|| {
    vec![
        AgentDef {
            kind: AgentKind::Claude,
            command_names: &["claude"],
            detect: DetectStrategy::StateFile { path: "/tmp/claude-panes.json" },
            icon: "󰚩",
            accept_keys: &["Enter"],
            reject_keys: &["n", "Enter"],
            default_command: "claude --dangerously-skip-permissions",
            display_name: "Claude",
        },
        AgentDef {
            kind: AgentKind::OpenCode,
            command_names: &["opencode"],
            detect: DetectStrategy::PaneScrape {
                active_re: Some(Regex::new(r"(?i)(generating|streaming|tool:|reading|writing|searching)").unwrap()),
                waiting_re: Some(Regex::new(r"(?i)(>\s*$|waiting for input|idle)").unwrap()),
                approval_re: Regex::new(r"(?i)(approve|deny|allow|reject|\[y/n\]|\[yes/no\])").unwrap(),
            },
            icon: "󰘦",
            accept_keys: &["y", "Enter"],
            reject_keys: &["n", "Enter"],
            default_command: "opencode",
            display_name: "OpenCode",
        },
        AgentDef {
            kind: AgentKind::Codex,
            command_names: &["codex"],
            detect: DetectStrategy::PaneScrape {
                active_re: None,
                waiting_re: Some(Regex::new(r"(?i)(>\s*$)").unwrap()),
                approval_re: Regex::new(r"(?i)(Would you like to run|Would you like to make|Allow Codex to|Approve app tool call|Do you trust the contents|Enable full access)").unwrap(),
            },
            icon: "󰅪",
            accept_keys: &["y", "Enter"],
            reject_keys: &["n", "Enter"],
            default_command: "codex",
            display_name: "Codex",
        },
        AgentDef {
            kind: AgentKind::Cursor,
            command_names: &["cursor"],
            detect: DetectStrategy::PaneScrape {
                active_re: None,
                waiting_re: Some(Regex::new(r"(?i)(>\s*$|waiting)").unwrap()),
                approval_re: Regex::new(r"(?i)(\[y/n\]|\[yes/no\]|confirm|approve)").unwrap(),
            },
            icon: "󰆍",
            accept_keys: &["Enter"],
            reject_keys: &["n", "Enter"],
            default_command: "cursor",
            display_name: "Cursor",
        },
    ]
});

#[derive(Deserialize)]
struct PaneStateEntry {
    state: AgentState,
}

/// Read the external hook's state file and return agent state per pane ID.
/// Missing file returns an empty map (not an error).
pub fn read_state_file() -> Result<HashMap<String, AgentState>, GroveError> {
    read_state_file_from(Path::new(STATE_FILE))
}

fn read_state_file_from(path: &Path) -> Result<HashMap<String, AgentState>, GroveError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let raw: HashMap<String, PaneStateEntry> = serde_json::from_str(&contents)?;
            Ok(raw.into_iter().map(|(id, e)| (id, e.state)).collect())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e.into()),
    }
}

/// Launch an agent in a tmux pane by sending the command as keystrokes.
pub fn launch_in_pane(target: &str, command: &str, verbose: bool) -> Result<(), GroveError> {
    tmux::send_keys(target, command, verbose)
}

/// Resolve live tmux state for a task: re-query pane ID and check agent state.
/// Returns (tmux_alive, agent_state).
pub fn resolve_task_state(
    task: &TaskEntry,
    agent_states: &HashMap<String, AgentState>,
    verbose: bool,
) -> (bool, AgentState) {
    let Some(ref target) = task.tmux_window else {
        return (false, AgentState::NotRunning);
    };

    match tmux::get_pane_id(target, verbose) {
        Ok(live_pane_id) => {
            let state = agent_states
                .get(&live_pane_id)
                .cloned()
                .unwrap_or(AgentState::NotRunning);
            (true, state)
        }
        Err(_) => (false, AgentState::NotRunning),
    }
}

/// Find which agent def matches a pane's command name or start command.
pub fn identify_agent(pane: &PaneInfo) -> Option<&'static AgentDef> {
    AGENT_REGISTRY.iter().find(|def| {
        def.command_names
            .iter()
            .any(|cmd| pane.current_command.contains(cmd) || pane.start_command.contains(cmd))
    })
}

/// Scrape pane content to detect agent state. Only for PaneScrape agents.
#[allow(dead_code)]
pub fn scrape_pane_state(pane_id: &str, def: &AgentDef, verbose: bool) -> AgentState {
    let content = match tmux::capture_pane_tail(pane_id, 20, verbose) {
        Ok(c) => c,
        Err(_) => return AgentState::NotRunning,
    };

    if let DetectStrategy::PaneScrape {
        active_re,
        waiting_re,
        approval_re,
    } = &def.detect
    {
        if approval_re.is_match(&content) {
            return AgentState::Waiting;
        }
        if let Some(re) = active_re {
            if re.is_match(&content) {
                return AgentState::Active;
            }
        }
        if let Some(re) = waiting_re {
            if re.is_match(&content) {
                return AgentState::NotRunning;
            }
        }
        AgentState::Active
    } else {
        AgentState::NotRunning
    }
}

/// Detect agent + state from state file data + command name.
pub fn detect_agent_in_pane(
    pane: &PaneInfo,
    state_file_states: &HashMap<String, AgentState>,
) -> Option<AgentInfo> {
    // 1. Check state file first (Claude)
    if let Some(state) = state_file_states.get(&pane.pane_id) {
        return Some(AgentInfo {
            kind: AgentKind::Claude,
            state: state.clone(),
        });
    }
    // 2. Check command name against registry
    if let Some(def) = identify_agent(pane) {
        let state = AgentState::Active; // will be refined by scraping later
        return Some(AgentInfo {
            kind: def.kind,
            state,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_pane(pane_id: &str, command: &str) -> PaneInfo {
        PaneInfo {
            pane_id: pane_id.to_string(),
            session_name: "test".to_string(),
            window_index: 0,
            window_name: "test-window".to_string(),
            current_path: PathBuf::from("/tmp"),
            current_command: command.to_string(),
            start_command: String::new(),
            pid: 1,
            activity: 0,
        }
    }

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
        assert_eq!(states.get("%42"), Some(&AgentState::Waiting));
        assert_eq!(states.get("%55"), Some(&AgentState::Active));
        assert_eq!(states.get("%99"), None);
    }

    #[test]
    fn test_unknown_state_maps_to_not_running() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), r#"{ "%10": { "state": "unknown_value" } }"#).unwrap();

        let states = read_state_file_from(tmp.path()).unwrap();
        assert_eq!(states.get("%10"), Some(&AgentState::NotRunning));
    }

    #[test]
    fn test_agent_state_display() {
        assert_eq!(AgentState::Active.to_string(), "active");
        assert_eq!(AgentState::Waiting.to_string(), "waiting");
        assert_eq!(AgentState::NotRunning.to_string(), "not running");
    }

    #[test]
    fn test_agent_registry_has_4_agents() {
        assert_eq!(AGENT_REGISTRY.len(), 4);
    }

    #[test]
    fn test_identify_agent_claude() {
        let pane = make_pane("%1", "claude");
        let def = identify_agent(&pane).expect("should find claude");
        assert_eq!(def.kind, AgentKind::Claude);
    }

    #[test]
    fn test_identify_agent_opencode() {
        let pane = make_pane("%2", "opencode");
        let def = identify_agent(&pane).expect("should find opencode");
        assert_eq!(def.kind, AgentKind::OpenCode);
    }

    #[test]
    fn test_identify_agent_codex() {
        let pane = make_pane("%3", "codex");
        let def = identify_agent(&pane).expect("should find codex");
        assert_eq!(def.kind, AgentKind::Codex);
    }

    #[test]
    fn test_identify_agent_unknown() {
        let pane = make_pane("%4", "vim");
        assert!(identify_agent(&pane).is_none());
    }
}
