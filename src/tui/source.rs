use std::collections::HashMap;

use crate::claude::{self, ClaudeState};
use crate::error::GroveError;
use crate::tmux::{self, PaneInfo};

/// Fetch all tmux panes.
pub(crate) fn fetch_panes(verbose: bool) -> Result<Vec<PaneInfo>, GroveError> {
    tmux::list_all_panes(verbose)
}

/// Fetch Claude states from the external hook state file.
pub(crate) fn fetch_claude_states() -> Result<HashMap<String, ClaudeState>, GroveError> {
    claude::read_state_file()
}

/// Capture the visible content of a tmux pane.
pub(crate) fn fetch_preview(pane_id: &str, verbose: bool) -> Result<String, GroveError> {
    tmux::capture_pane(pane_id, verbose)
}
