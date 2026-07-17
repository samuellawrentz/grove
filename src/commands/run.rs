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

    // Confirm the turn actually started before waiting for it to finish. The
    // agent is `idle` at the instant we send; if we jump straight to
    // `wait --status idle` it returns on that stale idle and we read the
    // PREVIOUS turn. Block until the agent flips to `working` (bounded), then
    // wait for it to settle. A timeout here is fine — a trivial/no-op turn may
    // never be observed as `working`.
    if let Err(e) = crate::herdr::wait(&pane_id, "working", 15_000) {
        if !matches!(e, GroveError::Timeout(_)) {
            return Err(e);
        }
    }

    let settled = wait::wait_for(
        std::slice::from_ref(&task.id),
        false,
        Duration::from_secs(timeout_secs),
        ctx,
    )?;
    let state = settled
        .first()
        .map(|s| s.state.clone())
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
