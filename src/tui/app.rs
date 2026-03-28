use std::time::{Duration, Instant};

use crate::agent::AgentFilter;
use crate::config::GroveConfig;
use crate::error::GroveError;
use crate::recents::{self, RecentEntry};
use crate::tmux;

use super::source::{self, DiffState};
use super::tree::TreeState;

const TREE_POLL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarFocus {
    Tree,
    Recents,
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
    pub recents: Vec<RecentEntry>,
    pub recents_cursor: usize,
}

impl App {
    /// Create a new App, querying the TUI's own pane ID.
    pub fn new(verbose: bool) -> Result<Self, GroveError> {
        let my_pane_id = std::env::var("TMUX_PANE").unwrap_or_default();
        let my_pane_id = if my_pane_id.is_empty() {
            tmux::get_pane_id("", verbose).unwrap_or_default()
        } else {
            my_pane_id
        };

        let default_agent_command = GroveConfig::load(None, None, None, None)
            .map(|(c, _)| c.claude_command)
            .unwrap_or_else(|_| "claude".to_string());

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
            recents: recents::load(),
            recents_cursor: 0,
        };

        app.refresh_tree();
        app.tree.jump_first_pane();
        app.refresh_preview();
        Ok(app)
    }

    /// Refresh tree data from tmux and claude state.
    pub fn refresh_tree(&mut self) {
        match (
            source::fetch_panes(self.verbose),
            source::fetch_agent_states(),
        ) {
            (Ok(panes), Ok(states)) => {
                self.tree.rebuild(&panes, &states, "");
                self.status_message = None;
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

    /// Refresh the recents list from disk.
    pub fn refresh_recents(&mut self) {
        self.recents = recents::load();
        if self.recents_cursor >= self.recents.len() && !self.recents.is_empty() {
            self.recents_cursor = self.recents.len() - 1;
        }
    }

    /// Called on each tick (timeout expiry) to refresh data.
    pub fn on_tick(&mut self) {
        self.refresh_tree();
        self.refresh_recents();
        self.refresh_preview();
    }
}
