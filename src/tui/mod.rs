//! `grove tui`: a recents-only launcher.
//!
//! A single pane — the recents (Projects) list. Selecting a recent, the `o`
//! picker, and `n` (new task) all create herdr *workspaces*. No tmux, no tree,
//! no preview, no notepad.

pub(crate) mod actions;
pub(crate) mod app;
pub(crate) mod event;
pub(crate) mod ui;

use std::io;

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::config::GroveConfig;
use crate::db::Db;
use crate::error::GroveError;

/// Guard that restores the terminal on drop.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
}

/// Entry point for the TUI. Takes ownership of the config and db handle.
pub(crate) fn run(
    config: GroveConfig,
    db: Db,
    _verbose: bool,
    popup: bool,
) -> Result<(), GroveError> {
    enable_raw_mode().map_err(|e| GroveError::Tui(format!("failed to enable raw mode: {e}")))?;
    execute!(io::stdout(), EnterAlternateScreen)
        .map_err(|e| GroveError::Tui(format!("failed to enter alternate screen: {e}")))?;

    let _guard = TerminalGuard;

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        original_hook(info);
    }));

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)
        .map_err(|e| GroveError::Tui(format!("failed to create terminal: {e}")))?;

    let mut app = app::App::new(config, db, popup);

    event::run_event_loop(&mut terminal, &mut app)
}
