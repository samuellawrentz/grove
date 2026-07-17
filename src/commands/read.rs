//! Read what an agent *said*, from its transcript.
//!
//! Critically, this reads the Claude JSONL transcript on disk, not herdr's
//! screen. The transcript is resolved by convention from the task's PRIMARY
//! worktree cwd (the first repo), independent of any live pane.

use crate::commands::Ctx;
use crate::db::TaskEntry;
use crate::error::GroveError;
use crate::output;
use crate::transcript::{self, Excerpt, ReadOpts};

/// Find and read a task's transcript. Shared by `grove read` and `grove run`.
pub fn excerpt_for(task: &TaskEntry, opts: &ReadOpts, _ctx: &Ctx) -> Result<Excerpt, GroveError> {
    let cwd = task
        .repos
        .first()
        .map(|r| r.worktree_path.clone())
        .unwrap_or_else(|| task.path.clone());

    let path = transcript::resolve(None, &cwd).ok_or_else(|| {
        GroveError::General(format!(
            "no transcript found for task '{}' (looked under ~/.claude/projects/ for {})",
            task.id,
            cwd.display()
        ))
    })?;

    transcript::read_excerpt(&path, opts)
}

pub fn run(
    task_id: &str,
    turns: usize,
    tools: bool,
    full: bool,
    max_chars: usize,
    ctx: &Ctx,
) -> Result<(), GroveError> {
    let task = ctx
        .db
        .get_task(task_id)?
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?;

    let opts = ReadOpts {
        turns,
        tools,
        full,
        max_chars,
    };
    let excerpt = excerpt_for(&task, &opts, ctx)?;

    let data = serde_json::json!({
        "task_id": task_id,
        "transcript": excerpt.source,
        "truncated": excerpt.truncated,
        "response": excerpt.text,
    });
    output::success(ctx.json_mode, &excerpt.text, data);

    Ok(())
}
