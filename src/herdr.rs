//! Thin wrapper over the `herdr` CLI.
//!
//! grove no longer owns any multiplexer/agent state: herdr does. Every
//! task→pane relationship is resolved *statelessly*, at call time, by matching a
//! repo's worktree path against the `cwd` (or `foreground_cwd`) of a live herdr
//! agent. That survives herdr restarts and needs no bookkeeping in grove's DB.

use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

/// How long to let a freshly-created pane's shell reach its prompt before typing
/// a command into it. herdr only creates shell panes, so a launch types into the
/// shell; if we type before the shell (zsh + a heavy rc / powerlevel10k) is ready
/// to read, the first keystrokes are dropped (`claude` → `laude`). Empirically a
/// generous settle makes it reliable; the cost is paid once per pane launch.
const PANE_SHELL_SETTLE: Duration = Duration::from_millis(1200);

use serde::Deserialize;

use crate::error::GroveError;

/// One live agent as herdr reports it (`herdr agent list`). Only the fields
/// grove needs are modeled; unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentInfo {
    pub pane_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub agent_status: String,
}

impl AgentInfo {
    /// Does either of this agent's directories point at `worktree_path`?
    fn matches_cwd(&self, worktree_path: &Path) -> bool {
        [self.cwd.as_deref(), self.foreground_cwd.as_deref()]
            .into_iter()
            .flatten()
            .any(|d| paths_equal(d, worktree_path))
    }
}

