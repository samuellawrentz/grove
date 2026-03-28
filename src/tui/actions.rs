use crossterm::event::KeyEvent;

use crate::agent::{AgentFilter, AgentKind, AgentState, AGENT_REGISTRY};
use crate::{recents, tmux};

use super::app::{App, SidebarFocus};

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
                if let Some(pane_id) = app.tree.selected_pane_id().map(|s| s.to_string()) {
                    if !text.is_empty() {
                        let _ = tmux::send_keys(&pane_id, &text, app.verbose);
                        app.refresh_tree();
                    }
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

    // Open prompt mode: user picked a directory, now choosing what to launch
    if let Some(dir) = app.open_prompt_dir.take() {
        match key.code {
            KeyCode::Char('c') => {
                let cmd = app.default_agent_command.clone();
                launch_in_new_window(app, &dir, Some(&cmd));
            }
            KeyCode::Char('o') => {
                let cmd = agent_command_for("opencode");
                launch_in_new_window(app, &dir, Some(&cmd));
            }
            KeyCode::Char('x') => {
                let cmd = agent_command_for("codex");
                launch_in_new_window(app, &dir, Some(&cmd));
            }
            KeyCode::Char('u') => {
                let cmd = agent_command_for("cursor");
                launch_in_new_window(app, &dir, Some(&cmd));
            }
            KeyCode::Char('t') => {
                launch_in_new_window(app, &dir, None);
            }
            KeyCode::Char('e') => {
                launch_in_new_window(app, &dir, Some("nvim ."));
            }
            KeyCode::Esc => {
                // cancelled
            }
            _ => {
                // unrecognized key, put dir back
                app.open_prompt_dir = Some(dir);
            }
        }
        return;
    }

    // Global keys (work in both panes)
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
            return;
        }
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.sidebar_focus = SidebarFocus::Tree;
            return;
        }
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.sidebar_focus = SidebarFocus::Recents;
            return;
        }
        KeyCode::Char('J') => {
            app.preview_scroll_up = app.preview_scroll_up.saturating_sub(3);
            return;
        }
        KeyCode::Char('K') => {
            app.preview_scroll_up = app.preview_scroll_up.saturating_add(3);
            return;
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.tree.agent_filter = match &app.tree.agent_filter {
                AgentFilter::All => AgentFilter::AnyAgent,
                AgentFilter::AnyAgent => AgentFilter::Specific(AgentKind::Claude),
                AgentFilter::Specific(AgentKind::Claude) => {
                    AgentFilter::Specific(AgentKind::OpenCode)
                }
                AgentFilter::Specific(AgentKind::OpenCode) => {
                    AgentFilter::Specific(AgentKind::Codex)
                }
                AgentFilter::Specific(AgentKind::Codex) => AgentFilter::Specific(AgentKind::Cursor),
                AgentFilter::Specific(AgentKind::Cursor) => AgentFilter::NonAgent,
                AgentFilter::NonAgent => AgentFilter::All,
            };
            app.tree.jump_first_pane();
            update_scroll(app);
            app.refresh_preview();
            return;
        }
        KeyCode::Char('/') => {
            app.search_input = Some(String::new());
            return;
        }
        KeyCode::Char('o') => {
            app.pending_fzf = true;
            return;
        }
        _ => {}
    }

    // Dispatch to focused pane
    match app.sidebar_focus {
        SidebarFocus::Tree => handle_tree_key(app, key),
        SidebarFocus::Recents => handle_recents_key(app, key),
    }
}

