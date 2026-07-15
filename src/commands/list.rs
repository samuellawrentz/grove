use crate::agent;
use crate::commands::Ctx;
use crate::error::GroveError;
use crate::output;
use crate::tmux;

pub fn run(ctx: &Ctx) -> Result<(), GroveError> {
    let db = ctx.db;
    let json_mode = ctx.json_mode;
    let verbose = ctx.verbose;

    let tasks = db.list_tasks()?;
    if tasks.is_empty() {
        let data = serde_json::json!({ "tasks": [] });
        output::success(json_mode, "No active tasks", data);
        return Ok(());
    }

    let agent_states = agent::read_state_file().unwrap_or_default();
    let panes = tmux::list_all_panes(verbose).unwrap_or_default();

    if json_mode {
        let task_list: Vec<serde_json::Value> = tasks
            .iter()
            .map(|t| {
                let exists = !t.is_stale();
                let repo_names: Vec<&str> = t.repos.iter().map(|r| r.repo_name.as_str()).collect();
                let branch = t.repos.first().map(|r| r.branch.as_str()).unwrap_or("");

                let live = agent::resolve_task_state(t, &panes, &agent_states);
                let (tmux_alive, agent_state) = (live.alive(), live.agent_state);

                serde_json::json!({
                    "task_id": t.id,
                    "path": t.path,
                    "repos": repo_names,
                    "repo_count": t.repos.len(),
                    "branch": branch,
                    "created_at": t.created_at,
                    "exists": exists,
                    "tmux_window": t.tmux_window,
                    "pane_id": live.pane_id,
                    "tmux_alive": tmux_alive,
                    "claude_state": agent_state.to_string(),
                    "agent_state": agent_state.to_string(),
                })
            })
            .collect();
        let data = serde_json::json!({ "tasks": task_list });
        output::success(true, "", data);
    } else {
        println!(
            "{:<20} {:<6} {:<30} {:<20} {:<12} {:<10}",
            "TASK", "REPOS", "REPO NAMES", "TMUX", "AGENT", "STATUS"
        );
        for t in &tasks {
            let stale = t.is_stale();
            let repo_names: Vec<&str> = t.repos.iter().map(|r| r.repo_name.as_str()).collect();

            let live = agent::resolve_task_state(t, &panes, &agent_states);
            let (tmux_alive, agent_state) = (live.alive(), live.agent_state);

            let tmux_str = match &t.tmux_window {
                None => "(none)".to_string(),
                Some(w) => {
                    let name = w.split(':').nth(1).unwrap_or(w);
                    if tmux_alive {
                        name.to_string()
                    } else {
                        format!("{name} [dead]")
                    }
                }
            };

            let agent_str = match &t.tmux_window {
                Some(_) => agent_state.to_string(),
                None => "—".to_string(),
            };

            let status = if stale { "STALE" } else { "ok" };

            println!(
                "{:<20} {:<6} {:<30} {:<20} {:<12} {:<10}",
                t.id,
                t.repos.len(),
                repo_names.join(", "),
                tmux_str,
                agent_str,
                status
            );
        }
    }

    Ok(())
}
