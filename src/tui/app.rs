use std::time::{Duration, Instant};

use crate::error::GroveError;
use crate::tmux;

use super::source;
use super::tree::TreeState;

const TREE_POLL: Duration = Duration::from_secs(5);

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
    pub preview_scroll_up: u16,
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
                search_filter: None,
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
            preview_scroll_up: 0,
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
            source::fetch_claude_states(),
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

    /// Get the poll timeout.
    pub fn poll_timeout(&self) -> Duration {
        TREE_POLL
    }

    /// Called on each tick (timeout expiry) to refresh data.
    pub fn on_tick(&mut self) {
        self.refresh_tree();
        self.refresh_preview();
    }
}
