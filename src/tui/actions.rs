use crossterm::event::KeyEvent;

use edtui::{EditorEventHandler, EditorMode};

use crate::agent::{AgentFilter, AgentState, AGENT_REGISTRY};
use crate::tmux;

use super::app::{App, Focus, Overlay, PendingShell, SidebarFocus};
use super::flat_rows::FlatRows;

/// Resolve the launch command for an agent `launch_key` (e.g. 'c','o','x','u')
/// via the registry + config overrides. Returns `None` for unknown keys.
fn launch_for_key(app: &App, key: char) -> Option<String> {
    AGENT_REGISTRY
        .iter()
        .find(|d| d.launch_key == key)
        .map(|d| app.config.resolved_agent_command(d.wire_name))
}

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

    // Notepad focused: pass keys to edtui, Esc/q in normal mode returns focus to sidebar
    if app.focus == Focus::Notepad {
        let note = &mut app.notepad;
        enum NoteAction {
            Unfocus,
            Send(String),
            Forward,
        }
        let action = match (&note.editor.mode, key.code) {
            (EditorMode::Normal, KeyCode::Esc) | (EditorMode::Normal, KeyCode::Char('q')) => {
                NoteAction::Unfocus
            }
            (EditorMode::Visual, KeyCode::Enter) => {
                let text = note
                    .editor
                    .selection
                    .as_ref()
                    .map(|sel| sel.copy_from(&note.editor.lines).to_string())
                    .unwrap_or_default();
                note.editor.mode = EditorMode::Normal;
                note.editor.selection = None;
                NoteAction::Send(text)
            }
            _ => NoteAction::Forward,
        };
        match action {
            NoteAction::Unfocus => {
                app.focus = Focus::Sidebar;
                app.save_note();
            }
            NoteAction::Send(text) => {
                if !text.is_empty() {
                    if let Some(pane_id) = app.tree.selected_pane_id().map(|s| s.to_string()) {
                        let _ = tmux::send_keys(&pane_id, &text, app.verbose);
                        app.status_message = Some("Sent to pane".to_string());
                    }
                }
            }
            NoteAction::Forward => {
                EditorEventHandler::default().on_key_event(key, &mut app.notepad.editor);
            }
        }
        return;
    }

    // Input-capture overlays: each is matched in exactly one place.
    match std::mem::replace(&mut app.overlay, Overlay::None) {
        Overlay::Search { mut query, target } => {
            handle_search_key(app, key, &mut query, target);
            return;
        }
        Overlay::Prompt(mut input) => {
            handle_prompt_key(app, key, &mut input);
            return;
        }
        Overlay::OpenChoice { dir } => {
            handle_open_choice_key(app, key, dir);
            return;
        }
        Overlay::None => {}
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
            app.sidebar_focus = SidebarFocus::Projects;
            return;
        }
        KeyCode::Char('j') if app.diff_mode && key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut ds) = app.diff_state {
                ds.move_down_by(10);
            }
            return;
        }
        KeyCode::Char('k') if app.diff_mode && key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut ds) = app.diff_state {
                ds.move_up_by(10);
            }
            return;
        }
        KeyCode::Char('j') | KeyCode::Char('J') if app.diff_mode => {
            if let Some(ref mut ds) = app.diff_state {
                ds.move_down();
            }
            return;
        }
        KeyCode::Char('k') | KeyCode::Char('K') if app.diff_mode => {
            if let Some(ref mut ds) = app.diff_state {
                ds.move_up();
            }
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
        KeyCode::Char('w') if app.diff_mode => {
            if let Some(ref mut ds) = app.diff_state {
                ds.toggle_expand();
            }
            return;
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.tree.agent_filter = match &app.tree.agent_filter {
                AgentFilter::AnyAgent => AgentFilter::Others,
                AgentFilter::Others => AgentFilter::AnyAgent,
            };
            app.tree.jump_first_pane();
            update_scroll(app);
            app.refresh_preview();
            return;
        }
        KeyCode::Char('m') => {
            if app.show_notepad {
                app.focus = if app.focus == Focus::Notepad {
                    app.save_note();
                    Focus::Sidebar
                } else {
                    Focus::Notepad
                };
            }
            return;
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.show_notepad = !app.show_notepad;
            if app.show_notepad {
                app.focus = Focus::Notepad;
            } else {
                app.focus = Focus::Sidebar;
                app.save_note();
            }
            return;
        }
        KeyCode::Char('d') => {
            app.diff_mode = !app.diff_mode;
            app.preview_scroll_up = 0;
            app.refresh_preview();
            return;
        }
        KeyCode::Char('/') => {
            app.overlay = Overlay::Search {
                query: String::new(),
                target: app.sidebar_focus,
            };
            return;
        }
        KeyCode::Char('o') => {
            app.pending_shell = PendingShell::FzfPicker;
            return;
        }
        _ => {}
    }

    // Dispatch to focused pane
    match app.sidebar_focus {
        SidebarFocus::Tree => handle_tree_key(app, key),
        SidebarFocus::Projects => handle_projects_key(app, key),
    }
}

