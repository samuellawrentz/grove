use crossterm::event::KeyEvent;

use crate::claude::ClaudeState;
use crate::tmux;

use super::app::{App, Focus};

/// Handle a key event in the TUI.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    app.last_interaction = std::time::Instant::now();
    app.status_message = None;

    use crossterm::event::{KeyCode, KeyModifiers};

    // Ctrl-C always quits
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }

    // Prompt input mode
    if let Some(ref mut input) = app.prompt_input {
        match key.code {
            KeyCode::Enter => {
                let text = input.clone();
                if let Some(pane_id) = app.tree.selected_pane_id().map(|s| s.to_string())
                    && !text.is_empty()
                {
                    let _ = tmux::send_keys(&pane_id, &text, app.verbose);
                    app.refresh_tree();
                }
                app.prompt_input = None;
            }
            KeyCode::Esc => {
                app.prompt_input = None;
            }
            KeyCode::Char(c) => {
                input.push(c);
            }
            KeyCode::Backspace => {
                input.pop();
            }
            _ => {}
        }
        return;
    }

    // Normal mode
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.tree.move_cursor(1);
            update_scroll(app);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.tree.move_cursor(-1);
            update_scroll(app);
        }
        KeyCode::Char('h') | KeyCode::Left => {
            app.tree.toggle_expand();
            update_scroll(app);
        }
        KeyCode::Char('l') | KeyCode::Right => {
            app.tree.toggle_expand();
            update_scroll(app);
        }
        KeyCode::Tab => {
            app.toggle_focus();
            // Refresh preview when switching to preview panel
            if matches!(app.focus, Focus::Preview) {
                app.refresh_preview();
            }
        }
        KeyCode::Enter => {
            if let Some(pane_id) = app.tree.selected_pane_id().map(|s| s.to_string()) {
                let _ = tmux::switch_to_pane(&pane_id, app.verbose);
                app.should_quit = true;
            }
        }
        KeyCode::Char('a') => {
            // Accept: send Enter to a waiting Claude pane
            if let Some(pane) = app.tree.selected_pane()
                && pane.claude_state == Some(ClaudeState::Waiting)
            {
                let _ = tmux::send_raw_keys(&pane.pane_info.pane_id, &["Enter"], app.verbose);
                app.refresh_tree();
            }
        }
        KeyCode::Char('r') => {
            // Reject: send "n" + Enter to a waiting Claude pane
            if let Some(pane) = app.tree.selected_pane()
                && pane.claude_state == Some(ClaudeState::Waiting)
            {
                let _ =
                    tmux::send_raw_keys(&pane.pane_info.pane_id, &["n", "Enter"], app.verbose);
                app.refresh_tree();
            }
        }
        KeyCode::Char('s') => {
            // Enter prompt input mode
            if app.tree.selected_pane().is_some() {
                app.prompt_input = Some(String::new());
            }
        }
        KeyCode::Char('n') => {
            app.status_message = Some("Run `grove init -i` in another pane".to_string());
        }
        KeyCode::Char('g') => {
            app.tree.cursor = 0;
            update_scroll(app);
        }
        KeyCode::Char('G') => {
            let count = app.tree.visible_count();
            if count > 0 {
                app.tree.cursor = count - 1;
            }
            update_scroll(app);
        }
        _ => {}
    }
}

/// Keep scroll_offset in sync with cursor position.
fn update_scroll(app: &mut App) {
    // We don't know the exact visible height here (it depends on the terminal size),
    // but we can ensure the cursor is always within a reasonable scroll window.
    // The ui.rs draw_tree function will use scroll_offset to render.
    // We use a simple heuristic: keep cursor visible in a window of ~20 rows.
    let visible_height = 20_usize; // approximate; ui.rs adjusts at render time

    if app.tree.cursor < app.tree.scroll_offset {
        app.tree.scroll_offset = app.tree.cursor;
    } else if app.tree.cursor >= app.tree.scroll_offset + visible_height {
        app.tree.scroll_offset = app.tree.cursor - visible_height + 1;
    }
}
