use crate::agent;
use crate::commands::Ctx;
use crate::db::TaskEntry;
use crate::error::GroveError;
use crate::output;
use crate::tmux::{self, PaneInfo};

/// Locate the pane to switch to, or explain why the task cannot be attached.
fn resolve_attach_target<'a>(
    task: &TaskEntry,
    panes: &'a [PaneInfo],
) -> Result<&'a PaneInfo, GroveError> {
    if task.tmux_window.is_none() {
        return Err(GroveError::TmuxNotRunning(format!(
            "task '{}' was created without tmux. Re-create with tmux to enable attach.",
            task.id
        )));
    }

    agent::locate_task_pane(task, panes).ok_or_else(|| {
        GroveError::TmuxNotRunning(format!(
            "tmux window for task '{}' no longer exists. It may have been killed externally.",
            task.id
        ))
    })
}

/// Address a window by `session:index`. The index is stable; the *name* is not —
/// tmux rewrites it whenever a program in the pane emits a title escape, and a
/// recorded `session:grove-<task>` target then matches nothing.
fn window_target(pane: &PaneInfo) -> String {
    format!("{}:{}", pane.session_name, pane.window_index)
}

pub fn run(task_id: &str, ctx: &Ctx) -> Result<(), GroveError> {
    let db = ctx.db;
    let json_mode = ctx.json_mode;
    let verbose = ctx.verbose;

    let task = db
        .get_task(task_id)?
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?;

    let panes = tmux::list_all_panes(verbose).unwrap_or_default();
    let pane = resolve_attach_target(&task, &panes)?;
    let target = window_target(pane);

    tmux::select_window(&target, verbose)?;

    db.heal_pane_id(task_id, &pane.pane_id)?;

    let data = serde_json::json!({
        "task_id": task_id,
        "tmux_window": target,
        "pane_id": pane.pane_id,
    });
    output::success(json_mode, &format!("Switched to task '{task_id}'"), data);

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn task_with(pane_id: Option<&str>, window: Option<&str>) -> TaskEntry {
        TaskEntry {
            id: "review-gate".to_string(),
            path: PathBuf::from("/home/user/tasks/review-gate"),
            repos: Vec::new(),
            created_at: chrono::Utc::now(),
            tmux_window: window.map(str::to_string),
            pane_id: pane_id.map(str::to_string),
        }
    }

    fn pane(pane_id: &str, window_index: u32, window_name: &str, path: &str) -> PaneInfo {
        PaneInfo {
            pane_id: pane_id.to_string(),
            session_name: "0".to_string(),
            window_index,
            window_name: window_name.to_string(),
            current_path: PathBuf::from(path),
            current_command: "zsh".to_string(),
            start_command: "zsh".to_string(),
            pid: 1,
            activity: 0,
        }
    }

    /// Attaching to a task whose window was killed must fail, not silently switch
    /// to whatever window happens to be active.
    #[test]
    fn attach_errors_when_task_pane_is_gone() {
        let task = task_with(Some("%70"), Some("0:grove-review-ao"));
        let panes = [pane("%180", 2, "2.1.208", "/home/user/tasks/glance-ship")];

        let err = resolve_attach_target(&task, &panes).expect_err("dead task must error");

        assert!(err.to_string().contains("no longer exists"), "got: {err}");
    }

    /// After tmux auto-renames the window, the recorded `session:grove-*` target is
    /// dead but the pane is alive — attach must resolve it and address the window
    /// by its index.
    #[test]
    fn attach_targets_window_by_index_after_rename() {
        let task = task_with(Some("%172"), Some("0:grove-review-gate"));
        let panes = [pane("%172", 5, "2.1.207", "/home/user/tasks/review-gate")];

        let found = resolve_attach_target(&task, &panes).expect("live pane should attach");

        assert_eq!(window_target(found), "0:5");
    }
}
