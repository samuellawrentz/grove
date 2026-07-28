use crate::commands::Ctx;
use crate::db::TaskEntry;
use crate::error::GroveError;
use crate::herdr::{self, AgentInfo};
use crate::output;

/// The live herdr agent for a task's primary repo, resolved by cwd. Returns the
/// pane id and status ("-"/"unknown" when no live agent matches).
fn live_agent(task: &TaskEntry, agents: &[AgentInfo]) -> (Option<String>, String) {
    let Some(worktree) = task.repos.first().map(|r| &r.worktree_path) else {
        return (None, "unknown".to_string());
    };
    match agents.iter().find(|a| {
        [a.cwd.as_deref(), a.foreground_cwd.as_deref()]
            .into_iter()
            .flatten()
            .any(|d| std::path::Path::new(d) == worktree.as_path())
    }) {
        Some(a) => (Some(a.pane_id.clone()), a.agent_status.clone()),
        None => (None, "unknown".to_string()),
    }
}

pub fn run(task_id: Option<&str>, ctx: &Ctx) -> Result<(), GroveError> {
    let db = ctx.db;
    let json_mode = ctx.json_mode;

    let all_tasks = db.list_tasks()?;

    if let Some(id) = task_id {
        if !all_tasks.iter().any(|t| t.id == id) {
            return Err(GroveError::TaskNotFound(id.to_string()));
        }
    }

    // herdr is the source of truth for live agent state; match by cwd.
    let agents = herdr::agents().unwrap_or_default();

    let tasks: Vec<_> = if let Some(id) = task_id {
        all_tasks.into_iter().filter(|t| t.id == id).collect()
    } else {
        all_tasks
    };

    if json_mode {
        let task_list: Vec<serde_json::Value> = tasks
            .iter()
            .map(|t| {
                let (pane_id, agent_status) = live_agent(t, &agents);
                let repo_names: Vec<&str> = t.repos.iter().map(|r| r.repo_name.as_str()).collect();
                let dirs: Vec<&str> = t.repos.iter().map(|r| r.dir_name()).collect();

                serde_json::json!({
                    "task_id": t.id,
                    "path": t.path,
                    "repos": repo_names,
                    "dirs": dirs,
                    "branch": t.repos.first().map(|r| r.branch.as_str()).unwrap_or(""),
                    "pane_id": pane_id,
                    "agent_status": agent_status,
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
            let (pane_id, agent_status) = live_agent(t, &agents);
            let repo_names: Vec<String> = t.repos.iter().map(|r| r.display_name()).collect();
            let branch = t.repos.first().map(|r| r.branch.as_str()).unwrap_or("");
            let pane_str = pane_id.as_deref().unwrap_or("-");

            println!(
                "Task: {}  Pane: {}  Agent: {}",
                t.id, pane_str, agent_status
            );
            println!("  Repos: {}  Branch: {}", repo_names.join(", "), branch);
            println!("  Path: {}", t.path.display());
            println!();
        }
    }

    Ok(())
}
