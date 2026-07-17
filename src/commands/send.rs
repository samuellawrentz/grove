use crate::commands::Ctx;
use crate::db::TaskEntry;
use crate::error::GroveError;
use crate::herdr;
use crate::output;

/// Appended by `--brief`. An orchestrator reads only the agent's final message
/// (see `grove read`), so asking for that message to *be* the report is the
/// cheapest possible summarization: it costs the orchestrator nothing and saves
/// it from pulling the whole transcript to work out what happened.
const BRIEF_SUFFIX: &str = "\n\nWhen you are done, end your final message with a summary of at \
most 5 lines: what changed, which files, what you verified, and any blockers. \
No preamble.";

/// The task's primary worktree — the first repo. That's the pane a prompt goes
/// to and the cwd `grove read` reads its transcript from.
fn primary_worktree(task: &TaskEntry) -> Result<&std::path::Path, GroveError> {
    task.repos
        .first()
        .map(|r| r.worktree_path.as_path())
        .ok_or_else(|| GroveError::General(format!("task '{}' has no repos", task.id)))
}

/// Deliver a prompt to a task's primary pane and return that pane id.
/// Shared by `grove send` and `grove run` — the guard rails belong to both.
///
/// Resolution is stateless: match the first repo's worktree cwd against a live
/// herdr agent. The busy-guard blocks only while that agent is `working`;
/// sending keystrokes mid-turn would interleave with its work.
pub fn deliver(
    task_id: &str,
    prompt: &str,
    brief: bool,
    ctx: &Ctx,
) -> Result<(TaskEntry, String), GroveError> {
    let task = ctx
        .db
        .get_task(task_id)?
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?;

    let worktree = primary_worktree(&task)?;
    let agent = herdr::resolve_agent_for_cwd(worktree)?.ok_or_else(|| {
        GroveError::General(format!(
            "no herdr agent found for task '{}' (looked for a pane whose cwd is {})",
            task.id,
            worktree.display()
        ))
    })?;

    if agent.agent_status == "working" {
        return Err(GroveError::General(format!(
            "Agent is busy working (status: {}); wait for it to finish its turn \
             before sending.",
            agent.agent_status
        )));
    }

    let pane_id = agent.pane_id.clone();

    let text = if brief {
        format!("{prompt}{BRIEF_SUFFIX}")
    } else {
        prompt.to_string()
    };

    herdr::send(&pane_id, &text)?;

    Ok((task, pane_id))
}

pub fn run(task_id: &str, prompt: &str, brief: bool, ctx: &Ctx) -> Result<(), GroveError> {
    let (_task, pane_id) = deliver(task_id, prompt, brief, ctx)?;

    let data = serde_json::json!({
        "task_id": task_id,
        "pane_id": pane_id,
        "prompt_sent": true,
    });
    output::success(
        ctx.json_mode,
        &format!("Sent prompt to task '{task_id}'"),
        data,
    );

    Ok(())
}
