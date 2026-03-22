use std::time::{Duration, Instant};

use crate::error::GroveError;
use crate::tmux;

use super::source;
use super::tree::TreeState;

const TREE_POLL: Duration = Duration::from_secs(5);
const PREVIEW_POLL: Duration = Duration::from_millis(200);
const PREVIEW_IDLE_POLL: Duration = Duration::from_secs(2);
const IDLE_THRESHOLD: Duration = Duration::from_secs(30);

/// Which panel has focus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Focus {
    Tree,
    Preview,
}

/// Main application state for the TUI.
pub(crate) struct App {
    pub tree: TreeState,
    pub focus: Focus,
    pub preview_content: String,
    pub last_interaction: Instant,
    pub should_quit: bool,
    pub verbose: bool,
    pub prompt_input: Option<String>,
    pub status_message: Option<String>,
    pub my_pane_id: String,
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

        let mut app = App {
            tree: TreeState {
                groups: Vec::new(),
                cursor: 0,
                scroll_offset: 0,
            },
            focus: Focus::Tree,
            preview_content: String::new(),
            last_interaction: Instant::now(),
            should_quit: false,
            verbose,
            prompt_input: None,
            status_message: None,
            my_pane_id,
        };

        app.refresh_tree();
        Ok(app)
    }

    /// Refresh tree data from tmux and claude state.
    pub fn refresh_tree(&mut self) {
        match (
            source::fetch_panes(self.verbose),
            source::fetch_claude_states(),
        ) {
            (Ok(panes), Ok(states)) => {
                self.tree.rebuild(&panes, &states, &self.my_pane_id);
                self.status_message = None;
            }
            (Err(e), _) | (_, Err(e)) => {
                self.status_message = Some(format!("Refresh error: {e}"));
            }
        }
    }

    /// Refresh preview content for the selected pane.
    pub fn refresh_preview(&mut self) {
        if let Some(pane_id) = self.tree.selected_pane_id().map(|s| s.to_string()) {
            match source::fetch_preview(&pane_id, self.verbose) {
                Ok(content) => {
                    self.preview_content = content;
                }
                Err(e) => {
                    self.status_message = Some(format!("Preview error: {e}"));
                }
            }
        }
    }

    /// Get the poll timeout based on focus and idle state.
    pub fn poll_timeout(&self) -> Duration {
        match self.focus {
            Focus::Tree => TREE_POLL,
            Focus::Preview => {
                if self.last_interaction.elapsed() > IDLE_THRESHOLD {
                    PREVIEW_IDLE_POLL
                } else {
                    PREVIEW_POLL
                }
            }
        }
    }

    /// Called on each tick (timeout expiry) to refresh data.
    pub fn on_tick(&mut self) {
        match self.focus {
            Focus::Tree => {
                self.refresh_tree();
            }
            Focus::Preview => {
                self.refresh_preview();
            }
        }
    }

    /// Toggle focus between Tree and Preview.
    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Tree => Focus::Preview,
            Focus::Preview => Focus::Tree,
        };
    }
}
