use crate::agent;
use crate::error::GroveError;
use crate::output;
use crate::state::GroveState;

pub fn run(
    task_id: Option<&str>,
    state: &GroveState,
    json_mode: bool,
    verbose: bool,
) -> Result<(), GroveError> {
    // If specific task requested, verify it exists
    if let Some(id) = task_id {
        if !state.tasks.contains_key(id) {
            return Err(GroveError::TaskNotFound(id.to_string()));
        }
    }

    let agent_states = agent::read_state_file().unwrap_or_default();

    let mut tasks: Vec<_> = state.tasks.values().collect();
    tasks.sort_by(|a, b| a.id.cmp(&b.id));

    // Filter to specific task if requested
    if let Some(id) = task_id {
        tasks.retain(|t| t.id == id);
    }

    if json_mode {
        let task_list: Vec<serde_json::Value> = tasks
            .iter()
            .map(|t| {
                let (tmux_alive, live_agent_state) =
                    agent::resolve_task_state(t, &agent_states, verbose);
                let repo_names: Vec<&str> = t.repos.iter().map(|r| r.repo_name.as_str()).collect();

                serde_json::json!({
                    "task_id": t.id,
                    "path": t.path,
                    "repos": repo_names,
                    "branch": t.repos.first().map(|r| r.branch.as_str()).unwrap_or(""),
                    "tmux_window": t.tmux_window,
                    "pane_id": t.pane_id,
                    "tmux_alive": tmux_alive,
                    "claude_state": live_agent_state.to_string(),
                    "agent_state": live_agent_state.to_string(),
                    "created_at": t.created_at,
                })
            })
            .collect();
        let data = serde_json::json!({ "tasks": task_list });
        output::success(true, "", data);
    } else {
        if tasks.is_empty() {
            println!("No active tasks");
            return Ok(());
        }

        for t in &tasks {
            let (tmux_alive, live_agent_state) =
                agent::resolve_task_state(t, &agent_states, verbose);
            let repo_names: Vec<&str> = t.repos.iter().map(|r| r.repo_name.as_str()).collect();
            let branch = t.repos.first().map(|r| r.branch.as_str()).unwrap_or("");

            let tmux_status = match &t.tmux_window {
                None => "(no tmux)".to_string(),
                Some(w) if tmux_alive => w.clone(),
                Some(w) => format!("{w} [dead]"),
            };

            let pane_str = t.pane_id.as_deref().unwrap_or("-");

            println!(
                "Task: {}  Window: {}  Pane: {}  Agent: {}",
                t.id, tmux_status, pane_str, live_agent_state
            );
            println!("  Repos: {}  Branch: {}", repo_names.join(", "), branch);
            println!("  Path: {}", t.path.display());
            println!();
        }
    }

    Ok(())
}
