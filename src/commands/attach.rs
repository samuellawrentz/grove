use crate::commands::Ctx;
use crate::error::GroveError;
use crate::herdr;
use crate::output;

pub fn run(task_id: &str, ctx: &Ctx) -> Result<(), GroveError> {
    let db = ctx.db;
    let json_mode = ctx.json_mode;

    let task = db
        .get_task(task_id)?
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?;

    let worktree = task
        .repos
        .first()
        .map(|r| r.worktree_path.as_path())
        .ok_or_else(|| GroveError::General(format!("task '{task_id}' has no repos")))?;

    let agent = herdr::resolve_agent_for_cwd(worktree)?.ok_or_else(|| {
        GroveError::General(format!(
            "no herdr agent found for task '{task_id}' (looked for a pane whose cwd is {})",
            worktree.display()
        ))
    })?;

    let workspace_id = agent.workspace_id.clone().ok_or_else(|| {
        GroveError::General(format!(
            "herdr agent for task '{task_id}' has no workspace_id to focus"
        ))
    })?;

    herdr::focus_workspace(&workspace_id)?;

    let data = serde_json::json!({
        "task_id": task_id,
        "workspace_id": workspace_id,
        "pane_id": agent.pane_id,
    });
    output::success(json_mode, &format!("Focused task '{task_id}'"), data);

    Ok(())
}
