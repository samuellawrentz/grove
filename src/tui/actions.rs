use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, Overlay, PendingShell};

/// Handle a key event in the recents launcher.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    app.status_message = None;

    // Ctrl-C always quits.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }

    // Input-capture overlays take the key first.
    match std::mem::replace(&mut app.overlay, Overlay::None) {
        Overlay::Search(mut query) => {
            handle_search_key(app, key, &mut query);
            return;
        }
        Overlay::OpenChoice { dir } => {
            handle_open_choice_key(app, key, dir);
            return;
        }
        Overlay::None => {}
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => move_cursor(app, true),
        KeyCode::Char('k') | KeyCode::Up => move_cursor(app, false),
        KeyCode::Char('g') => app.cursor = 0,
        KeyCode::Char('G') => app.cursor = app.filtered_indices().len().saturating_sub(1),
        KeyCode::Char('/') => app.overlay = Overlay::Search(String::new()),
        // (a) Enter → new Claude workspace at the selected recent.
        KeyCode::Enter => {
            if let Some(dir) = app.selected_dir() {
                let cmd = app.claude_cmd();
                app.launch_workspace(&dir, Some(&cmd));
            }
        }
        // (c) `o` → fzf directory picker → OpenChoice overlay.
        KeyCode::Char('o') => app.pending_shell = PendingShell::FzfPicker,
        // (b) `n` → interactive `grove init -i`, then build its workspace.
        KeyCode::Char('n') => app.pending_shell = PendingShell::NewTask,
        _ => {}
    }
}

fn move_cursor(app: &mut App, down: bool) {
    let len = app.filtered_indices().len();
    if down {
        if app.cursor + 1 < len {
            app.cursor += 1;
        }
    } else {
        app.cursor = app.cursor.saturating_sub(1);
    }
}

/// Search over recents. Enter launches Claude at the selected match.
fn handle_search_key(app: &mut App, key: KeyEvent, query: &mut String) {
    match key.code {
        KeyCode::Enter => {
            if let Some(dir) = app.selected_dir() {
                let cmd = app.claude_cmd();
                app.search = None;
                app.launch_workspace(&dir, Some(&cmd));
            }
            app.search = None;
        }
        KeyCode::Esc => app.search = None,
        KeyCode::Down => {
            move_cursor(app, true);
            rearm_search(app, query);
        }
        KeyCode::Up => {
            move_cursor(app, false);
            rearm_search(app, query);
        }
        KeyCode::Char(c) => {
            query.push(c);
            app.search = Some(query.clone());
            app.cursor = 0;
            rearm_search(app, query);
        }
        KeyCode::Backspace => {
            query.pop();
            app.search = if query.is_empty() {
                None
            } else {
                Some(query.clone())
            };
            app.cursor = 0;
            rearm_search(app, query);
        }
        _ => rearm_search(app, query),
    }
}

fn rearm_search(app: &mut App, query: &str) {
    app.overlay = Overlay::Search(query.to_string());
}

/// A directory was picked; choose what to run in a new workspace.
fn handle_open_choice_key(app: &mut App, key: KeyEvent, dir: String) {
    let claude = app.claude_cmd();
    let cmd: Option<&str> = match key.code {
        KeyCode::Char('c') => Some(claude.as_str()),
        KeyCode::Char('t') => None,          // plain shell
        KeyCode::Char('e') => Some("nvim ."),
        KeyCode::Esc => {
            return; // cancelled
        }
        _ => {
            // Unrecognized: keep the overlay open awaiting a valid choice.
            app.overlay = Overlay::OpenChoice { dir };
            return;
        }
    };
    app.launch_workspace(&dir, cmd);
}
