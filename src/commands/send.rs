use std::collections::HashMap;

use crate::agent::{self, AgentState};
use crate::commands::Ctx;
use crate::db::TaskEntry;
use crate::error::GroveError;
use crate::output;
use crate::tmux::{self, PaneInfo};

/// Pick the pane a prompt should go to, or explain why it cannot be sent.
///
/// The agent-busy guard must be keyed to the *task's* pane. It used to read
/// whichever pane tmux considered active (a `display-message -t <gone-window>`
/// answers with the active pane and exits 0), so an unrelated busy agent on
/// screen could block a send — or a dead task could look idle and sendable.
fn resolve_send_target<'a>(
    task: &TaskEntry,
    panes: &'a [PaneInfo],
    agent_states: &HashMap<String, AgentState>,
) -> Result<&'a PaneInfo, GroveError> {
    if task.tmux_window.is_none() {
        return Err(GroveError::TmuxNotRunning(format!(
            "task '{}' was created without tmux",
            task.id
        )));
    }

    let pane = agent::locate_task_pane(task, panes).ok_or_else(|| {
        GroveError::TmuxNotRunning(format!(
            "tmux window for task '{}' no longer exists",
            task.id
        ))
    })?;

    // Only block when the agent is mid-turn (Active) — sending keystrokes then
    // would interleave with its work. Every other state (Waiting on a prompt,
    // freshly launched / idle at the prompt with no state-file entry yet) is a
    // legitimate moment to send input.
    let state = agent_states
        .get(&pane.pane_id)
        .cloned()
        .unwrap_or(AgentState::NotRunning);
    if state == AgentState::Active {
        return Err(GroveError::General(format!(
            "Agent is busy working (current state: {state}); wait for it to \
             finish its turn before sending. Ensure agent-tmux-status.sh hook is \
             running if state seems wrong."
        )));
    }

    Ok(pane)
}

pub fn run(task_id: &str, prompt: &str, ctx: &Ctx) -> Result<(), GroveError> {
    let db = ctx.db;
    let json_mode = ctx.json_mode;
    let verbose = ctx.verbose;

    let task = db
        .get_task(task_id)?
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?;

    let agent_states = agent::read_state_file().unwrap_or_default();
    let panes = tmux::list_all_panes(verbose).unwrap_or_default();

    let pane = resolve_send_target(&task, &panes, &agent_states)?;
    let live_pane_id = pane.pane_id.clone();

    // Address the pane directly: a pane id is unambiguous and survives the window
    // being renamed, whereas a stale `session:name` target resolves to nothing.
    tmux::send_keys(&live_pane_id, prompt, verbose)?;

    db.heal_pane_id(task_id, &live_pane_id)?;

    let data = serde_json::json!({
        "task_id": task_id,
        "tmux_window": task.tmux_window,
        "pane_id": live_pane_id,
        "prompt_sent": true,
    });
    output::success(json_mode, &format!("Sent prompt to task '{task_id}'"), data);

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn task_with(pane_id: Option<&str>, window: Option<&str>) -> TaskEntry {
        TaskEntry {
            id: "review-gate".to_string(),
            path: PathBuf::from("/home/user/tasks/review-gate"),
            repos: Vec::new(),
            created_at: chrono::Utc::now(),
            tmux_window: window.map(str::to_string),
            pane_id: pane_id.map(str::to_string),
        }
    }

    fn pane(pane_id: &str, path: &str) -> PaneInfo {
        PaneInfo {
            pane_id: pane_id.to_string(),
            session_name: "0".to_string(),
            window_index: 1,
            window_name: "2.1.206".to_string(), // auto-renamed away from grove-*
            current_path: PathBuf::from(path),
            current_command: "zsh".to_string(),
            start_command: "zsh".to_string(),
            pid: 1,
            activity: 0,
        }
    }

    /// The busy-guard reads the *task's* pane, not whatever pane is active. A
    /// prompt for an idle task must go through even while another agent works.
    #[test]
    fn send_is_allowed_when_another_pane_is_busy() {
        let task = task_with(Some("%172"), Some("0:grove-review-gate"));
        let panes = [
            pane("%180", "/home/user/tasks/glance-ship"),
            pane("%172", "/home/user/tasks/review-gate"),
        ];
        let states = HashMap::from([
            ("%180".to_string(), AgentState::Active), // busy, unrelated
            ("%172".to_string(), AgentState::Waiting),
        ]);

        let target = resolve_send_target(&task, &panes, &states).expect("send should be allowed");

        assert_eq!(target.pane_id, "%172");
    }

    /// Conversely, the guard must still fire when the *task's own* agent is busy.
    #[test]
    fn send_is_blocked_when_the_tasks_agent_is_busy() {
        let task = task_with(Some("%172"), Some("0:grove-review-gate"));
        let panes = [pane("%172", "/home/user/tasks/review-gate")];
        let states = HashMap::from([("%172".to_string(), AgentState::Active)]);

        let err = resolve_send_target(&task, &panes, &states).expect_err("busy agent must block");

        assert!(err.to_string().contains("busy"), "got: {err}");
    }

    /// A task whose window is gone must fail loudly, never silently target the
    /// active pane.
    #[test]
    fn send_errors_when_task_pane_is_gone() {
        let task = task_with(Some("%70"), Some("0:grove-review-ao"));
        let panes = [pane("%180", "/home/user/tasks/glance-ship")];
        let states = HashMap::from([("%180".to_string(), AgentState::Waiting)]);

        let err = resolve_send_target(&task, &panes, &states).expect_err("dead task must error");

        assert!(err.to_string().contains("no longer exists"), "got: {err}");
    }
}
