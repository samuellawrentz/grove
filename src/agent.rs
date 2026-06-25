use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::process::Command;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use crate::db::{Db, TaskEntry};
use crate::error::GroveError;
use crate::tmux::{self, PaneInfo};

const STATE_FILE_NAME: &str = "claude-panes.json";

/// Resolve the shared agent state-file path, user-scoped so it is never a fixed
/// world-writable `/tmp/<name>` that another user could pre-create (TOCTOU /
/// symlink hazard). Precedence:
///   1. `$GROVE_STATE_FILE` — explicit override (kept in sync with the hook).
///   2. `$XDG_RUNTIME_DIR/grove/<name>` — per-user runtime dir (mode 0700).
///   3. `/tmp/grove-<user>/<name>` — uid/user-scoped subdir, not a shared name.
///
/// The shell hook (`hooks/agent-tmux-status.sh`) computes the same path.
fn state_file_path() -> std::path::PathBuf {
    if let Ok(explicit) = std::env::var("GROVE_STATE_FILE") {
        if !explicit.is_empty() {
            return std::path::PathBuf::from(explicit);
        }
    }
    if let Some(runtime) = dirs::runtime_dir() {
        return runtime.join("grove").join(STATE_FILE_NAME);
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    std::env::temp_dir()
        .join(format!("grove-{user}"))
        .join(STATE_FILE_NAME)
}

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

/// Whether a state entry should be dropped as dead.
///
/// Only transient states expire by time: an `Active` turn that stopped updating
/// has almost certainly crashed without firing its idle/stop hook, and must not
/// show green forever. The "needs you" states (`Idle`, `Waiting`) are stable —
/// a pane can legitimately sit there for hours — so they persist regardless of
/// age. Liveness is handled elsewhere: the tree only colors panes that exist in
/// the live tmux list, so a lingering entry for a dead pane renders nothing.
fn is_stale(entry: &PaneStateEntry, now: u64) -> bool {
    match entry.state {
        AgentState::Idle | AgentState::Waiting => false,
        _ => matches!(entry.updated, Some(u) if now.saturating_sub(u) > STATE_TTL_SECS),
    }
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
    read_state_file_from(&state_file_path(), now_unix())
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
    read_state_kinds_from(&state_file_path(), now_unix())
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

/// Known interactive shells. A pane whose top process is a bare shell with no
/// agent child is the common case; we still walk it once but cache the negative
/// result so the next tick is cheap.
const KNOWN_SHELLS: &[&str] = &["bash", "zsh", "sh", "fish"];

fn is_known_shell(cmd: &str) -> bool {
    KNOWN_SHELLS.contains(&cmd)
}

/// How long a negative (no-agent) result stays cached before we re-walk, so an
/// agent launched *later* under the same shell pid is still discovered.
const TREE_NEG_TTL_SECS: u64 = 30;
/// Cap on cached pane entries; oldest-stamped entries are evicted past this.
const TREE_CACHE_CAP: usize = 512;

/// Cache of resolved agent kinds keyed by pane_id. A positive entry is
/// invalidated when the pane's pid changes (pane respawned / agent exited); a
/// negative entry (`kind == None`) also expires after `TREE_NEG_TTL_SECS` so a
/// later-launched agent under the same shell pid is rediscovered.
#[derive(Clone, Copy)]
struct CachedKind {
    pane_pid: u32,
    kind: Option<AgentKind>,
    stamp: u64,
}

/// Bounded, TTL-aware process-tree cache with an injectable walk resolver and
/// clock so it is testable without spawning `pgrep`/`ps`.
struct ProcessTreeCache {
    entries: HashMap<String, CachedKind>,
}

impl ProcessTreeCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Look up a pane's agent kind, walking only when the cache misses.
    ///
    /// `child_resolver(pid)` reports the pane's direct child pids; `walk(pid)`
    /// performs the deep process-tree walk. For a known shell with NO children
    /// we short-circuit to a cheap negative without ever invoking `walk`.
    fn lookup(
        &mut self,
        pane: &PaneInfo,
        now: u64,
        shell_cmd: &str,
        child_resolver: impl Fn(u32) -> Vec<u32>,
        walk: impl Fn(u32) -> Option<AgentKind>,
    ) -> Option<AgentKind> {
        if let Some(c) = self.entries.get(&pane.pane_id) {
            if c.pane_pid == pane.pid {
                match c.kind {
                    Some(kind) => return Some(kind),
                    None if now.saturating_sub(c.stamp) < TREE_NEG_TTL_SECS => return None,
                    None => {} // negative TTL elapsed → re-walk below
                }
            }
        }

        // Cheap path: a known shell with no children can't host an agent — skip
        // the deep walk entirely.
        let kind = if is_known_shell(shell_cmd) && child_resolver(pane.pid).is_empty() {
            None
        } else {
            walk(pane.pid)
        };

        self.insert(pane.pane_id.clone(), pane.pid, kind, now);
        kind
    }

    fn insert(&mut self, pane_id: String, pane_pid: u32, kind: Option<AgentKind>, now: u64) {
        if self.entries.len() >= TREE_CACHE_CAP && !self.entries.contains_key(&pane_id) {
            self.evict_oldest();
        }
        self.entries.insert(
            pane_id,
            CachedKind {
                pane_pid,
                kind,
                stamp: now,
            },
        );
    }

    fn evict_oldest(&mut self) {
        if let Some(oldest) = self
            .entries
            .iter()
            .min_by_key(|(_, c)| c.stamp)
            .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
    }

    /// Drop entries for panes not in `live` and enforce the capacity bound.
    fn gc(&mut self, live: &std::collections::HashSet<String>) {
        self.entries.retain(|k, _| live.contains(k));
        while self.entries.len() > TREE_CACHE_CAP {
            self.evict_oldest();
        }
    }
}

static TREE_CACHE: LazyLock<Mutex<ProcessTreeCache>> =
    LazyLock::new(|| Mutex::new(ProcessTreeCache::new()));

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Deep walk of a pane's process tree (via `pgrep -P`), inspecting each
/// descendant's argv (via `ps -o command=`) for an agent binary.
fn walk_process_tree(root_pid: u32) -> Option<AgentKind> {
    let mut frontier = vec![root_pid];
    let mut seen = Vec::with_capacity(8);
    while let Some(pid) = frontier.pop() {
        if seen.contains(&pid) {
            continue;
        }
        seen.push(pid);

        if let Some(cmd) = process_command(pid) {
            if let Some(kind) = AgentKind::from_command(&cmd) {
                return Some(kind);
            }
        }
        frontier.extend(child_pids(pid));
    }
    None
}

/// Walk the pane's process tree for an agent binary. Result (positive AND
/// negative) is cached per pane; see [`ProcessTreeCache`].
pub fn detect_via_process_tree(pane: &PaneInfo) -> Option<AgentKind> {
    let mut cache = TREE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.lookup(
        pane,
        now_secs(),
        &pane.current_command,
        child_pids,
        walk_process_tree,
    )
}

/// Drop cached process-tree entries for panes no longer live.
pub fn gc_process_tree_cache(live: &std::collections::HashSet<String>) {
    TREE_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .gc(live);
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

/// The three identity signals a pane can carry, keyed by pane_id. Holds
/// borrows of the caller's maps — `resolve` never clones a whole map.
pub struct AgentSources<'a> {
    /// Live state reported by the agent hook (state file).
    pub state_map: &'a HashMap<String, AgentState>,
    /// Kind declared by the agent in the state file.
    pub state_kind_map: &'a HashMap<String, AgentKind>,
    /// Kind recorded by grove at launch (DB).
    pub db_kind_map: &'a HashMap<String, AgentKind>,
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
        // 1. State file (agent hook). For the KIND, the grove launch record (DB)
        //    is authoritative and wins over the agent's self-declared state-file
        //    kind: grove pins the kind at launch, whereas the state file's job is
        //    liveness/state, not identity. The state-file kind is only a fallback
        //    for panes grove didn't launch, and Claude is the last-resort default
        //    for legacy entries that carry no kind anywhere.
        if let Some(state) = sources.state_map.get(&pane.pane_id) {
            let kind = sources
                .db_kind_map
                .get(&pane.pane_id)
                .or_else(|| sources.state_kind_map.get(&pane.pane_id))
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

/// Thin wrapper for callers that carry a single merged kind map (`recorded_kinds`).
/// It is routed through `db_kind_map` — the authoritative kind source that wins
/// in `resolve` — with an empty `state_kind_map`, so the wrapper agrees with the
/// resolver's precedence (recorded kind wins) rather than contradicting it.
pub fn detect_agent_in_pane(
    pane: &PaneInfo,
    state_file_states: &HashMap<String, AgentState>,
    recorded_kinds: &HashMap<String, AgentKind>,
) -> Option<AgentInfo> {
    static EMPTY_KINDS: LazyLock<HashMap<String, AgentKind>> = LazyLock::new(HashMap::new);
    let sources = AgentSources {
        state_map: state_file_states,
        state_kind_map: &EMPTY_KINDS,
        db_kind_map: recorded_kinds,
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

    /// GC dead rows and return the surviving pane_id → AgentKind from the same
    /// read, so callers that need the kinds right after a GC avoid a second
    /// `list_pane_agents` SELECT.
    pub fn gc_returning(
        &self,
        live_pane_ids: &std::collections::HashSet<String>,
    ) -> Result<HashMap<String, AgentKind>, GroveError> {
        let mut kinds = self.kinds();
        let dead: Vec<String> = kinds
            .keys()
            .filter(|id| !live_pane_ids.contains(*id))
            .cloned()
            .collect();
        for pane_id in dead {
            self.db.delete_pane_agent(&pane_id)?;
            kinds.remove(&pane_id);
        }
        Ok(kinds)
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
    fn test_only_active_expires_by_time() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        // A crashed `active` turn (old, no idle/stop hook) must expire; the
        // stable "needs you" states persist no matter how old, since a pane can
        // legitimately sit waiting/idle for hours.
        std::fs::write(
            tmp.path(),
            r#"{
                "%fresh_active":  { "state": "active",  "updated": 1000 },
                "%stale_active":  { "state": "active",  "updated": 100 },
                "%old_waiting":   { "state": "waiting", "updated": 100 },
                "%old_idle":      { "state": "idle",    "updated": 100 }
            }"#,
        )
        .unwrap();

        // now = 1200; TTL = 300 -> only %stale_active (age 1100) dropped.
        let states = read_state_file_from(tmp.path(), 1200).unwrap();
        assert_eq!(states.get("%fresh_active"), Some(&AgentState::Active));
        assert_eq!(states.get("%stale_active"), None);
        assert_eq!(states.get("%old_waiting"), Some(&AgentState::Waiting));
        assert_eq!(states.get("%old_idle"), Some(&AgentState::Idle));
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
        let empty_state = HashMap::new();
        let empty_kind = HashMap::new();
        let sources = AgentSources {
            state_map: &empty_state,
            state_kind_map: &empty_kind,
            db_kind_map: &db_kind_map,
        };

        let info = AgentResolver::resolve(&make_pane("%7", "zsh"), &sources).unwrap();
        assert_eq!(info.kind, AgentKind::Codex);
        assert_eq!(info.state, AgentState::Unknown);
        assert_ne!(info.state, AgentState::Active);
    }

    #[test]
    fn ag_resolve_precedence_pinned() {
        // RESOLVER-precedence-pinned: when both a state-file kind and a DB kind
        // exist for a live pane, the grove-recorded DB kind wins (authoritative
        // launch identity), not the agent's self-declared state-file kind.
        let mut state_map = HashMap::new();
        state_map.insert("%9".to_string(), AgentState::Active);
        let mut state_kind_map = HashMap::new();
        state_kind_map.insert("%9".to_string(), AgentKind::Claude);
        let mut db_kind_map = HashMap::new();
        db_kind_map.insert("%9".to_string(), AgentKind::Codex);
        let sources = AgentSources {
            state_map: &state_map,
            state_kind_map: &state_kind_map,
            db_kind_map: &db_kind_map,
        };
        let info = AgentResolver::resolve(&make_pane("%9", "zsh"), &sources).unwrap();
        assert_eq!(
            info.kind,
            AgentKind::Codex,
            "DB kind must win over state kind"
        );
        assert_eq!(info.state, AgentState::Active);

        // db-kind-only (no live state) => kind known, state Unknown.
        let empty_state = HashMap::new();
        let empty_kind = HashMap::new();
        let sources = AgentSources {
            state_map: &empty_state,
            state_kind_map: &empty_kind,
            db_kind_map: &db_kind_map,
        };
        let info = AgentResolver::resolve(&make_pane("%9", "zsh"), &sources).unwrap();
        assert_eq!(info.kind, AgentKind::Codex);
        assert_eq!(info.state, AgentState::Unknown);

        // state-only legacy (live state, no kind anywhere) => Claude default.
        let empty_db = HashMap::new();
        let sources = AgentSources {
            state_map: &state_map,
            state_kind_map: &empty_kind,
            db_kind_map: &empty_db,
        };
        let info = AgentResolver::resolve(&make_pane("%9", "zsh"), &sources).unwrap();
        assert_eq!(info.kind, AgentKind::Claude);
        assert_eq!(info.state, AgentState::Active);
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
        store.gc_returning(&live).unwrap();

        let kinds = store.kinds();
        assert_eq!(kinds.get("%1"), Some(&AgentKind::Claude));
        assert_eq!(kinds.get("%2"), None);
    }

    use std::cell::Cell;
    use std::collections::HashSet;

    #[test]
    fn ptree_negative_cached() {
        let mut cache = ProcessTreeCache::new();
        let pane = make_pane("%1", "node");
        let calls = Cell::new(0);
        let walk = |_pid| {
            calls.set(calls.get() + 1);
            None
        };
        let children = |_pid| vec![99u32];

        assert_eq!(cache.lookup(&pane, 100, "node", children, walk), None);
        assert_eq!(calls.get(), 1);
        // Second lookup within TTL must NOT invoke the walk.
        assert_eq!(cache.lookup(&pane, 110, "node", children, walk), None);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn ptree_ttl_rewalk() {
        let mut cache = ProcessTreeCache::new();
        let pane = make_pane("%1", "node");
        let found = Cell::new(false);
        let walk = |_pid| {
            if found.get() {
                Some(AgentKind::Cursor)
            } else {
                None
            }
        };
        let children = |_pid| vec![99u32];

        assert_eq!(cache.lookup(&pane, 0, "node", children, walk), None);
        // An agent launches later; re-walk only happens past the TTL window.
        found.set(true);
        let after_ttl = TREE_NEG_TTL_SECS + 1;
        assert_eq!(
            cache.lookup(&pane, after_ttl, "node", children, walk),
            Some(AgentKind::Cursor)
        );
    }

    #[test]
    fn ptree_shell_skip() {
        let mut cache = ProcessTreeCache::new();
        let pane = make_pane("%1", "zsh");
        let walk_calls = Cell::new(0);
        let walk = |_pid| {
            walk_calls.set(walk_calls.get() + 1);
            None
        };
        // Known shell with NO children: deep walk must never run.
        let children = |_pid| Vec::<u32>::new();

        assert_eq!(cache.lookup(&pane, 0, "zsh", children, walk), None);
        assert_eq!(walk_calls.get(), 0);
    }

    #[test]
    fn cache_evicts_dead_and_bounds() {
        let mut cache = ProcessTreeCache::new();
        for i in 0..(TREE_CACHE_CAP + 10) {
            cache.insert(format!("%{i}"), i as u32, None, i as u64);
        }
        assert!(cache.entries.len() <= TREE_CACHE_CAP);

        let keep = TREE_CACHE_CAP + 5; // a surviving (recent) entry
        let live: HashSet<String> = [format!("%{keep}")].into_iter().collect();
        cache.gc(&live);
        assert!(cache.entries.contains_key(&format!("%{keep}")));
        assert!(!cache.entries.contains_key("%0"));
        assert!(cache.entries.len() <= live.len());
    }

    #[test]
    fn detect_borrow_equiv_clone() {
        // Oracle: replay the resolve priority rules over varied inputs and
        // confirm the borrowed-signature path matches.
        let cases: &[(&str, &str, Option<AgentState>, Option<AgentKind>)] = &[
            ("%a", "claude", Some(AgentState::Active), None),
            (
                "%b",
                "zsh",
                Some(AgentState::Waiting),
                Some(AgentKind::Codex),
            ),
            ("%c", "zsh", None, Some(AgentKind::OpenCode)),
            ("%d", "opencode", None, None),
        ];
        for (id, cmd, state, kind) in cases {
            let pane = make_pane(id, cmd);
            let mut states = HashMap::new();
            if let Some(s) = state {
                states.insert(id.to_string(), s.clone());
            }
            let mut recorded = HashMap::new();
            if let Some(k) = kind {
                recorded.insert(id.to_string(), *k);
            }

            let expected = oracle_resolve(&pane, state.as_ref(), *kind);
            let got = detect_agent_in_pane(&pane, &states, &recorded);
            assert_eq!(
                got.as_ref().map(|i| (i.kind, &i.state)),
                expected.as_ref().map(|i| (i.kind, &i.state)),
                "case {id}",
            );
        }
    }

    fn oracle_resolve(
        pane: &PaneInfo,
        state: Option<&AgentState>,
        db_kind: Option<AgentKind>,
    ) -> Option<AgentInfo> {
        if let Some(s) = state {
            return Some(AgentInfo {
                kind: db_kind.unwrap_or(AgentKind::Claude),
                state: s.clone(),
            });
        }
        if let Some(k) = db_kind {
            return Some(AgentInfo {
                kind: k,
                state: AgentState::Unknown,
            });
        }
        identify_agent(pane).map(|def| AgentInfo {
            kind: def.kind,
            state: AgentState::Unknown,
        })
    }

    /// POISON-recovery-consistent (S25): poisoning the shared cache mutex (a
    /// panic while holding the lock) must NOT cascade — a subsequent access
    /// recovers the inner value instead of re-panicking.
    /// STATEFILE-path-hardened (S25): the state-file path must be user-scoped
    /// (runtime dir or `/tmp/grove-<user>/`), never a fixed world-writable
    /// `/tmp/claude-panes.json` that another user could pre-create.
    #[test]
    fn state_file_path_is_user_scoped() {
        // Explicit override wins.
        std::env::set_var("GROVE_STATE_FILE", "/custom/grove-state.json");
        assert_eq!(
            state_file_path(),
            std::path::PathBuf::from("/custom/grove-state.json")
        );
        std::env::remove_var("GROVE_STATE_FILE");

        // Default must be user-scoped, never the old shared fixed name.
        let p = state_file_path();
        let s = p.to_string_lossy();
        assert!(
            s.ends_with(&format!("grove/{STATE_FILE_NAME}")) || s.contains("/grove-"),
            "path must be user-scoped, got {s}"
        );
        assert_ne!(p, std::path::PathBuf::from("/tmp/claude-panes.json"));
    }

    #[test]
    fn poisoned_tree_cache_recovers() {
        // Poison the global TREE_CACHE by panicking while its lock is held.
        let poisoned = std::panic::catch_unwind(|| {
            let _guard = TREE_CACHE.lock().unwrap();
            panic!("poison the cache");
        });
        assert!(poisoned.is_err(), "the helper thread must have panicked");
        assert!(TREE_CACHE.is_poisoned(), "lock should now be poisoned");

        // The production access path must still succeed (recovers inner value).
        let pane = make_pane("%poison", "vim");
        let live: HashSet<String> = HashSet::new();
        let _ = detect_via_process_tree(&pane);
        gc_process_tree_cache(&live);
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
