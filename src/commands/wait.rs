//! Block until an agent finishes its turn.
//!
//! The token-efficiency argument for this command: an orchestrator that polls
//! `grove status` in a loop pays a full tool-call round trip every few seconds
//! for as long as the task runs. Blocking here costs exactly one round trip no
//! matter how long the agent works. `--any` extends that to a fleet.
//!
//! Turn-completion is delegated to `herdr agent wait --status idle`: herdr owns
//! agent state, so grove just resolves each task's pane by cwd and blocks on it.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::commands::Ctx;
use crate::db::TaskEntry;
use crate::error::GroveError;
use crate::herdr;
use crate::output;

/// A task that has stopped working, and what it stopped as.
#[derive(Debug, Clone)]
pub struct Settled {
    pub task_id: String,
    pub state: String,
    pub pane_id: Option<String>,
}

/// The task's primary worktree cwd — the first repo. See `send::primary_worktree`.
fn primary_worktree(task: &TaskEntry) -> Result<&std::path::Path, GroveError> {
    task.repos
        .first()
        .map(|r| r.worktree_path.as_path())
        .ok_or_else(|| GroveError::General(format!("task '{}' has no repos", task.id)))
}

/// Block until the given tasks settle. Shared by `grove wait` and `grove run`.
///
/// Each task's primary pane is resolved statelessly by cwd, then handed to
/// `herdr agent wait --status idle`. A task with no live agent is treated as
/// already settled (nothing to wait for). Multi-repo tasks wait only on the
/// primary repo's pane.
pub fn wait_for(
    task_ids: &[String],
    any: bool,
    timeout: Duration,
    ctx: &Ctx,
) -> Result<Vec<Settled>, GroveError> {
    let tasks: Vec<TaskEntry> = task_ids
        .iter()
        .map(|id| {
            ctx.db
                .get_task(id)?
                .ok_or_else(|| GroveError::TaskNotFound(id.clone()))
        })
        .collect::<Result<_, _>>()?;

    let timeout_ms = timeout.as_millis() as u64;

    let mut settled: Vec<Settled> = Vec::new();
    let (tx, rx) = mpsc::channel::<(String, String, Result<(), GroveError>)>();
    let mut pending = 0usize;

    for task in &tasks {
        let worktree = primary_worktree(task)?;
        match herdr::resolve_agent_for_cwd(worktree)? {
            Some(agent) => {
                pending += 1;
                let tx = tx.clone();
                let task_id = task.id.clone();
                let pane_id = agent.pane_id.clone();
                thread::spawn(move || {
                    let res = herdr::wait(&pane_id, "idle", timeout_ms);
                    let _ = tx.send((task_id, pane_id, res));
                });
            }
            // No live agent: nothing to wait for.
            None => settled.push(Settled {
                task_id: task.id.clone(),
                state: "unknown".to_string(),
                pane_id: None,
            }),
        }
    }
    drop(tx);

    let mut timed_out: Vec<String> = Vec::new();
    for _ in 0..pending {
        let (task_id, pane_id, res) = rx
            .recv()
            .map_err(|e| GroveError::General(format!("wait channel closed: {e}")))?;
        match res {
            Ok(()) => {
                settled.push(Settled {
                    task_id,
                    state: "idle".to_string(),
                    pane_id: Some(pane_id),
                });
                if any {
                    return Ok(settled);
                }
            }
            Err(GroveError::Timeout(_)) => timed_out.push(task_id),
            Err(e) => return Err(e),
        }
    }

    // `--any` only fails when nothing at all settled; wait-all fails if any task
    // timed out.
    let should_fail = if any {
        settled.is_empty()
    } else {
        !timed_out.is_empty()
    };
    if should_fail {
        return Err(GroveError::Timeout(format!(
            "still working after {}s: {}",
            timeout.as_secs(),
            timed_out.join(", ")
        )));
    }

    Ok(settled)
}

pub fn run(task_ids: &[String], any: bool, timeout_secs: u64, ctx: &Ctx) -> Result<(), GroveError> {
    let start = Instant::now();
    let settled = wait_for(task_ids, any, Duration::from_secs(timeout_secs), ctx)?;

    let waited = start.elapsed().as_secs();
    let human = settled
        .iter()
        .map(|s| format!("{} is {}", s.task_id, s.state))
        .collect::<Vec<_>>()
        .join("\n");

    let data = serde_json::json!({
        "waited_secs": waited,
        "tasks": settled.iter().map(|s| serde_json::json!({
            "task_id": s.task_id,
            "state": s.state,
            "pane_id": s.pane_id,
        })).collect::<Vec<_>>(),
    });
    output::success(ctx.json_mode, &human, data);

    Ok(())
}
