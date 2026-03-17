use crate::error::GroveError;
use crate::output;
use crate::state::GroveState;

pub fn run(state: &GroveState, json_mode: bool) -> Result<(), GroveError> {
    if state.tasks.is_empty() {
        let data = serde_json::json!({ "tasks": [] });
        output::success(json_mode, "No active tasks", data);
        return Ok(());
    }

    let mut tasks: Vec<_> = state.tasks.values().collect();
    tasks.sort_by(|a, b| a.id.cmp(&b.id));

    if json_mode {
        let task_list: Vec<serde_json::Value> = tasks
            .iter()
            .map(|t| {
                let exists = t.path.exists();
                let repo_names: Vec<&str> = t.repos.iter().map(|r| r.repo_name.as_str()).collect();
                serde_json::json!({
                    "task_id": t.id,
                    "path": t.path,
                    "repos": repo_names,
                    "repo_count": t.repos.len(),
                    "created_at": t.created_at,
                    "exists": exists,
                })
            })
            .collect();
        let data = serde_json::json!({ "tasks": task_list });
        output::success(true, "", data);
    } else {
        println!(
            "{:<20} {:<6} {:<30} {:<20} STATUS",
            "TASK", "REPOS", "REPO NAMES", "CREATED"
        );
        for t in &tasks {
            let status = if t.path.exists() { "ok" } else { "STALE" };
            let repo_names: Vec<&str> = t.repos.iter().map(|r| r.repo_name.as_str()).collect();
            let created = t.created_at.format("%Y-%m-%d %H:%M").to_string();
            println!(
                "{:<20} {:<6} {:<30} {:<20} {}",
                t.id,
                t.repos.len(),
                repo_names.join(", "),
                created,
                status
            );
        }
    }

    Ok(())
}
