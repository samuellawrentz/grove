use crate::agent;
use crate::db::Db;
use crate::error::GroveError;
use crate::output;
use crate::tmux;

pub fn run(
    task_id: &str,
    prompt: &str,
    db: &Db,
    json_mode: bool,
    verbose: bool,
) -> Result<(), GroveError> {
    let task = db
        .get_task(task_id)?
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?;

    let target = task.tmux_window.as_deref().ok_or_else(|| {
        GroveError::TmuxNotRunning(format!("task '{task_id}' was created without tmux"))
    })?;

    let live_pane_id = tmux::get_pane_id(target, verbose).map_err(|_| {
        GroveError::TmuxNotRunning(format!("tmux window for task '{task_id}' no longer exists"))
    })?;

    let agent_states = agent::read_state_file().unwrap_or_default();
    let agent_state = agent_states
        .get(&live_pane_id)
        .cloned()
        .unwrap_or(agent::AgentState::NotRunning);
    // Only block when the agent is mid-turn (Active) — sending keystrokes then
    // would interleave with its work. Every other state (Waiting on a prompt,
    // freshly launched / idle at the prompt with no state-file entry yet) is a
    // legitimate moment to send input.
    if agent_state == agent::AgentState::Active {
        return Err(GroveError::General(format!(
            "Agent is busy working (current state: {agent_state}); wait for it to \
             finish its turn before sending. Ensure agent-tmux-status.sh hook is \
             running if state seems wrong."
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
