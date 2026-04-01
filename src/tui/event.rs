use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::error::GroveError;

use super::actions;
use super::app::App;
use super::ui;

/// Suspend the TUI, run a closure, then restore the terminal.
fn suspend_tui<F, R>(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), LeaveAlternateScreen);

    let result = f();

    let _ = enable_raw_mode();
    let _ = execute!(std::io::stdout(), EnterAlternateScreen);
    terminal.clear().ok();

    result
}

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
            .draw(|f| ui::draw(f, &mut *app))
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
            suspend_tui(terminal, || {
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

                eprintln!("[grove] Press Enter to return to TUI...");
                let _ = std::io::stdin().read_line(&mut String::new());

                if let Err(e) = status {
                    app.status_message = Some(format!("command failed: {e}"));
                }
            });

            app.refresh_tree();
            app.refresh_preview();
        }

        // Handle fzf directory picker → sets open_prompt_dir for sub-choice
        if std::mem::take(&mut app.pending_fzf) {
            let result = suspend_tui(terminal, || {
                std::process::Command::new("sh")
                    .arg("-c")
                    .arg("{ grove list --json 2>/dev/null | jq -r '.tasks[].path // empty' 2>/dev/null; zoxide query -l 2>/dev/null; } | awk '!seen[$0]++' | fzf --prompt='Directory> '")
                    .stdin(std::process::Stdio::inherit())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::inherit())
                    .output()
            });

            if let Ok(output) = result {
                if output.status.success() {
                    let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !dir.is_empty() {
                        app.open_prompt_dir = Some(dir);
                    }
                }
            }

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
