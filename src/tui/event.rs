use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::error::GroveError;

use super::actions;
use super::app::App;
use super::ui;

/// Run the main event loop.
pub(crate) fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<(), GroveError> {
    // Register SIGUSR1 handler for tmux hook refresh
    let sigusr1_flag = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        let _ =
            signal_hook::flag::register(signal_hook::consts::SIGUSR1, Arc::clone(&sigusr1_flag));
    }

    loop {
        // Draw
        terminal
            .draw(|f| ui::draw(f, app))
            .map_err(|e| GroveError::Tui(format!("draw error: {e}")))?;

        if app.should_quit {
            break;
        }

        // Poll for events with adaptive timeout
        let timeout = app.poll_timeout();
        let has_event =
            event::poll(timeout).map_err(|e| GroveError::Tui(format!("poll error: {e}")))?;

        if has_event {
            let ev = event::read().map_err(|e| GroveError::Tui(format!("read error: {e}")))?;
            match ev {
                Event::Key(key) => {
                    actions::handle_key(app, key);
                }
                Event::Resize(_, _) => {
                    // Ratatui handles resize on next draw
                }
                _ => {}
            }
        }

        // Handle pending shell-out: suspend TUI, run command, resume
        if let Some(cmd) = app.pending_popup.take() {
            // Leave alternate screen and raw mode so the child can use the terminal
            let _ = disable_raw_mode();
            let _ = execute!(std::io::stdout(), LeaveAlternateScreen);

            // Run command directly as a child process (like vim :!)
            let status = std::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .status();

            match &status {
                Ok(s) if !s.success() => {
                    eprintln!("\n[grove] command exited with {s}");
                }
                Err(e) => {
                    eprintln!("\n[grove] command failed: {e}");
                }
                _ => {}
            }

            // Wait for keypress so user can see output
            eprintln!("[grove] Press Enter to return to TUI...");
            let _ = std::io::stdin().read_line(&mut String::new());

            if let Err(e) = status {
                app.status_message = Some(format!("command failed: {e}"));
            }

            // Restore terminal state
            let _ = enable_raw_mode();
            let _ = execute!(std::io::stdout(), EnterAlternateScreen);
            terminal.clear().ok();

            app.refresh_tree();
            app.refresh_preview();
        }

        if !has_event {
            // Timeout: refresh data
            app.on_tick();
        }

        // Check SIGUSR1 flag
        if sigusr1_flag.swap(false, Ordering::Relaxed) {
            app.refresh_tree();
        }
    }

    Ok(())
}
