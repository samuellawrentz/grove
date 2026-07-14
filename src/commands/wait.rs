//! Block until an agent finishes its turn.
//!
//! The token-efficiency argument for this command: an orchestrator that polls
//! `grove status` in a loop pays a full tool-call round trip — prompt, output,
//! and the model's reasoning about whether to poll again — every few seconds,
//! for as long as the task runs. Blocking here costs exactly one round trip no
//! matter how long the agent works. `--any` extends that to a fleet: supervising
//! N parallel tasks costs ~N calls total rather than N × (poll frequency).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::agent::{self, AgentState};
use crate::commands::Ctx;
use crate::db::TaskEntry;
use crate::error::GroveError;
use crate::output;

const POLL_INTERVAL: Duration = Duration::from_millis(300);

/// A task that has stopped working, and what it stopped as.
#[derive(Debug, Clone)]
pub struct Settled {
    pub task_id: String,
    pub state: AgentState,
    pub pane_id: Option<String>,
}

/// Has this task's turn ended?
///
/// `Idle`/`Waiting` mean the agent stopped and wants input — that is the signal.
/// `NotRunning`/`Unknown` mean there is no live agent at all, which is only
/// *settled* once the settle window has passed: immediately after a `send` the
/// hook has not yet fired `active`, and treating that gap as "finished" would
/// return the previous turn's answer as if it were this one's.
fn is_settled(state: &AgentState, seen_active: bool, within_settle: bool) -> bool {
    match state {
        AgentState::Active => false,
        AgentState::Idle | AgentState::Waiting => seen_active || !within_settle,
        AgentState::NotRunning | AgentState::Unknown => !within_settle,
    }
}

/// Block until the given tasks settle. Shared by `grove wait` and `grove run`.
///
/// `settle` guards the send→active race described in [`is_settled`]; pass zero
/// when the caller has not just sent a prompt.
pub fn wait_for(
    task_ids: &[String],
    any: bool,
    timeout: Duration,
    settle: Duration,
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

    let start = Instant::now();
    let mut seen_active: HashSet<String> = HashSet::new();

    loop {
        let panes = crate::tmux::list_all_panes(ctx.verbose).unwrap_or_default();
        let states = agent::read_state_file().unwrap_or_default();
        let within_settle = start.elapsed() < settle;

        let mut settled = Vec::new();
        for task in &tasks {
            let live = agent::resolve_task_state(task, &panes, &states);
            if live.agent_state == AgentState::Active {
                seen_active.insert(task.id.clone());
            }
            if is_settled(
                &live.agent_state,
                seen_active.contains(&task.id),
                within_settle,
            ) {
                settled.push(Settled {
                    task_id: task.id.clone(),
                    state: live.agent_state,
                    pane_id: live.pane_id,
                });
            }
        }

        if (any && !settled.is_empty()) || settled.len() == tasks.len() {
            return Ok(settled);
        }

        if start.elapsed() >= timeout {
            let pending: Vec<&str> = tasks
                .iter()
                .map(|t| t.id.as_str())
                .filter(|id| !settled.iter().any(|s| s.task_id == *id))
                .collect();
            return Err(GroveError::Timeout(format!(
                "still working after {}s: {}",
                timeout.as_secs(),
                pending.join(", ")
            )));
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}

pub fn run(task_ids: &[String], any: bool, timeout_secs: u64, ctx: &Ctx) -> Result<(), GroveError> {
    let start = Instant::now();
    let settled = wait_for(
        task_ids,
        any,
        Duration::from_secs(timeout_secs),
        Duration::ZERO,
        ctx,
    )?;

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
            "state": s.state.to_string(),
            "pane_id": s.pane_id,
        })).collect::<Vec<_>>(),
    });
    output::success(ctx.json_mode, &human, data);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The race this guard exists for: `send` returns, the agent has not picked
    /// up the keystrokes yet, and the state file still says `idle` from the
    /// previous turn. Settling there would hand back the *old* answer.
    #[test]
    fn stale_idle_is_not_settled_during_the_settle_window() {
        assert!(!is_settled(&AgentState::Idle, false, true));
        assert!(!is_settled(&AgentState::NotRunning, false, true));
    }

    /// Once the turn has demonstrably started, going idle is the finish line —
    /// no need to burn the rest of the settle window.
    #[test]
    fn idle_after_an_observed_active_turn_is_settled() {
        assert!(is_settled(&AgentState::Idle, true, true));
        assert!(is_settled(&AgentState::Waiting, true, true));
    }

    /// An agent that never starts (crashed launch, wrong pane) must not hang the
    /// caller until timeout — past the settle window, absence is an answer.
    #[test]
    fn absent_agent_settles_once_the_settle_window_lapses() {
        assert!(is_settled(&AgentState::NotRunning, false, false));
        assert!(is_settled(&AgentState::Unknown, false, false));
    }

    #[test]
    fn an_actively_working_agent_is_never_settled() {
        assert!(!is_settled(&AgentState::Active, true, false));
        assert!(!is_settled(&AgentState::Active, false, false));
    }
}
