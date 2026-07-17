use std::collections::HashSet;

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::error::GroveError;

use super::actions;
use super::app::{App, Overlay, PendingShell};
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
    loop {
        terminal
            .draw(|f| ui::draw(f, app))
            .map_err(|e| GroveError::Tui(format!("draw error: {e}")))?;

        if app.should_quit {
            break;
        }

        let timeout = app.poll_timeout();
        let has_event =
            event::poll(timeout).map_err(|e| GroveError::Tui(format!("poll error: {e}")))?;

        if has_event {
            if let Event::Key(key) = event::read()
                .map_err(|e| GroveError::Tui(format!("read error: {e}")))?
            {
                actions::handle_key(app, key);
            }
        }

        // Drain a one-shot deferred shell side-effect: suspend TUI, run, resume.
        match std::mem::replace(&mut app.pending_shell, PendingShell::None) {
            PendingShell::FzfPicker => run_fzf_picker(terminal, app),
            PendingShell::NewTask => run_new_task(terminal, app),
            PendingShell::None => {}
        }

        // Idle tick: refresh recents so externally-created tasks show up.
        if !has_event && !app.overlay_active() {
            app.load_recents();
        }
    }

    Ok(())
}

/// fzf over recent dirs (grove tasks + zoxide) → opens the OpenChoice overlay.
fn run_fzf_picker(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) {
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
                app.overlay = Overlay::OpenChoice { dir };
            }
        }
    }
}

/// (b) Suspend and run interactive `grove init -i` (task name + repos + branch),
/// then build one herdr workspace for whatever task it created.
fn run_new_task(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) {
    let before: HashSet<String> = app
        .db
        .list_tasks()
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.id)
        .collect();

    suspend_tui(terminal, || {
        let status = std::process::Command::new("grove")
            .args(["init", "-i"])
            .status();
        if let Err(e) = status {
            eprintln!("\n[grove] init failed: {e}");
            eprintln!("[grove] Press Enter to return...");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
    });

    // The subprocess committed to the same DB; find the task it created.
    app.load_recents();
    let new_task = app
        .db
        .list_tasks()
        .unwrap_or_default()
        .into_iter()
        .find(|t| !before.contains(&t.id));

    let Some(task) = new_task else {
        app.status_message = Some("No new task created".to_string());
        return;
    };

    match app.launch_task_workspace(&task) {
        Ok(()) => app.should_quit = app.popup,
        Err(e) => app.status_message = Some(format!("workspace build failed: {e}")),
    }
}
