//! Read what an agent *said*, from its transcript.

use crate::agent;
use crate::commands::Ctx;
use crate::db::TaskEntry;
use crate::error::GroveError;
use crate::output;
use crate::transcript::{self, Excerpt, ReadOpts};

/// Find and read a task's transcript. Shared by `grove read` and `grove run`.
pub fn excerpt_for(task: &TaskEntry, opts: &ReadOpts, ctx: &Ctx) -> Result<Excerpt, GroveError> {
    let panes = crate::tmux::list_all_panes(ctx.verbose).unwrap_or_default();
    let snapshots = agent::read_pane_snapshots().unwrap_or_default();

    let snapshot = agent::locate_task_pane(task, &panes).and_then(|p| snapshots.get(&p.pane_id));

    // The hook's recorded path wins; its cwd, then the task dir, are fallbacks
    // for an agent launched before the hook learned to record transcripts.
    let cwd = snapshot
        .and_then(|s| s.cwd.clone())
        .unwrap_or_else(|| task.path.clone());
    let recorded = snapshot.and_then(|s| s.transcript.as_deref());

    let path = transcript::resolve(recorded, &cwd).ok_or_else(|| {
        GroveError::General(format!(
            "no transcript found for task '{}' (looked for a recorded path, then \
             under ~/.claude/projects/ for {}). Is the agent-tmux-status.sh hook wired up?",
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
