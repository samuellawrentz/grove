use crate::error::GroveError;
use crate::output;
use crate::state::GroveState;
use crate::tmux;

pub fn run(
    task_id: &str,
    state: &GroveState,
    json_mode: bool,
    verbose: bool,
) -> Result<(), GroveError> {
    let task = state
        .tasks
        .get(task_id)
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?;

    let target = task.tmux_window.as_deref().ok_or_else(|| {
        GroveError::TmuxNotRunning(format!(
            "task '{task_id}' was created without tmux. Re-create with tmux to enable attach."
        ))
    })?;

    // Verify window still exists by trying to get its pane ID
    tmux::get_pane_id(target, verbose).map_err(|_| {
        GroveError::TmuxNotRunning(format!(
            "tmux window for task '{task_id}' no longer exists. It may have been killed externally."
        ))
    })?;

    tmux::select_window(target, verbose)?;

    let data = serde_json::json!({
        "task_id": task_id,
        "tmux_window": target,
    });
    output::success(json_mode, &format!("Switched to task '{task_id}'"), data);

    Ok(())
}
