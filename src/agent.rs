use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::process::Command;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use crate::db::{Db, TaskEntry};
use crate::error::GroveError;
use crate::tmux::{self, PaneInfo};

const STATE_FILE: &str = "/tmp/claude-panes.json";

/// Entries whose `updated` timestamp is older than this are treated as dead
/// (agent crashed / exited without a cleanup). Entries with no timestamp
/// (legacy producers) are never expired.
const STATE_TTL_SECS: u64 = 300;

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
        let wire = AGENT_REGISTRY
            .iter()
            .find(|def| def.kind == *self)
            .map(|def| def.wire_name)
            .unwrap_or("unknown");
        write!(f, "{wire}")
    }
}

impl AgentKind {
    pub fn parse(s: &str) -> Option<Self> {
        AGENT_REGISTRY
            .iter()
            .find(|def| def.wire_name == s)
            .map(|def| def.kind)
    }

    /// Match a command line (binary path or argv) against the registry.
    pub fn from_command(cmd: &str) -> Option<Self> {
        AGENT_REGISTRY
            .iter()
            .find(|def| def.command_names.iter().any(|n| cmd.contains(n)))
            .map(|d| d.kind)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum AgentState {
    Active,
    Waiting,
    /// Agent finished its turn (Stop hook fired) and is awaiting the user's
    /// next input. Distinct from `NotRunning`: the agent is alive, just idle.
    Idle,
    /// We know the agent KIND but have no live STATE signal. Must NOT be
    /// treated as actively working.
    Unknown,
    #[serde(other)]
    NotRunning,
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Waiting => write!(f, "waiting"),
            Self::Idle => write!(f, "idle"),
            Self::Unknown => write!(f, "unknown"),
            Self::NotRunning => write!(f, "not running"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AgentFilter {
    AnyAgent,
    Others,
}

#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub kind: AgentKind,
    pub state: AgentState,
}

#[allow(dead_code)]
pub struct AgentDef {
    pub kind: AgentKind,
    pub wire_name: &'static str,
    pub launch_key: char,
    pub command_names: &'static [&'static str],
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
            wire_name: "claude",
            launch_key: 'c',
            command_names: &["claude"],
            icon: "󰚩",
            accept_keys: &["Enter"],
            reject_keys: &["n", "Enter"],
            default_command: "claude --dangerously-skip-permissions",
            display_name: "Claude",
        },
        AgentDef {
            kind: AgentKind::OpenCode,
            wire_name: "opencode",
            launch_key: 'o',
            command_names: &["opencode"],
            icon: "󰘦",
            accept_keys: &["y", "Enter"],
            reject_keys: &["n", "Enter"],
            default_command: "opencode",
            display_name: "OpenCode",
        },
        AgentDef {
            kind: AgentKind::Codex,
            wire_name: "codex",
            launch_key: 'x',
            command_names: &["codex"],
            icon: "󰅪",
            accept_keys: &["y", "Enter"],
            reject_keys: &["n", "Enter"],
            default_command: "codex",
            display_name: "Codex",
        },
        AgentDef {
            kind: AgentKind::Cursor,
            wire_name: "cursor",
            launch_key: 'u',
            command_names: &["cursor"],
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
    /// Which agent wrote this entry. Optional for legacy producers (assumed Claude).
    #[serde(default)]
    kind: Option<String>,
    /// Unix seconds of the last update; used to expire dead entries.
    #[serde(default)]
    updated: Option<u64>,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_stale(entry: &PaneStateEntry, now: u64) -> bool {
    matches!(entry.updated, Some(u) if now.saturating_sub(u) > STATE_TTL_SECS)
}

/// Read and parse the raw state file. Missing file returns an empty map.
fn read_entries(path: &Path) -> Result<HashMap<String, PaneStateEntry>, GroveError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(serde_json::from_str(&contents)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e.into()),
    }
}

/// Read the external hook's state file and return agent state per pane ID.
/// Stale entries (see `STATE_TTL_SECS`) are dropped. Missing file returns an
/// empty map (not an error).
pub fn read_state_file() -> Result<HashMap<String, AgentState>, GroveError> {
    read_state_file_from(Path::new(STATE_FILE), now_unix())
}

fn read_state_file_from(path: &Path, now: u64) -> Result<HashMap<String, AgentState>, GroveError> {
    let raw = read_entries(path)?;
    Ok(raw
        .into_iter()
        .filter(|(_, e)| !is_stale(e, now))
        .map(|(id, e)| (id, e.state))
        .collect())
}

/// Read pane_id -> AgentKind for entries that declare a `kind`. Lets non-Claude
/// agents (Codex, OpenCode, ...) report their identity via the same state file.
pub fn read_state_kinds() -> HashMap<String, AgentKind> {
    read_state_kinds_from(Path::new(STATE_FILE), now_unix())
}

fn read_state_kinds_from(path: &Path, now: u64) -> HashMap<String, AgentKind> {
    let Ok(raw) = read_entries(path) else {
        return HashMap::new();
    };
    raw.into_iter()
        .filter(|(_, e)| !is_stale(e, now))
        .filter_map(|(id, e)| {
            e.kind
                .as_deref()
                .and_then(AgentKind::parse)
                .map(|k| (id, k))
        })
        .collect()
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

/// Cache of resolved agent kinds keyed by pane_id. Invalidated when the pane's
/// pid changes (which means the pane was respawned or the agent process exited).
#[derive(Clone, Copy)]
struct CachedKind {
    pane_pid: u32,
    kind: AgentKind,
}

static TREE_CACHE: LazyLock<Mutex<HashMap<String, CachedKind>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Walk the pane's process tree (via `pgrep -P`) and inspect each descendant's
/// argv (via `ps -o command=`) for an agent binary. Result is cached per pane.
pub fn detect_via_process_tree(pane: &PaneInfo) -> Option<AgentKind> {
    {
        let cache = TREE_CACHE.lock().unwrap();
        if let Some(c) = cache.get(&pane.pane_id) {
            if c.pane_pid == pane.pid {
                return Some(c.kind);
            }
        }
    }

    let mut frontier = vec![pane.pid];
    let mut seen = Vec::with_capacity(8);
    while let Some(pid) = frontier.pop() {
        if seen.contains(&pid) {
            continue;
        }
        seen.push(pid);

        if let Some(cmd) = process_command(pid) {
            if let Some(kind) = AgentKind::from_command(&cmd) {
                let mut cache = TREE_CACHE.lock().unwrap();
                cache.insert(
                    pane.pane_id.clone(),
                    CachedKind {
                        pane_pid: pane.pid,
                        kind,
                    },
                );
                return Some(kind);
            }
        }

        for child in child_pids(pid) {
            frontier.push(child);
        }
    }
    None
}

fn child_pids(parent: u32) -> Vec<u32> {
    let Ok(out) = Command::new("pgrep")
        .args(["-P", &parent.to_string()])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect()
}

fn process_command(pid: u32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// The three identity signals a pane can carry, keyed by pane_id.
pub struct AgentSources {
    /// Live state reported by the agent hook (state file).
    pub state_map: HashMap<String, AgentState>,
    /// Kind declared by the agent in the state file.
    pub state_kind_map: HashMap<String, AgentKind>,
    /// Kind recorded by grove at launch (DB).
    pub db_kind_map: HashMap<String, AgentKind>,
}

/// Resolves a pane's agent identity + state from the available sources.
pub struct AgentResolver;

impl AgentResolver {
    /// Resolve agent + state, in priority order:
    ///   1. live state file → kind from declared/recorded (default Claude), real state
    ///   2. grove-recorded kind (DB) → kind known, state Unknown (no live signal)
    ///   3. tmux `current_command` / `start_command` substring match → Unknown
    ///   4. process-tree walk (e.g. `cursor` running as a `node` subprocess) → Unknown
    ///
    /// A kind-known-but-state-unknown pane must NOT be reported as active: a
    /// grove-launched pane whose hook never fired should show `Unknown`, not a
    /// stuck `Active`.
    pub fn resolve(pane: &PaneInfo, sources: &AgentSources) -> Option<AgentInfo> {
        // 1. State file (agent hook). Use the declared/recorded kind if known,
        //    falling back to Claude for legacy entries that carry no kind.
        if let Some(state) = sources.state_map.get(&pane.pane_id) {
            let kind = sources
                .state_kind_map
                .get(&pane.pane_id)
                .or_else(|| sources.db_kind_map.get(&pane.pane_id))
                .copied()
                .unwrap_or(AgentKind::Claude);
            return Some(AgentInfo {
                kind,
                state: state.clone(),
            });
        }
        // 2. Grove-recorded kind (authoritative for panes grove launched). No
        //    live state signal yet → Unknown, never Active.
        if let Some(kind) = sources.db_kind_map.get(&pane.pane_id) {
            return Some(AgentInfo {
                kind: *kind,
                state: AgentState::Unknown,
            });
        }
        // 3. tmux command-name substring match
        if let Some(def) = identify_agent(pane) {
            return Some(AgentInfo {
                kind: def.kind,
                state: AgentState::Unknown,
            });
        }
        // 4. Process-tree fallback
        if let Some(kind) = detect_via_process_tree(pane) {
            return Some(AgentInfo {
                kind,
                state: AgentState::Unknown,
            });
        }
        None
    }
}

/// Thin wrapper preserved for callers that already merged state-file kinds into
/// `recorded_kinds`. Builds an `AgentSources` and delegates to `AgentResolver`.
pub fn detect_agent_in_pane(
    pane: &PaneInfo,
    state_file_states: &HashMap<String, AgentState>,
    recorded_kinds: &HashMap<String, AgentKind>,
) -> Option<AgentInfo> {
    let sources = AgentSources {
        state_map: state_file_states.clone(),
        state_kind_map: HashMap::new(),
        db_kind_map: recorded_kinds.clone(),
    };
    AgentResolver::resolve(pane, &sources)
}

/// Typed facade over the raw `pane_agents` DB table: records the agent kind for
/// panes grove launched, reads them back as `AgentKind`, and garbage-collects
/// rows whose panes are gone. The raw `Db` methods are sealed (`pub(crate)`) so
/// all access flows through here.
pub struct PaneAgentStore<'a> {
    db: &'a Db,
}

impl<'a> PaneAgentStore<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// Record (or update) the agent kind for a pane.
    pub fn record(&self, pane_id: &str, kind: AgentKind) -> Result<(), GroveError> {
        self.db.record_pane_agent(pane_id, &kind.to_string())
    }

    /// All recorded pane_id → AgentKind, dropping rows whose kind no longer parses.
    pub fn kinds(&self) -> HashMap<String, AgentKind> {
        let Ok(raw) = self.db.list_pane_agents() else {
            return HashMap::new();
        };
        raw.into_iter()
            .filter_map(|(pane_id, kind)| AgentKind::parse(&kind).map(|k| (pane_id, k)))
            .collect()
    }

    /// Drop recorded rows for panes that are no longer live.
    pub fn gc(&self, live_pane_ids: &std::collections::HashSet<String>) -> Result<(), GroveError> {
        for pane_id in self.kinds().keys() {
            if !live_pane_ids.contains(pane_id) {
                self.db.delete_pane_agent(pane_id)?;
            }
        }
        Ok(())
    }

    /// Remove a single pane's recorded row.
    pub fn remove(&self, pane_id: &str) -> Result<(), GroveError> {
        self.db.delete_pane_agent(pane_id)
    }
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
        let result = read_state_file_from(path, 0).unwrap();
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

        let states = read_state_file_from(tmp.path(), 0).unwrap();
        assert_eq!(states.get("%42"), Some(&AgentState::Waiting));
        assert_eq!(states.get("%55"), Some(&AgentState::Active));
        assert_eq!(states.get("%99"), None);
    }

