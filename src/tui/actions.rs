use crossterm::event::KeyEvent;

use crate::claude::ClaudeState;
use crate::tmux;

use super::app::App;

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

    // Search input mode
    if let Some(ref mut query) = app.search_input {
        match key.code {
            KeyCode::Enter => {
                if let Some(pane_id) = app.tree.selected_pane_id().map(|s| s.to_string()) {
                    let _ = tmux::switch_to_pane(&pane_id, app.verbose);
                    app.should_quit = true;
                }
                app.search_input = None;
                app.tree.search_filter = None;
            }
            KeyCode::Esc => {
                app.search_input = None;
                app.tree.search_filter = None;
            }
            KeyCode::Down => {
                app.tree.move_cursor_to_pane(true);
                update_scroll(app);
                app.refresh_preview();
            }
            KeyCode::Up => {
                app.tree.move_cursor_to_pane(false);
                update_scroll(app);
                app.refresh_preview();
            }
            KeyCode::Char(c) => {
                query.push(c);
                app.tree.search_filter = Some(query.clone());
                app.tree.jump_first_pane();
                update_scroll(app);
                app.refresh_preview();
            }
            KeyCode::Backspace => {
                query.pop();
                app.tree.search_filter = if query.is_empty() {
                    None
                } else {
                    Some(query.clone())
                };
                app.tree.jump_first_pane();
                update_scroll(app);
                app.refresh_preview();
            }
            _ => {}
        }
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
            app.tree.move_cursor_to_pane(true);
            update_scroll(app);
            app.preview_scroll_up = 0;
            app.refresh_preview();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.tree.move_cursor_to_pane(false);
            update_scroll(app);
            app.preview_scroll_up = 0;
            app.refresh_preview();
        }
        KeyCode::Char('J') => {
            app.preview_scroll_up = app.preview_scroll_up.saturating_sub(3);
        }
        KeyCode::Char('K') => {
            app.preview_scroll_up = app.preview_scroll_up.saturating_add(3);
        }
        KeyCode::Char('h') | KeyCode::Left => {
            app.tree.collapse_current_group();
            update_scroll(app);
        }
        KeyCode::Char('l') | KeyCode::Right => {
            app.tree.expand_current_group();
            update_scroll(app);
            app.refresh_preview();
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
                let _ = tmux::send_raw_keys(&pane.pane_info.pane_id, &["n", "Enter"], app.verbose);
                app.refresh_tree();
            }
        }
        KeyCode::Char('s') => {
            // Enter prompt input mode
            if app.tree.selected_pane().is_some() {
                app.prompt_input = Some(String::new());
            }
        }
        KeyCode::Char('/') => {
            app.search_input = Some(String::new());
        }
        KeyCode::Char('x') => {
            if let Some(pane) = app.tree.selected_pane() {
                let pane_id = pane.pane_info.pane_id.clone();
                let _ = tmux::kill_pane(&pane_id, app.verbose);
                app.refresh_tree();
                app.refresh_preview();
            }
        }
        KeyCode::Char('n') => {
            app.pending_popup = Some("grove init -i".to_string());
        }
        KeyCode::Char('g') => {
            app.tree.jump_first_pane();
            update_scroll(app);
            app.refresh_preview();
        }
        KeyCode::Char('G') => {
            app.tree.jump_last_pane();
            update_scroll(app);
            app.refresh_preview();
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