/// Compare a herdr-reported path string against a worktree path. Falls back to a
/// canonicalized comparison so trailing slashes / `..` / symlinks don't defeat
/// the match, then to a plain string compare when canonicalization fails (e.g.
/// the directory was removed).
fn paths_equal(reported: &str, worktree_path: &Path) -> bool {
    let reported = Path::new(reported);
    if reported == worktree_path {
        return true;
    }
    match (reported.canonicalize(), worktree_path.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Run `herdr <args>` and return the parsed `.result` value.
///
/// herdr emits `{"id":..,"result":{..}}` on success and `{"id":..,"error":{..}}`
/// on failure. A missing binary, non-JSON output, or an `error` object all map
/// to `GroveError`.
fn run(args: &[&str]) -> Result<serde_json::Value, GroveError> {
    let output = Command::new("herdr").args(args).output().map_err(|e| {
        GroveError::General(format!(
            "failed to run herdr (is it installed and on PATH?): {e}"
        ))
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        if output.status.success() {
            return Ok(serde_json::Value::Null);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GroveError::General(format!(
            "herdr {} failed: {}",
            args.first().copied().unwrap_or(""),
            stderr.trim()
        )));
    }

    let value: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|e| GroveError::General(format!("herdr returned non-JSON output: {e}")))?;

    if let Some(err) = value.get("error") {
        return Err(GroveError::General(format!("herdr error: {err}")));
    }

    Ok(value
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

/// All live agents herdr knows about.
pub fn agents() -> Result<Vec<AgentInfo>, GroveError> {
    let result = run(&["agent", "list"])?;
    let agents = result
        .get("agents")
        .cloned()
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    serde_json::from_value(agents)
        .map_err(|e| GroveError::General(format!("could not parse herdr agents: {e}")))
}

/// The full agent record whose cwd matches `worktree_path`, if any.
pub fn resolve_agent_for_cwd(worktree_path: &Path) -> Result<Option<AgentInfo>, GroveError> {
    Ok(agents()?.into_iter().find(|a| a.matches_cwd(worktree_path)))
}

/// The herdr pane id (e.g. "w1:p1") whose agent cwd matches `worktree_path`.
// Superseded by resolve_agent_for_cwd, which returns the whole agent record.
#[allow(dead_code)]
pub fn resolve_pane_for_cwd(worktree_path: &Path) -> Result<Option<String>, GroveError> {
    Ok(resolve_agent_for_cwd(worktree_path)?.map(|a| a.pane_id))
}

/// Type `text` into `pane_id`'s agent and submit it.
///
/// `herdr agent prompt` (herdr >= 0.7.5) atomically delivers the text and the
/// Enter, honoring the pane's bracketed-paste mode, so the turn actually starts
/// instead of the prompt sitting in the input box.
pub fn send(pane_id: &str, text: &str) -> Result<(), GroveError> {
    run(&["agent", "prompt", pane_id, text]).map(|_| ())
}

/// Block until `pane_id`'s agent reaches `status`, or `timeout_ms` elapses.
///
/// A timeout is surfaced as [`GroveError::Timeout`] (exit code 9). herdr reports
/// timeouts via its `error` channel; any error whose text mentions a timeout is
/// mapped to `Timeout`, everything else stays a general failure.
pub fn wait(pane_id: &str, status: &str, timeout_ms: u64) -> Result<(), GroveError> {
    let timeout = timeout_ms.to_string();
    match run(&[
        "agent",
        "wait",
        pane_id,
        "--status",
        status,
        "--timeout",
        &timeout,
    ]) {
        Ok(_) => Ok(()),
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            if msg.contains("timeout") || msg.contains("timed out") {
                Err(GroveError::Timeout(format!(
                    "pane '{pane_id}' did not reach '{status}' within {timeout_ms}ms"
                )))
            } else {
                Err(e)
            }
        }
    }
}

/// Focus a workspace by id.
pub fn focus_workspace(workspace_id: &str) -> Result<(), GroveError> {
    run(&["workspace", "focus", workspace_id]).map(|_| ())
}

/// Close a workspace by id.
pub fn close_workspace(workspace_id: &str) -> Result<(), GroveError> {
    run(&["workspace", "close", workspace_id]).map(|_| ())
}

/// Create a focused workspace rooted at `cwd`. Returns `(workspace_id, root_pane_id)`.
pub fn create_workspace(cwd: &str, label: &str) -> Result<(String, String), GroveError> {
    let result = run(&[
        "workspace",
        "create",
        "--cwd",
        cwd,
        "--label",
        label,
        "--focus",
    ])?;
    let workspace_id = result
        .pointer("/workspace/workspace_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            GroveError::General("herdr workspace create: missing workspace.workspace_id".into())
        })?
        .to_string();
    let root_pane_id = result
        .pointer("/root_pane/pane_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            GroveError::General("herdr workspace create: missing root_pane.pane_id".into())
        })?
        .to_string();
    Ok((workspace_id, root_pane_id))
}

/// Type `cmd` into `pane_id` and run it (via `herdr pane run`).
///
/// Callers pass panes fresh from `create_workspace`/`split_pane`, so we let the
/// shell settle first (see [`PANE_SHELL_SETTLE`]) to avoid dropped leading chars.
pub fn run_in_pane(pane_id: &str, cmd: &str) -> Result<(), GroveError> {
    thread::sleep(PANE_SHELL_SETTLE);
    run(&["pane", "run", pane_id, cmd]).map(|_| ())
}

/// Split `pane_id` to the right with a new pane rooted at `cwd`. Returns the new
/// pane id.
pub fn split_pane(pane_id: &str, cwd: &str) -> Result<String, GroveError> {
    let result = run(&[
        "pane",
        "split",
        pane_id,
        "--direction",
        "right",
        "--cwd",
        cwd,
    ])?;
    result
        .pointer("/pane/pane_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| GroveError::General("herdr pane split: missing pane.pane_id".into()))
}

/// Create a focused workspace at `dir` (label `label`) and optionally run `cmd`
/// in its root pane. Returns the workspace id. Replaces the old tmux
/// new-window launcher used by the TUI's recents/`o` actions.
pub fn launch_workspace(dir: &str, label: &str, cmd: Option<&str>) -> Result<String, GroveError> {
    let (workspace_id, root_pane_id) = create_workspace(dir, label)?;
    if let Some(cmd) = cmd {
        run_in_pane(&root_pane_id, cmd)?;
    }
    Ok(workspace_id)
}
