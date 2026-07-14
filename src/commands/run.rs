//! `send` + `wait` + `read`, fused into one call.
//!
//! This is the command an orchestrator should reach for. The three primitives
//! exist because a human driving grove by hand wants them separately, but an
//! agent delegating work wants one blocking call that hands back the answer:
//! three tool-call round trips collapse to one, and the only thing that enters
//! the orchestrator's context is the sub-agent's final message.

use std::time::{Duration, Instant};

use crate::commands::{read, send, wait, Ctx};
use crate::error::GroveError;
use crate::output;
use crate::transcript::ReadOpts;

/// How long to allow for the agent to pick up the keystrokes and start its turn
/// before concluding that an idle state means "finished" rather than "not yet
/// started". See `wait::is_settled`.
const SETTLE: Duration = Duration::from_secs(15);

#[allow(clippy::too_many_arguments)]
pub fn run(
    task_id: &str,
    prompt: &str,
    brief: bool,
    timeout_secs: u64,
    max_chars: usize,
    tools: bool,
    ctx: &Ctx,
) -> Result<(), GroveError> {
    let start = Instant::now();

    let (task, pane_id) = send::deliver(task_id, prompt, brief, ctx)?;

    let settled = wait::wait_for(
        std::slice::from_ref(&task.id),
        false,
        Duration::from_secs(timeout_secs),
        SETTLE,
        ctx,
    )?;
    let state = settled
        .first()
        .map(|s| s.state.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let opts = ReadOpts {
        turns: 1,
        tools,
        full: false,
        max_chars,
    };
    let excerpt = read::excerpt_for(&task, &opts, ctx)?;

    let data = serde_json::json!({
        "task_id": task_id,
        "pane_id": pane_id,
        "state": state,
        "duration_secs": start.elapsed().as_secs(),
        "truncated": excerpt.truncated,
        "response": excerpt.text,
    });
    output::success(ctx.json_mode, &excerpt.text, data);

    Ok(())
}
