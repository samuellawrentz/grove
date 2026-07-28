use crate::commands::Ctx;
use crate::db::TaskEntry;
use crate::error::GroveError;
use crate::herdr::{self, AgentInfo};
use crate::output;

/// The live herdr agent for a task's primary repo, resolved by cwd.
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

pub fn run(ctx: &Ctx) -> Result<(), GroveError> {
    let db = ctx.db;
    let json_mode = ctx.json_mode;

    let tasks = db.list_tasks()?;
    if tasks.is_empty() {
        let data = serde_json::json!({ "tasks": [] });
        output::success(json_mode, "No active tasks", data);
        return Ok(());
    }

    // herdr owns live agent state; match by cwd.
    let agents = herdr::agents().unwrap_or_default();

    if json_mode {
        let task_list: Vec<serde_json::Value> = tasks
            .iter()
            .map(|t| {
                let exists = !t.is_stale();
                let repo_names: Vec<&str> = t.repos.iter().map(|r| r.repo_name.as_str()).collect();
                let dirs: Vec<&str> = t.repos.iter().map(|r| r.dir_name()).collect();
                let branch = t.repos.first().map(|r| r.branch.as_str()).unwrap_or("");
                let (pane_id, agent_status) = live_agent(t, &agents);

                serde_json::json!({
                    "task_id": t.id,
                    "path": t.path,
                    "repos": repo_names,
                    "dirs": dirs,
                    "repo_count": t.repos.len(),
                    "branch": branch,
                    "created_at": t.created_at,
                    "exists": exists,
                    "pane_id": pane_id,
                    "agent_status": agent_status,
                })
            })
            .collect();
        let data = serde_json::json!({ "tasks": task_list });
        output::success(true, "", data);
    } else {
        println!(
            "{:<20} {:<6} {:<30} {:<12} {:<10}",
            "TASK", "REPOS", "REPO NAMES", "AGENT", "STATUS"
        );
        for t in &tasks {
            let stale = t.is_stale();
            let repo_names: Vec<String> = t.repos.iter().map(|r| r.display_name()).collect();
            let (_pane_id, agent_status) = live_agent(t, &agents);
            let status = if stale { "STALE" } else { "ok" };

            println!(
                "{:<20} {:<6} {:<30} {:<12} {:<10}",
                t.id,
                t.repos.len(),
                repo_names.join(", "),
                agent_status,
                status
            );
        }
    }

    Ok(())
}
