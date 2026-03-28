use std::collections::HashMap;

use crate::agent::{self, identify_agent, scrape_pane_state, AgentState, DetectStrategy};
use crate::error::GroveError;
use crate::tmux::{self, PaneInfo};

/// Fetch all tmux panes.
pub(crate) fn fetch_panes(verbose: bool) -> Result<Vec<PaneInfo>, GroveError> {
    tmux::list_all_panes(verbose)
}

/// Fetch agent states from the external hook state file.
pub(crate) fn fetch_agent_states() -> Result<HashMap<String, AgentState>, GroveError> {
    agent::read_state_file()
}

#[allow(dead_code)]
const MAX_SCRAPE_PANES: usize = 8;
#[allow(dead_code)]
const SKIP_COMMANDS: &[&str] = &[
    "zsh", "bash", "fish", "vim", "nvim", "tmux", "less", "man", "top", "htop", "grove",
];

/// Fetch agent states with pane scraping for non-Claude agents.
#[allow(dead_code)]
pub(crate) fn fetch_agent_states_with_scraping(
    panes: &[PaneInfo],
    verbose: bool,
) -> Result<HashMap<String, AgentState>, GroveError> {
    let mut states = agent::read_state_file()?;

    let mut scraped = 0;
    for pane in panes {
        if scraped >= MAX_SCRAPE_PANES {
            break;
        }
        if states.contains_key(&pane.pane_id) {
            continue;
        }
        if SKIP_COMMANDS.contains(&pane.current_command.as_str()) {
            continue;
        }
        if let Some(def) = identify_agent(pane) {
            if matches!(def.detect, DetectStrategy::PaneScrape { .. }) {
                let state = scrape_pane_state(&pane.pane_id, def, verbose);
                states.insert(pane.pane_id.clone(), state);
                scraped += 1;
            }
        }
    }

    Ok(states)
}

/// Capture the visible content of a tmux pane.
pub(crate) fn fetch_preview(pane_id: &str, verbose: bool) -> Result<String, GroveError> {
    tmux::capture_pane(pane_id, verbose)
}

/// Fetch a directory listing for preview when cursor is on a group header.
/// Returns directories first (with `/` suffix), then files, sorted alphabetically.
pub(crate) fn fetch_directory_listing(path: &std::path::Path) -> Result<String, GroveError> {
    let entries = std::fs::read_dir(path)
        .map_err(|e| GroveError::General(format!("read_dir failed: {e}")))?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden files
        if name.starts_with('.') {
            continue;
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            dirs.push(format!("{name}/"));
        } else {
            files.push(name);
        }
    }

    dirs.sort();
    files.sort();

    let mut output = String::new();
    for d in &dirs {
        output.push_str(d);
        output.push('\n');
    }
    for f in &files {
        output.push_str(f);
        output.push('\n');
    }

    Ok(output)
}
