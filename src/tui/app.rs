use std::time::{Duration, Instant};

use edtui::{EditorState, Lines};

use crate::agent::AgentFilter;
use crate::config::GroveConfig;
use crate::db::{Db, Project};
use crate::error::GroveError;
use crate::tmux;

use super::source::{self, DiffState};
use super::tree::TreeState;

const TREE_POLL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarFocus {
    Tree,
    Projects,
}

/// Which panel has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Sidebar,
    Notepad,
}

/// Current input-capture overlay. Mutually exclusive by construction.
pub(crate) enum Overlay {
    None,
    Search { query: String, target: SidebarFocus },
    Prompt(String),
    OpenChoice { dir: String },
}

/// A one-shot deferred shell side-effect to run after the next draw.
pub(crate) enum PendingShell {
    None,
    Popup(String),
    FzfPicker,
}

/// Main application state for the TUI.
pub(crate) struct App {
    pub tree: TreeState,
    pub preview_content: String,
    pub last_interaction: Instant,
    pub should_quit: bool,
    pub verbose: bool,
    pub overlay: Overlay,
    pub status_message: Option<String>,
    pub my_pane_id: String,
    pub pending_shell: PendingShell,
    pub preview_scroll_up: u16,
    pub diff_mode: bool,
    pub diff_state: Option<DiffState>,
    pub default_agent_command: String,
    pub sidebar_focus: SidebarFocus,
    pub focus: Focus,
    pub config: GroveConfig,
    pub db: Db,
    pub projects: Vec<Project>,
    pub projects_cursor: usize,
    pub projects_search_filter: Option<String>,
    /// When true, quit after launching a pane (popup mode).
    pub popup: bool,
    pub show_notepad: bool,
    pub notepad: NoteState,
}

pub(crate) struct NoteState {
    pub editor: EditorState,
    pub project: String,
}

impl App {
    /// Create a new App from the owned config + db, querying the TUI's own pane ID.
    pub fn new(
        config: GroveConfig,
        db: Db,
        verbose: bool,
        popup: bool,
    ) -> Result<Self, GroveError> {
        let my_pane_id = std::env::var("TMUX_PANE").unwrap_or_default();
        let my_pane_id = if my_pane_id.is_empty() {
            tmux::get_pane_id("", verbose).unwrap_or_default()
        } else {
            my_pane_id
        };

        let mut app = Self::with_parts(config, db, my_pane_id, verbose, popup);
        app.projects = app.db.list_projects().unwrap_or_default();
        app.refresh_tree();
        app.tree.jump_first_pane();
        app.refresh_preview();
        app.sync_note_to_group();
        Ok(app)
    }

    /// Construct an App from explicit parts without touching tmux or the
    /// network. The headless seam used by tests (and by `new`, which then
    /// performs the live refresh).
    pub(crate) fn with_parts(
        config: GroveConfig,
        db: Db,
        my_pane_id: String,
        verbose: bool,
        popup: bool,
    ) -> Self {
        let default_agent_command = config.claude_command.clone();
        App {
            tree: TreeState {
                groups: Vec::new(),
                cursor: 0,
                scroll_offset: 0,
                search_filter: None,
                agent_filter: AgentFilter::AnyAgent,
            },
            overlay: Overlay::None,
            preview_content: String::new(),
            last_interaction: Instant::now(),
            should_quit: false,
            verbose,
            status_message: None,
            my_pane_id,
            pending_shell: PendingShell::None,
            preview_scroll_up: 0,
            diff_mode: false,
            diff_state: None,
            default_agent_command,
            sidebar_focus: SidebarFocus::Tree,
            config,
            db,
            projects: Vec::new(),
            projects_cursor: 0,
            projects_search_filter: None,
            popup,
            focus: Focus::Sidebar,
            show_notepad: false,
            notepad: NoteState {
                editor: EditorState::default(),
                project: String::new(),
            },
        }
    }

    /// Whether an input-capture overlay is active. Background refresh
    /// (`on_tick`/SIGUSR1) is gated on this so a tmux hook firing mid-overlay
    /// can't rebuild the tree and yank the cursor out from under the user.
    pub(crate) fn overlay_active(&self) -> bool {
        !matches!(self.overlay, Overlay::None)
    }

    /// Refresh tree data from tmux and claude state.
    pub fn refresh_tree(&mut self) {
        match (
            source::fetch_panes(self.verbose),
            source::fetch_agent_states(),
        ) {
            (Ok(panes), Ok(states)) => {
                // Drop recorded pane_agents rows whose panes are gone, before
                // reading them back, so dead rows can't resurrect a stale agent.
                let live: std::collections::HashSet<String> =
                    panes.iter().map(|p| p.pane_id.clone()).collect();
                if let Err(e) = crate::agent::PaneAgentStore::new(&self.db).gc(&live) {
                    if self.verbose {
                        eprintln!("Warning: pane_agents GC failed: {e}");
                    }
                }
                // DB-recorded kinds are authoritative (set at launch); fall back
                // to kinds the agents declared in the shared state file.
                let mut recorded = source::fetch_recorded_agents(&self.db);
                for (id, kind) in source::fetch_state_kinds() {
                    recorded.entry(id).or_insert(kind);
                }
                let marked = self.db.list_pane_overrides().unwrap_or_else(|e| {
                    if self.verbose {
                        eprintln!("Warning: list_pane_overrides failed: {e}");
                    }
                    std::collections::HashSet::new()
                });
                let current_session = crate::tmux::current_session(self.verbose).ok();
                let old_group_count = self.tree.groups.len();
                // Exclude the TUI's own pane so it never lists itself (D7).
                let my_pane_id = self.my_pane_id.clone();
                self.tree.rebuild(
                    &panes,
                    &states,
                    &recorded,
                    &marked,
                    current_session.as_deref(),
                    &my_pane_id,
                );
                self.status_message = None;
                // Only upsert projects when groups change (avoids writes every 5s tick)
                if self.tree.groups.len() != old_group_count {
                    for group in &self.tree.groups {
                        let _ = self.db.upsert_project(&group.path.to_string_lossy());
                    }
                }
            }
            (Err(e), _) | (_, Err(e)) => {
                self.status_message = Some(format!("Refresh error: {e}"));
            }
        }
    }