    #[test]
    fn test_unknown_state_maps_to_not_running() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), r#"{ "%10": { "state": "unknown_value" } }"#).unwrap();

        let states = read_state_file_from(tmp.path(), 0).unwrap();
        assert_eq!(states.get("%10"), Some(&AgentState::NotRunning));
    }

    #[test]
    fn test_stale_entries_dropped() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        // fresh @1000, stale @100, legacy (no timestamp) all in one file.
        std::fs::write(
            tmp.path(),
            r#"{
                "%fresh": { "state": "waiting", "updated": 1000 },
                "%stale": { "state": "waiting", "updated": 100 },
                "%legacy": { "state": "active" }
            }"#,
        )
        .unwrap();

        // now = 1200; TTL = 300 -> %stale (age 1100) dropped, others kept.
        let states = read_state_file_from(tmp.path(), 1200).unwrap();
        assert_eq!(states.get("%fresh"), Some(&AgentState::Waiting));
        assert_eq!(states.get("%legacy"), Some(&AgentState::Active));
        assert_eq!(states.get("%stale"), None);
    }

    #[test]
    fn test_read_state_kinds() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            r#"{
                "%1": { "state": "active", "kind": "codex" },
                "%2": { "state": "waiting", "kind": "opencode" },
                "%3": { "state": "active" },
                "%4": { "state": "active", "kind": "bogus" }
            }"#,
        )
        .unwrap();

        let kinds = read_state_kinds_from(tmp.path(), 0);
        assert_eq!(kinds.get("%1"), Some(&AgentKind::Codex));
        assert_eq!(kinds.get("%2"), Some(&AgentKind::OpenCode));
        assert_eq!(kinds.get("%3"), None); // no kind declared
        assert_eq!(kinds.get("%4"), None); // unparseable kind
    }

    #[test]
    fn test_detect_uses_state_file_kind() {
        // A pane grove launched as Codex, reporting via the shared state file,
        // must NOT be mislabelled as Claude.
        let pane = make_pane("%7", "zsh");
        let mut states = HashMap::new();
        states.insert("%7".to_string(), AgentState::Waiting);
        let mut recorded = HashMap::new();
        recorded.insert("%7".to_string(), AgentKind::Codex);

        let info = detect_agent_in_pane(&pane, &states, &recorded).unwrap();
        assert_eq!(info.kind, AgentKind::Codex);
        assert_eq!(info.state, AgentState::Waiting);
    }

    #[test]
    fn test_detect_state_file_defaults_to_claude() {
        let pane = make_pane("%8", "zsh");
        let mut states = HashMap::new();
        states.insert("%8".to_string(), AgentState::Active);
        let recorded = HashMap::new();

        let info = detect_agent_in_pane(&pane, &states, &recorded).unwrap();
        assert_eq!(info.kind, AgentKind::Claude);
    }

    #[test]
    fn test_agent_state_display() {
        assert_eq!(AgentState::Active.to_string(), "active");
        assert_eq!(AgentState::Waiting.to_string(), "waiting");
        assert_eq!(AgentState::Idle.to_string(), "idle");
        assert_eq!(AgentState::Unknown.to_string(), "unknown");
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

    #[test]
    fn ag_resolve_kind_known_state_unknown_not_active() {
        // A pane grove launched (DB kind known) but whose hook never fired must
        // resolve to Unknown — never a stuck Active.
        let mut db_kind_map = HashMap::new();
        db_kind_map.insert("%7".to_string(), AgentKind::Codex);
        let sources = AgentSources {
            state_map: HashMap::new(),
            state_kind_map: HashMap::new(),
            db_kind_map,
        };

        let info = AgentResolver::resolve(&make_pane("%7", "zsh"), &sources).unwrap();
        assert_eq!(info.kind, AgentKind::Codex);
        assert_eq!(info.state, AgentState::Unknown);
        assert_ne!(info.state, AgentState::Active);
    }

    #[test]
    fn ag_kind_serde_wire_strings() {
        let cases = [
            (AgentKind::Claude, "\"claude\""),
            (AgentKind::OpenCode, "\"opencode\""),
            (AgentKind::Codex, "\"codex\""),
            (AgentKind::Cursor, "\"cursor\""),
        ];
        for (kind, wire) in cases {
            assert_eq!(serde_json::to_string(&kind).unwrap(), wire);
            let back: AgentKind = serde_json::from_str(wire).unwrap();
            assert_eq!(back, kind);
        }
    }

    fn open_temp_db() -> crate::db::Db {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_path_buf();
        std::mem::forget(f);
        crate::db::Db::open_path(&path).unwrap()
    }

    #[test]
    fn pas_gc_removes_dead_pane() {
        let db = open_temp_db();
        let store = PaneAgentStore::new(&db);
        store.record("%1", AgentKind::Claude).unwrap();
        store.record("%2", AgentKind::Codex).unwrap();

        let live: std::collections::HashSet<String> = ["%1".to_string()].into_iter().collect();
        store.gc(&live).unwrap();

        let kinds = store.kinds();
        assert_eq!(kinds.get("%1"), Some(&AgentKind::Claude));
        assert_eq!(kinds.get("%2"), None);
    }

    #[test]
    fn pas_remove_clears_row() {
        let db = open_temp_db();
        let store = PaneAgentStore::new(&db);
        store.record("%1", AgentKind::Claude).unwrap();
        store.remove("%1").unwrap();
        assert!(store.kinds().is_empty());
    }
}
