use crate::claude;
use crate::error::GroveError;
use crate::output;
use crate::state::GroveState;
use crate::tmux;

pub fn run(
    task_id: &str,
    prompt: &str,
    state: &GroveState,
    json_mode: bool,
    verbose: bool,
) -> Result<(), GroveError> {
    let task = state
        .tasks
        .get(task_id)
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?;

    let target = task.tmux_window.as_deref().ok_or_else(|| {
        GroveError::TmuxNotRunning(format!("task '{task_id}' was created without tmux"))
    })?;

    // Re-query live pane ID (handles respawns)
    let live_pane_id = tmux::get_pane_id(target, verbose).map_err(|_| {
        GroveError::TmuxNotRunning(format!("tmux window for task '{task_id}' no longer exists"))
    })?;

    // Check Claude state — read file once, look up pane
    let claude_states = claude::read_state_file().unwrap_or_default();
    let claude_state = claude_states
        .get(&live_pane_id)
        .cloned()
        .unwrap_or(claude::ClaudeState::NotRunning);
    if claude_state != claude::ClaudeState::Waiting {
        return Err(GroveError::General(format!(
            "Claude is not waiting for input (current state: {claude_state}). \
             Ensure claude-tmux-status.sh hook is running if state seems wrong."
        )));
    }

    tmux::send_keys(target, prompt, verbose)?;

    let data = serde_json::json!({
        "task_id": task_id,
        "tmux_window": target,
        "pane_id": live_pane_id,
        "prompt_sent": true,
    });
    output::success(json_mode, &format!("Sent prompt to task '{task_id}'"), data);

    Ok(())
}