    /// Refresh preview content for the selected pane.
    pub fn refresh_preview(&mut self) {
        if self.diff_mode {
            let dir = self
                .tree
                .selected_group()
                .map(|g| g.path.clone())
                .or_else(|| {
                    self.tree
                        .selected_pane()
                        .map(|p| p.pane_info.current_path.clone())
                });
            if let Some(path) = dir {
                match source::fetch_git_diffs(&path) {
                    Ok(repos) => {
                        if let Some(ref mut ds) = self.diff_state {
                            ds.update(repos);
                        } else {
                            self.diff_state = Some(DiffState::new(repos));
                        }
                    }
                    Err(e) => self.status_message = Some(format!("Git diff error: {e}")),
                }
            }
            return;
        }
        if let Some(pane_id) = self.tree.selected_pane_id().map(|s| s.to_string()) {
            match source::fetch_preview(&pane_id, self.verbose) {
                Ok(content) => {
                    self.preview_content = content;
                }
                Err(e) => {
                    self.status_message = Some(format!("Preview error: {e}"));
                }
            }
        } else if let Some(group) = self.tree.selected_group() {
            let path = group.path.clone();
            match source::fetch_directory_listing(&path) {
                Ok(listing) => {
                    self.preview_content = listing;
                }
                Err(e) => {
                    self.status_message = Some(format!("Directory error: {e}"));
                }
            }
        }
    }

    /// Get the poll timeout.
    pub fn poll_timeout(&self) -> Duration {
        TREE_POLL
    }

    /// Refresh the projects list from the database.
    pub fn refresh_projects(&mut self) {
        self.projects = self.db.list_projects().unwrap_or_default();
        if self.projects_cursor >= self.projects.len() {
            self.projects_cursor = self.projects.len().saturating_sub(1);
        }
    }

    /// Called on each tick (timeout expiry) to refresh data.
    pub fn on_tick(&mut self) {
        self.refresh_tree();
        self.refresh_projects();
        self.refresh_preview();
    }

    pub fn sync_note_to_group(&mut self) {
        let current_path = self
            .tree
            .cursor_group()
            .map(|g| g.path.to_string_lossy().to_string());
        let Some(path) = current_path else {
            return;
        };
        if path != self.notepad.project {
            self.save_note();
            self.notepad.editor = self.load_note(&path);
            self.notepad.project = path;
        }
    }

    fn load_note(&self, path: &str) -> EditorState {
        let content = match self.db.get_note(path) {
            Ok(Some(c)) => c,
            Ok(None) => String::new(),
            Err(e) => {
                eprintln!("Note load error: {e}");
                String::new()
            }
        };
        EditorState::new(Lines::from(content.as_str()))
    }

    /// Get filtered projects list indices matching the current search filter.
    pub fn filtered_project_indices(&self) -> Vec<usize> {
        match &self.projects_search_filter {
            Some(query) if !query.is_empty() => self
                .projects
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    let q = query.to_lowercase();
                    p.name.to_lowercase().contains(&q)
                        || p.path.to_string_lossy().to_lowercase().contains(&q)
                })
                .map(|(i, _)| i)
                .collect(),
            _ => (0..self.projects.len()).collect(),
        }
    }

    pub fn save_note(&mut self) {
        if self.notepad.project.is_empty() {
            return;
        }
        let content = self.notepad.editor.lines.to_string();
        if let Err(e) = self.db.save_note(&self.notepad.project, &content) {
            self.status_message = Some(format!("Failed to save note: {e}"));
        }
    }
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

    /// TUI-config-flag-honored (P0 / A2): the App carries the config it was
    /// constructed with (which `main` resolved from `--config`), not a reloaded
    /// default. Pre-refactor `App::new` called `GroveConfig::load(None,..)`,
    /// silently discarding `--config`.
    #[test]
    fn config_flag_honored() {
        let config = GroveConfig {
            claude_command: "claude --distinctive-flag".to_string(),
            ..Default::default()
        };
        let app = App::with_parts(config, temp_db(), String::new(), false, false);
        assert_eq!(app.config.claude_command, "claude --distinctive-flag");
        assert_eq!(app.default_agent_command, "claude --distinctive-flag");
    }

    /// TUI-refresh-gated-under-modal (P1 / sigusr1-tick-modal): the event loop
    /// gates `on_tick`/SIGUSR1 `refresh_tree` on `!overlay_active()`, so a tmux
    /// hook firing mid-search can't rebuild the tree and reset the cursor under
    /// the user's typing. Pin the predicate the gate relies on.
    #[test]
    fn refresh_gated_under_modal() {
        let mut app = App::with_parts(
            GroveConfig::default(),
            temp_db(),
            String::new(),
            false,
            false,
        );

        // No overlay: background refresh is allowed.
        assert!(!app.overlay_active());

        // Entering search captures input → background refresh is suppressed.
        app.overlay = Overlay::Search {
            query: "x".to_string(),
            target: SidebarFocus::Tree,
        };
        assert!(app.overlay_active());

        // Closing the overlay re-enables background refresh.
        app.overlay = Overlay::None;
        assert!(!app.overlay_active());
    }
}