fn handle_tree_key(app: &mut App, key: KeyEvent) {
    use crossterm::event::KeyCode;

    match key.code {
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
        KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Left => {
            app.tree.collapse_current_group();
            update_scroll(app);
        }
        KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Right => {
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
            if let Some(pane) = app.tree.selected_pane() {
                if pane
                    .agent
                    .as_ref()
                    .is_some_and(|a| a.state == AgentState::Waiting)
                {
                    let keys: &[&str] = pane
                        .agent
                        .as_ref()
                        .and_then(|a| AGENT_REGISTRY.iter().find(|d| d.kind == a.kind))
                        .map(|d| d.accept_keys)
                        .unwrap_or(&["Enter"]);
                    let _ = tmux::send_raw_keys(&pane.pane_info.pane_id, keys, app.verbose);
                    app.refresh_tree();
                }
            }
        }
        KeyCode::Char('r') => {
            if let Some(pane) = app.tree.selected_pane() {
                if pane
                    .agent
                    .as_ref()
                    .is_some_and(|a| a.state == AgentState::Waiting)
                {
                    let keys: &[&str] = pane
                        .agent
                        .as_ref()
                        .and_then(|a| AGENT_REGISTRY.iter().find(|d| d.kind == a.kind))
                        .map(|d| d.reject_keys)
                        .unwrap_or(&["n", "Enter"]);
                    let _ = tmux::send_raw_keys(&pane.pane_info.pane_id, keys, app.verbose);
                    app.refresh_tree();
                }
            }
        }
        KeyCode::Char('s') => {
            if app.tree.selected_pane().is_some() {
                app.prompt_input = Some(String::new());
            }
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
        KeyCode::Char('e') => {
            if let Some((target, cwd)) = selected_target_cwd(app) {
                launch_split(app, &target, &cwd, Some("nvim ."));
            }
        }
        KeyCode::Char('C') => {
            let cmd = app.default_agent_command.clone();
            if let Some((target, cwd)) = selected_target_cwd(app) {
                launch_split(app, &target, &cwd, Some(&cmd));
            }
        }
        KeyCode::Char('O') => {
            let cmd = agent_command_for("opencode");
            if let Some((target, cwd)) = selected_target_cwd(app) {
                launch_split(app, &target, &cwd, Some(&cmd));
            }
        }
        KeyCode::Char('X') => {
            let cmd = agent_command_for("codex");
            if let Some((target, cwd)) = selected_target_cwd(app) {
                launch_split(app, &target, &cwd, Some(&cmd));
            }
        }
        KeyCode::Char('U') => {
            let cmd = agent_command_for("cursor");
            if let Some((target, cwd)) = selected_target_cwd(app) {
                launch_split(app, &target, &cwd, Some(&cmd));
            }
        }
        KeyCode::Char('T') => {
            if let Some((target, cwd)) = selected_target_cwd(app) {
                launch_split(app, &target, &cwd, None);
            }
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

fn handle_recents_key(app: &mut App, key: KeyEvent) {
    use crossterm::event::KeyCode;

    if app.recents.is_empty() {
        return;
    }

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if app.recents_cursor + 1 < app.recents.len() {
                app.recents_cursor += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.recents_cursor = app.recents_cursor.saturating_sub(1);
        }
        KeyCode::Char('c') | KeyCode::Enter => {
            let dir = app.recents[app.recents_cursor]
                .path
                .to_string_lossy()
                .to_string();
            let cmd = format!("{} -c", app.default_agent_command);
            launch_in_new_window(app, &dir, Some(&cmd));
        }
        KeyCode::Char('n') => {
            let dir = app.recents[app.recents_cursor]
                .path
                .to_string_lossy()
                .to_string();
            let cmd = app.default_agent_command.clone();
            launch_in_new_window(app, &dir, Some(&cmd));
        }
        KeyCode::Char('t') => {
            let dir = app.recents[app.recents_cursor]
                .path
                .to_string_lossy()
                .to_string();
            launch_in_new_window(app, &dir, None);
        }
        KeyCode::Char('x') => {
            recents::remove(app.recents_cursor);
            app.refresh_recents();
        }
        KeyCode::Char('g') => {
            app.recents_cursor = 0;
        }
        KeyCode::Char('G') => {
            if !app.recents.is_empty() {
                app.recents_cursor = app.recents.len() - 1;
            }
        }
        _ => {}
    }
}

/// Get target pane ID and cwd from the selected pane or group header.
fn selected_target_cwd(app: &App) -> Option<(String, String)> {
    if let Some(pane) = app.tree.selected_pane() {
        let cwd = pane.pane_info.current_path.to_string_lossy().to_string();
        let target = pane.pane_info.pane_id.clone();
        Some((target, cwd))
    } else if let Some(group) = app.tree.selected_group() {
        group.panes.first().map(|first_pane| {
            let cwd = group.path.to_string_lossy().to_string();
            let target = first_pane.pane_info.pane_id.clone();
            (target, cwd)
        })
    } else {
        None
    }
}

/// Split a window and switch to it.
fn launch_split(app: &mut App, target: &str, cwd: &str, cmd: Option<&str>) {
    match tmux::split_window(target, cwd, cmd, app.verbose) {
        Ok(new_pane_id) => {
            let _ = tmux::switch_to_pane(&new_pane_id, app.verbose);
            app.should_quit = true;
        }
        Err(e) => {
            app.status_message = Some(format!("split failed: {e}"));
        }
    }
}

/// Create a new tmux window and switch to it.
fn launch_in_new_window(app: &mut App, dir: &str, cmd: Option<&str>) {
    match tmux::new_window(dir, cmd, app.verbose) {
        Ok(pane_id) => {
            let _ = tmux::switch_to_pane(&pane_id, app.verbose);
            app.should_quit = true;
        }
        Err(e) => {
            app.status_message = Some(format!("new window failed: {e}"));
        }
    }
}

/// Look up the default command for an agent by name from the registry.
fn agent_command_for(name: &str) -> String {
    AGENT_REGISTRY
        .iter()
        .find(|d| d.display_name.eq_ignore_ascii_case(name))
        .map(|d| d.default_command.to_string())
        .unwrap_or_else(|| name.to_string())
}

/// Keep scroll_offset in sync with cursor position.
fn update_scroll(app: &mut App) {
    let visible_height = 20_usize;

    if app.tree.cursor < app.tree.scroll_offset {
        app.tree.scroll_offset = app.tree.cursor;
    } else if app.tree.cursor >= app.tree.scroll_offset + visible_height {
        app.tree.scroll_offset = app.tree.cursor - visible_height + 1;
    }
}
