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

/// Main application state for the TUI.
pub(crate) struct App {
    pub tree: TreeState,
    pub preview_content: String,
    pub last_interaction: Instant,
    pub should_quit: bool,
    pub verbose: bool,
    pub search_input: Option<String>,
    pub prompt_input: Option<String>,
    pub status_message: Option<String>,
    #[allow(dead_code)]
    pub my_pane_id: String,
    pub pending_popup: Option<String>,
    pub pending_fzf: bool,
    /// Directory picked by fzf, awaiting open-prompt sub-choice.
    pub open_prompt_dir: Option<String>,
    pub preview_scroll_up: u16,
    pub diff_mode: bool,
    pub diff_state: Option<DiffState>,
    pub default_agent_command: String,
    pub sidebar_focus: SidebarFocus,
    pub focus: Focus,
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
    /// Create a new App, querying the TUI's own pane ID.
    pub fn new(verbose: bool, popup: bool) -> Result<Self, GroveError> {
        let my_pane_id = std::env::var("TMUX_PANE").unwrap_or_default();
        let my_pane_id = if my_pane_id.is_empty() {
            tmux::get_pane_id("", verbose).unwrap_or_default()
        } else {
            my_pane_id
        };

        let default_agent_command = GroveConfig::load(None, None, None, None)
            .map(|(c, _)| c.claude_command)
            .unwrap_or_else(|_| "claude".to_string());

        let db = Db::open()?;
        let projects = db.list_projects().unwrap_or_default();

        let mut app = App {
            tree: TreeState {
                groups: Vec::new(),
                cursor: 0,
                scroll_offset: 0,
                search_filter: None,
                agent_filter: AgentFilter::AnyAgent,
            },
            search_input: None,
            preview_content: String::new(),
            last_interaction: Instant::now(),
            should_quit: false,
            verbose,
            prompt_input: None,
            status_message: None,
            my_pane_id,
            pending_popup: None,
            pending_fzf: false,
            open_prompt_dir: None,
            preview_scroll_up: 0,
            diff_mode: false,
            diff_state: None,
            default_agent_command,
            sidebar_focus: SidebarFocus::Tree,
            db,
            projects,
            projects_cursor: 0,
            projects_search_filter: None,
            popup,
            focus: Focus::Sidebar,
            show_notepad: false,
            notepad: NoteState {
                editor: EditorState::default(),
                project: String::new(),
            },
        };

        app.refresh_tree();
        app.tree.jump_first_pane();
        app.refresh_preview();
        app.sync_note_to_group();
        Ok(app)
    }

    /// Refresh tree data from tmux and claude state.
    pub fn refresh_tree(&mut self) {
        match (
            source::fetch_panes(self.verbose),
            source::fetch_agent_states(),
        ) {
            (Ok(panes), Ok(states)) => {
                let old_group_count = self.tree.groups.len();
                self.tree.rebuild(&panes, &states, "");
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
