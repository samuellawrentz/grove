use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{self, Event};
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
        } else {
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