/// Handle a key while the Search overlay is active. On exit the overlay stays
/// `None`; otherwise it is re-armed with the (possibly mutated) query.
fn handle_search_key(app: &mut App, key: KeyEvent, query: &mut String, target: SidebarFocus) {
    use crossterm::event::KeyCode;

    match target {
        SidebarFocus::Tree => match key.code {
            KeyCode::Enter => {
                if let Some(pane_id) = app.tree.selected_pane_id().map(|s| s.to_string()) {
                    let _ = tmux::switch_to_pane(&pane_id, app.verbose);
                    app.should_quit = app.popup;
                }
                app.tree.search_filter = None;
            }
            KeyCode::Esc => {
                app.tree.search_filter = None;
            }
            KeyCode::Down => {
                app.tree.move_cursor_to_pane(true);
                update_scroll(app);
                app.refresh_preview();
                rearm_search(app, query, target);
            }
            KeyCode::Up => {
                app.tree.move_cursor_to_pane(false);
                update_scroll(app);
                app.refresh_preview();
                rearm_search(app, query, target);
            }
            KeyCode::Char(c) => {
                query.push(c);
                app.tree.search_filter = Some(query.clone());
                app.tree.jump_first_pane();
                update_scroll(app);
                app.refresh_preview();
                rearm_search(app, query, target);
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
                rearm_search(app, query, target);
            }
            _ => rearm_search(app, query, target),
        },
        SidebarFocus::Projects => match key.code {
            KeyCode::Enter => {
                let indices = app.filtered_project_indices();
                if !indices.is_empty() {
                    let real_idx = indices[app.projects_cursor.min(indices.len() - 1)];
                    let dir = app.projects[real_idx].path.to_string_lossy().to_string();
                    let cmd = format!("{} -c", app.default_agent_command);
                    app.projects_search_filter = None;
                    launch_in_new_window(app, &dir, Some(&cmd));
                }
                app.projects_search_filter = None;
            }
            KeyCode::Esc => {
                app.projects_search_filter = None;
            }
            KeyCode::Down => {
                let max = app.filtered_project_indices().len();
                if app.projects_cursor + 1 < max {
                    app.projects_cursor += 1;
                }
                rearm_search(app, query, target);
            }
            KeyCode::Up => {
                app.projects_cursor = app.projects_cursor.saturating_sub(1);
                rearm_search(app, query, target);
            }
            KeyCode::Char(c) => {
                query.push(c);
                app.projects_search_filter = Some(query.clone());
                app.projects_cursor = 0;
                rearm_search(app, query, target);
            }
            KeyCode::Backspace => {
                query.pop();
                app.projects_search_filter = if query.is_empty() {
                    None
                } else {
                    Some(query.clone())
                };
                app.projects_cursor = 0;
                rearm_search(app, query, target);
            }
            _ => rearm_search(app, query, target),
        },
    }
}

/// Re-arm the Search overlay with the current query/target (it was taken out
/// by the `mem::replace` in `handle_key`).
fn rearm_search(app: &mut App, query: &str, target: SidebarFocus) {
    app.overlay = Overlay::Search {
        query: query.to_string(),
        target,
    };
}

/// Handle a key while the Prompt overlay is active.
fn handle_prompt_key(app: &mut App, key: KeyEvent, input: &mut String) {
    use crossterm::event::KeyCode;

    match key.code {
        KeyCode::Enter => {
            let text = input.clone();
            if let Some(pane_id) = app.tree.selected_pane_id().map(|s| s.to_string()) {
                if !text.is_empty() {
                    let _ = tmux::send_keys(&pane_id, &text, app.verbose);
                    app.refresh_tree();
                }
            }
        }
        KeyCode::Esc => {}
        KeyCode::Char(c) => {
            input.push(c);
            app.overlay = Overlay::Prompt(input.clone());
        }
        KeyCode::Backspace => {
            input.pop();
            app.overlay = Overlay::Prompt(input.clone());
        }
        _ => {
            app.overlay = Overlay::Prompt(input.clone());
        }
    }
}

/// Handle a key in the open-prompt sub-choice (a directory was picked by fzf,
/// now choosing what to launch in it).
fn handle_open_choice_key(app: &mut App, key: KeyEvent, dir: String) {
    use crossterm::event::KeyCode;

    match key.code {
        KeyCode::Char(c @ ('c' | 'o' | 'x' | 'u')) => {
            if let Some(cmd) = launch_for_key(app, c) {
                launch_in_new_window(app, &dir, Some(&cmd));
            }
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
            // unrecognized key: keep the overlay open awaiting a valid choice
            app.overlay = Overlay::OpenChoice { dir };
        }
    }
}

