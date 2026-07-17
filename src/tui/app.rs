use std::time::Duration;

use crate::config::GroveConfig;
use crate::db::{Db, Project, TaskEntry};
use crate::error::GroveError;
use crate::herdr;

/// Poll cadence for redraw when idle.
pub(crate) const POLL: Duration = Duration::from_secs(5);

/// Current input-capture overlay. Mutually exclusive by construction.
pub(crate) enum Overlay {
    None,
    /// Incremental search over the recents list.
    Search(String),
    /// A directory was picked (Enter or `o` → fzf); choosing what to run in it.
    OpenChoice { dir: String },
}

/// A one-shot deferred shell side-effect run after the next draw, in a suspended
/// terminal (raw mode off, alternate screen left).
pub(crate) enum PendingShell {
    None,
    /// fzf directory picker → opens the `OpenChoice` overlay.
    FzfPicker,
    /// Interactive `grove init -i`, then build the task's herdr workspace.
    NewTask,
}

/// Recents launcher state. One pane: the recents (Projects) list.
pub(crate) struct App {
    pub recents: Vec<Project>,
    pub cursor: usize,
    pub search: Option<String>,
    pub overlay: Overlay,
    pub pending_shell: PendingShell,
    pub status_message: Option<String>,
    pub should_quit: bool,
    pub popup: bool,
    pub config: GroveConfig,
    pub db: Db,
}

impl App {
    pub(crate) fn new(config: GroveConfig, db: Db, popup: bool) -> Self {
        let mut app = App {
            recents: Vec::new(),
            cursor: 0,
            search: None,
            overlay: Overlay::None,
            pending_shell: PendingShell::None,
            status_message: None,
            should_quit: false,
            popup,
            config,
            db,
        };
        app.load_recents();
        app
    }

    pub(crate) fn poll_timeout(&self) -> Duration {
        POLL
    }

    /// The configured Claude launch command (default:
    /// `claude --dangerously-skip-permissions`).
    pub(crate) fn claude_cmd(&self) -> String {
        self.config.claude_command.clone()
    }

    pub(crate) fn overlay_active(&self) -> bool {
        !matches!(self.overlay, Overlay::None)
    }

    /// Load the recents list (the projects table). Errors are surfaced as a
    /// status message and leave the previous list intact.
    pub(crate) fn load_recents(&mut self) {
        match self.db.list_projects() {
            Ok(list) => {
                self.recents = list;
                if self.cursor >= self.recents.len() {
                    self.cursor = self.recents.len().saturating_sub(1);
                }
            }
            Err(e) => self.status_message = Some(format!("Recents load error: {e}")),
        }
    }

    /// Indices into `recents` that match the current search filter.
    pub(crate) fn filtered_indices(&self) -> Vec<usize> {
        match &self.search {
            Some(query) if !query.is_empty() => {
                let q = query.to_lowercase();
                self.recents
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| {
                        p.name.to_lowercase().contains(&q)
                            || p.path.to_string_lossy().to_lowercase().contains(&q)
                    })
                    .map(|(i, _)| i)
                    .collect()
            }
            _ => (0..self.recents.len()).collect(),
        }
    }

    /// The `recents` index currently under the cursor, honoring the filter.
    pub(crate) fn selected_recent(&self) -> Option<usize> {
        let idxs = self.filtered_indices();
        if idxs.is_empty() {
            None
        } else {
            Some(idxs[self.cursor.min(idxs.len() - 1)])
        }
    }

    /// The selected recent's directory as a String.
    pub(crate) fn selected_dir(&self) -> Option<String> {
        self.selected_recent()
            .map(|i| self.recents[i].path.to_string_lossy().to_string())
    }

    /// Create a focused herdr workspace at `dir` running `cmd`; quit in popup
    /// mode. Shared tail of the recents-Enter and `o`-picker actions.
    pub(crate) fn launch_workspace(&mut self, dir: &str, cmd: Option<&str>) {
        let label = basename(dir);
        match herdr::launch_workspace(dir, &label, cmd) {
            Ok(_) => self.should_quit = self.popup,
            Err(e) => self.status_message = Some(format!("launch failed: {e}")),
        }
    }

    /// Build ONE herdr workspace for a freshly-created task: root pane at the
    /// first repo's worktree, one split pane per additional repo, each running
    /// Claude with cwd = that repo's worktree. Focuses the workspace.
    pub(crate) fn launch_task_workspace(&mut self, task: &TaskEntry) -> Result<(), GroveError> {
        let first = task
            .repos
            .first()
            .ok_or_else(|| GroveError::General(format!("task '{}' has no repos", task.id)))?;
        let first_cwd = first.worktree_path.to_string_lossy().to_string();

        let claude = self.claude_cmd();
        let (workspace_id, root_pane) = herdr::create_workspace(&first_cwd, &task.id)?;
        herdr::run_in_pane(&root_pane, &claude)?;

        for repo in task.repos.iter().skip(1) {
            let cwd = repo.worktree_path.to_string_lossy().to_string();
            let pane = herdr::split_pane(&root_pane, &cwd)?;
            herdr::run_in_pane(&pane, &claude)?;
        }

        herdr::focus_workspace(&workspace_id)?;
        Ok(())
    }
}

/// Last path component, or the whole string if there is none.
pub(crate) fn basename(dir: &str) -> String {
    std::path::Path::new(dir)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(dir)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> Db {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_path_buf();
        std::mem::forget(f);
        Db::open_path(&path).unwrap()
    }

    fn test_app() -> App {
        App::new(GroveConfig::default(), temp_db(), false)
    }

    #[test]
    fn basename_of_path() {
        assert_eq!(basename("/home/user/tasks/foo"), "foo");
        assert_eq!(basename("bar"), "bar");
    }

    #[test]
    fn empty_recents_has_no_selection() {
        let app = test_app();
        assert!(app.filtered_indices().is_empty());
        assert!(app.selected_recent().is_none());
        assert!(app.selected_dir().is_none());
    }

    #[test]
    fn search_filters_recents() {
        let mut app = test_app();
        app.db.upsert_project("/home/user/alpha").unwrap();
        app.db.upsert_project("/home/user/beta").unwrap();
        app.load_recents();
        assert_eq!(app.filtered_indices().len(), 2);
        app.search = Some("alph".to_string());
        assert_eq!(app.filtered_indices().len(), 1);
    }
}
