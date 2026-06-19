pub(crate) mod actions;
pub(crate) mod app;
pub(crate) mod diff_view;
pub(crate) mod event;
pub(crate) mod flat_rows;
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

/// Entry point for the TUI. Takes ownership of the config (already resolved
/// against `--config`) and the db handle, so the App owns both — no reload.
pub(crate) fn run(
    config: GroveConfig,
    db: Db,
    verbose: bool,
    popup: bool,
) -> Result<(), GroveError> {
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

    // Write PID file for tmux hook signal delivery. A write failure must not be
    // silently swallowed (hooks would then fail to signal the TUI with no clue
    // why) — surface it as a warning to stderr.
    let pid = std::process::id();
    let pid_path = "/tmp/grove-tui.pid";
    let pid_write = std::fs::write(pid_path, pid.to_string());
    if let Some(warning) =
        pidfile_warning(pid_path, pid_write.map(|_| ()).map_err(|e| e.to_string()))
    {
        eprintln!("{warning}");
    }

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)
        .map_err(|e| GroveError::Tui(format!("failed to create terminal: {e}")))?;

    // Register tmux hooks to track projects
    crate::tmux::register_project_hooks(verbose);

    // Cheap construction only — the blocking refresh happens after the first
    // frame is drawn, inside the event loop, so startup is not a perceived hang.
    let my_pane_id = app::App::resolve_my_pane_id(verbose);
    let mut app = app::App::construct(config, db, my_pane_id, verbose, popup);

    let result = event::run_event_loop(&mut terminal, &mut app);

    // Clean up PID file
    let _ = std::fs::remove_file(pid_path);

    result
}

/// Compute the warning to surface for a pid-file write outcome: `Some(message)`
/// on failure (so the caller logs it), `None` on success. Pure so the
/// surface-don't-swallow behavior is unit-testable without touching `/tmp`.
fn pidfile_warning(path: &str, outcome: Result<(), String>) -> Option<String> {
    outcome
        .err()
        .map(|e| format!("Warning: failed to write pid file {path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PIDFILE-write-failure-surfaced (S25): a write error must produce a
    /// warning, not be silently dropped.
    #[test]
    fn pidfile_write_error_is_surfaced() {
        let warn = pidfile_warning("/tmp/grove-tui.pid", Err("permission denied".to_string()));
        let warn = warn.expect("write failure must surface a warning");
        assert!(warn.contains("/tmp/grove-tui.pid"));
        assert!(warn.contains("permission denied"));
    }

    #[test]
    fn pidfile_write_success_is_quiet() {
        assert!(pidfile_warning("/tmp/grove-tui.pid", Ok(())).is_none());
    }
}
