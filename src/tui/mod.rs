pub(crate) mod actions;
pub(crate) mod app;
pub(crate) mod event;
pub(crate) mod source;
pub(crate) mod tree;
pub(crate) mod ui;

use std::io;

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

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

/// Entry point for the TUI.
pub(crate) fn run(verbose: bool, popup: bool) -> Result<(), GroveError> {
    // Set up terminal
    enable_raw_mode().map_err(|e| GroveError::Tui(format!("failed to enable raw mode: {e}")))?;
    execute!(io::stdout(), EnterAlternateScreen)
        .map_err(|e| GroveError::Tui(format!("failed to enter alternate screen: {e}")))?;

    // Create guard for cleanup on drop (normal exit or early ? return)
    let _guard = TerminalGuard;

    // Set panic hook to restore terminal on panic (belt AND suspenders)
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        original_hook(info);
    }));

    // Write PID file for tmux hook signal delivery
    let pid = std::process::id();
    let pid_path = "/tmp/grove-tui.pid";
    let _ = std::fs::write(pid_path, pid.to_string());

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)
        .map_err(|e| GroveError::Tui(format!("failed to create terminal: {e}")))?;

    // Register tmux hooks to track projects
    crate::tmux::register_project_hooks(verbose);

    let mut app = app::App::new(verbose, popup)?;

    let result = event::run_event_loop(&mut terminal, &mut app);

    // Clean up PID file
    let _ = std::fs::remove_file(pid_path);

    result
}
