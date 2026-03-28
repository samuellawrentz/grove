use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

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

const MAX_REPOS: usize = 5;
const MAX_LINES_PER_REPO: usize = 500;

/// Find git repos in a directory (the dir itself + immediate children), run git diff.
pub(crate) fn fetch_git_diffs(dir: &Path) -> Result<String, GroveError> {
    let mut repos: Vec<std::path::PathBuf> = Vec::new();

    // Check if dir itself is a repo
    if dir.join(".git").exists() {
        repos.push(dir.to_path_buf());
    }

    // Check immediate children
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if repos.len() >= MAX_REPOS {
                break;
            }
            let path = entry.path();
            if path.is_dir() && path.join(".git").exists() {
                repos.push(path);
            }
        }
    }

    if repos.is_empty() {
        return Ok("No git repositories found".to_string());
    }

    let mut output = String::new();
    for (i, repo) in repos.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }
        let name = repo.to_string_lossy();
        output.push_str(&format!("━━━ {} ━━━\n", name));

        let stat = Command::new("git")
            .args(["-C", &name, "diff", "--stat"])
            .output();
        let diff = Command::new("git")
            .args(["-C", &name, "diff", "--color=always"])
            .output();

        match (stat, diff) {
            (Ok(s), Ok(d)) => {
                let stat_out = String::from_utf8_lossy(&s.stdout);
                let diff_out = String::from_utf8_lossy(&d.stdout);
                if stat_out.is_empty() && diff_out.is_empty() {
                    output.push_str("No changes\n");
                } else {
                    if !stat_out.is_empty() {
                        output.push_str(&stat_out);
                        output.push('\n');
                    }
                    // Cap diff lines
                    let lines: Vec<&str> = diff_out.lines().collect();
                    let capped = lines.len() > MAX_LINES_PER_REPO;
                    for line in lines.iter().take(MAX_LINES_PER_REPO) {
                        output.push_str(line);
                        output.push('\n');
                    }
                    if capped {
                        output.push_str(&format!(
                            "... truncated ({} lines total)\n",
                            lines.len()
                        ));
                    }
                }
            }
            _ => {
                output.push_str("Failed to run git diff\n");
            }
        }
    }

    Ok(output)
}