fn handle_tree_key(app: &mut App, key: KeyEvent) {
    use crossterm::event::KeyCode;

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            app.tree.move_cursor_to_pane(true);
            update_scroll(app);
            app.preview_scroll_up = 0;
            app.sync_note_to_group();
            app.refresh_preview();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.tree.move_cursor_to_pane(false);
            update_scroll(app);
            app.preview_scroll_up = 0;
            app.sync_note_to_group();
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
                app.should_quit = app.popup;
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
        KeyCode::Char('s') if app.tree.selected_pane().is_some() => {
            app.overlay = Overlay::Prompt(String::new());
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
            app.pending_shell = PendingShell::Popup("grove init -i".to_string());
        }
        KeyCode::Char('e') => {
            if let Some((target, cwd)) = selected_target_cwd(app) {
                launch_split(app, &target, &cwd, Some("nvim ."));
            }
        }
        // Split-launch an agent: C/O/X/U map to the registry launch_key (lowercased).
        KeyCode::Char(c @ ('C' | 'O' | 'X' | 'U')) => {
            if let Some(cmd) = launch_for_key(app, c.to_ascii_lowercase()) {
                if let Some((target, cwd)) = selected_target_cwd(app) {
                    launch_split(app, &target, &cwd, Some(&cmd));
                }
            }
        }
        KeyCode::Char('T') => {
            if let Some((target, cwd)) = selected_target_cwd(app) {
                launch_split(app, &target, &cwd, None);
            }
        }
        KeyCode::Char('M') => {
            if let Some(pane) = app.tree.selected_pane() {
                let pane_id = pane.pane_info.pane_id.clone();
                let result = if pane.forced_other {
                    app.db.unmark_pane_other(&pane_id)
                } else {
                    app.db.mark_pane_other(&pane_id)
                };
                if let Err(e) = result {
                    app.status_message = Some(format!("Mark error: {e}"));
                }
                app.refresh_tree();
                app.refresh_preview();
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

fn handle_projects_key(app: &mut App, key: KeyEvent) {
    use crossterm::event::KeyCode;

    let indices = app.filtered_project_indices();
    if indices.is_empty() {
        return;
    }

    // Resolve the real project index from the display cursor
    let real_idx = |app: &App| {
        let idxs = app.filtered_project_indices();
        if idxs.is_empty() {
            None
        } else {
            Some(idxs[app.projects_cursor.min(idxs.len() - 1)])
        }
    };

    match key.code {
        KeyCode::Char('j') | KeyCode::Down if app.projects_cursor + 1 < indices.len() => {
            app.projects_cursor += 1;
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.projects_cursor = app.projects_cursor.saturating_sub(1);
        }
        KeyCode::Char('c') | KeyCode::Enter => {
            if let Some(idx) = real_idx(app) {
                let dir = app.projects[idx].path.to_string_lossy().to_string();
                let cmd = format!("{} -c", app.default_agent_command);
                launch_in_new_window(app, &dir, Some(&cmd));
            }
        }
        KeyCode::Char('n') => {
            if let Some(idx) = real_idx(app) {
                let dir = app.projects[idx].path.to_string_lossy().to_string();
                let cmd = app.default_agent_command.clone();
                launch_in_new_window(app, &dir, Some(&cmd));
            }
        }
        KeyCode::Char('t') => {
            if let Some(idx) = real_idx(app) {
                let dir = app.projects[idx].path.to_string_lossy().to_string();
                launch_in_new_window(app, &dir, None);
            }
        }
        KeyCode::Char('x') => {
            if let Some(idx) = real_idx(app) {
                let path = app.projects[idx].path.to_string_lossy().to_string();
                let _ = app.db.delete_project(&path);
                app.refresh_projects();
                let new_len = app.filtered_project_indices().len();
                if app.projects_cursor >= new_len {
                    app.projects_cursor = new_len.saturating_sub(1);
                }
            }
        }
        KeyCode::Char('g') => {
            app.projects_cursor = 0;
        }
        KeyCode::Char('G') => {
            app.projects_cursor = indices.len().saturating_sub(1);
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
    } else if app.sidebar_focus == SidebarFocus::Projects {
        let indices = app.filtered_project_indices();
        indices
            .get(app.projects_cursor)
            .and_then(|&idx| app.projects.get(idx))
            .map(|proj| {
                let path = proj.path.to_string_lossy().to_string();
                ("new-window".to_string(), path)
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
            app.should_quit = app.popup;
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
            app.should_quit = app.popup;
        }
        Err(e) => {
            app.status_message = Some(format!("new window failed: {e}"));
        }
    }
}

/// Keep scroll_offset in sync with cursor position.
fn update_scroll(app: &mut App) {
    let visible_height = crossterm::terminal::size()
        .map(|(_, h)| (h as usize).saturating_sub(4))
        .unwrap_or(20);

    if app.tree.cursor < app.tree.scroll_offset {
        app.tree.scroll_offset = app.tree.cursor;
    } else if app.tree.cursor >= app.tree.scroll_offset + visible_height {
        app.tree.scroll_offset = app.tree.cursor - visible_height + 1;
    }
}
