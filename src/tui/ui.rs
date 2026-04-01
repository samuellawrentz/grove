use edtui::{EditorTheme, EditorView};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::Frame;

use super::app::{App, Focus, SidebarFocus};
use crate::agent::{AgentFilter, AgentState, AGENT_REGISTRY, TERMINAL_ICON};

/// Draw the TUI frame.
pub(crate) fn draw(f: &mut Frame, app: &mut App) {
    let bar_height = 1;
    let outer =
        Layout::vertical([Constraint::Min(0), Constraint::Length(bar_height)]).split(f.area());

    let panels = if app.show_notepad {
        Layout::horizontal([
            Constraint::Percentage(20),
            Constraint::Percentage(50),
            Constraint::Percentage(30),
        ])
        .split(outer[0])
    } else {
        Layout::horizontal([Constraint::Percentage(25), Constraint::Percentage(75)])
            .split(outer[0])
    };

    // Split sidebar into tree (top) and projects (bottom)
    let sidebar =
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).split(panels[0]);

    draw_tree(f, app, sidebar[0]);
    draw_projects(f, app, sidebar[1]);
    draw_preview(f, app, panels[1]);
    if app.show_notepad {
        draw_notepad(f, app, panels[2]);
    }
    draw_status_bar(f, app, outer[1]);

    // Draw prompt modal overlay on top of everything
    if let Some(ref input) = app.prompt_input {
        draw_prompt_modal(f, input);
    }
}

fn draw_tree(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = app.sidebar_focus == SidebarFocus::Tree;
    let border_color = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let pane_title = match &app.tree.agent_filter {
        AgentFilter::All => " Panes [all] ".to_string(),
        AgentFilter::AnyAgent => " Panes [agents] ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(pane_title.as_str());

    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.tree.groups.is_empty() {
        let empty = Paragraph::new("No tmux panes found")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(empty, inner);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut row_idx = 0;

    for group in &app.tree.groups {
        let arrow = if group.expanded { "▼" } else { "▶" };
        let group_has_matches = group
            .panes
            .iter()
            .any(|p| app.tree.pane_matches(p, &group.name));
        let header_style = if focused && row_idx == app.tree.cursor {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::DarkGray)
        } else if !group_has_matches {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::DarkGray)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(Span::styled(
            format!("{arrow} {}", group.name),
            header_style,
        )));
        row_idx += 1;

        if group.expanded {
            for pane in &group.panes {
                let (icon, icon_color) = match &pane.agent {
                    Some(info) => {
                        let def = AGENT_REGISTRY.iter().find(|d| d.kind == info.kind);
                        let icon = def.map(|d| d.icon).unwrap_or(TERMINAL_ICON);
                        let color = match info.state {
                            AgentState::Active => Color::Green,
                            AgentState::Waiting => Color::Yellow,
                            AgentState::NotRunning => Color::DarkGray,
                        };
                        (icon, color)
                    }
                    None => (TERMINAL_ICON, Color::DarkGray),
                };

                let basename = pane
                    .pane_info
                    .current_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(".");
                let label = format!("  {} [{}]", basename, pane.pane_info.current_command);

                let matches = app.tree.pane_matches(pane, &group.name);
                let style = if focused && row_idx == app.tree.cursor {
                    Style::default().bg(Color::DarkGray)
                } else if !matches {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };
                let icon_style = if focused && row_idx == app.tree.cursor {
                    Style::default().fg(icon_color).bg(Color::DarkGray)
                } else if !matches {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(icon_color)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {icon} "), icon_style),
                    Span::styled(label, style),
                ]));
                row_idx += 1;
            }
        }
    }

    // Apply scroll offset
    let visible_height = inner.height as usize;
    let start = app.tree.scroll_offset;
    let end = std::cmp::min(start + visible_height, lines.len());
    let visible_lines: Vec<Line> = if start < lines.len() {
        lines[start..end].to_vec()
    } else {
        Vec::new()
    };

    let tree_widget = Paragraph::new(visible_lines);
    f.render_widget(tree_widget, inner);
}

fn format_relative_time(last_seen: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let ts = chrono::NaiveDateTime::parse_from_str(last_seen, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|d| d.and_utc().timestamp() as u64)
        .unwrap_or(0);
    let delta = now.saturating_sub(ts);
    if delta < 60 {
        "now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

fn draw_projects(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = app.sidebar_focus == SidebarFocus::Projects;
    let border_color = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(" Projects ");

    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.projects.is_empty() {
        let empty = Paragraph::new("No projects yet")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(empty, inner);
        return;
    }

    // Collect active project paths from current tree groups
    let active_paths: std::collections::HashSet<String> = app
        .tree
        .groups
        .iter()
        .map(|g| g.path.to_string_lossy().to_string())
        .collect();

    let lines: Vec<Line> = app
        .projects
        .iter()
        .enumerate()
        .map(|(i, project)| {
            let is_active = active_paths.contains(&project.path.to_string_lossy().to_string());
            let time = format_relative_time(&project.last_seen);
            let selected = focused && i == app.projects_cursor;
            let style = if selected {
                Style::default().bg(Color::DarkGray)
            } else if is_active {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let dim = if selected {
                Style::default().fg(Color::DarkGray).bg(Color::DarkGray)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let short_path = crate::tui::tree::shorten_path(&project.path);
            let mut spans = vec![Span::styled(format!("  {}", project.name), style)];
            spans.push(Span::styled(format!("  {short_path}"), dim));
            spans.push(Span::styled(format!("  {time}"), dim));
            Line::from(spans)
        })
        .collect();

    let visible_height = inner.height as usize;
    let start = if app.projects_cursor >= visible_height {
        app.projects_cursor - visible_height + 1
    } else {
        0
    };
    let end = std::cmp::min(start + visible_height, lines.len());
    let visible_lines: Vec<Line> = if start < lines.len() {
        lines[start..end].to_vec()
    } else {
        Vec::new()
    };

    let widget = Paragraph::new(visible_lines);
    f.render_widget(widget, inner);
}

fn draw_preview(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let border_style = Style::default().fg(Color::DarkGray);

    let filter_label = match &app.tree.agent_filter {
        AgentFilter::All => " [all] ".to_string(),
        AgentFilter::AnyAgent => " [agents] ".to_string(),
    };
    let title = if app.diff_mode {
        " Git Diff ".to_string()
    } else if let Some(pane_id) = app.tree.selected_pane_id() {
        format!(" Preview{}-- {pane_id} ", filter_label)
    } else {
        format!(" Preview{}", filter_label)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner = block.inner(area);
    let text = if app.diff_mode {
        if let Some(ref ds) = app.diff_state {
            ratatui::text::Text::from(ds.render())
        } else {
            ratatui::text::Text::raw("No diff data")
        }
    } else {
        use ansi_to_tui::IntoText as _;
        app.preview_content
            .into_text()
            .unwrap_or_else(|_| ratatui::text::Text::raw(&app.preview_content))
    };
    let line_count = text.lines.len();
    let visible_height = inner.height as usize;
    let scroll = if app.diff_mode {
        if let Some(ref ds) = app.diff_state {
            // Keep cursor centered-ish in viewport
            ds.cursor.saturating_sub(visible_height / 2) as u16
        } else {
            0
        }
    } else {
        let max_scroll = if line_count > visible_height {
            (line_count - visible_height) as u16
        } else {
            0
        };
        max_scroll.saturating_sub(app.preview_scroll_up)
    };
    let preview = Paragraph::new(text).block(block).scroll((scroll, 0));
    f.render_widget(preview, area);
}

fn draw_notepad(f: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let focused = app.focus == Focus::Notepad;
    let project_name = std::path::Path::new(&app.notepad.project)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("notes");
    let title = if focused {
        let mode_label = format!("{:?}", app.notepad.editor.mode).to_uppercase();
        format!(" \u{270e} Notepad: {} [{}] ", project_name, mode_label)
    } else {
        format!(" \u{270e} Notepad: {} ", project_name)
    };

    let border_color = if focused { Color::Cyan } else { Color::DarkGray };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let cursor_style = if focused {
        Style::default().bg(Color::White).fg(Color::Black)
    } else {
        Style::default()
    };
    let theme = EditorTheme::default()
        .base(Style::default().fg(Color::White))
        .cursor_style(cursor_style)
        .line_numbers_style(Style::default().fg(Color::DarkGray));

    EditorView::new(&mut app.notepad.editor)
        .theme(theme)
        .wrap(true)
        .render(inner, f.buffer_mut());
}

fn draw_prompt_modal(f: &mut Frame, input: &str) {
    let area = f.area();
    let width = (area.width / 2).max(40).min(area.width.saturating_sub(4));
    // 2 for borders + content lines (at least 1)
    let inner_width = width.saturating_sub(2) as usize;
    let content_lines = if inner_width == 0 {
        1
    } else {
        ((input.len() + 1) / inner_width.max(1) + 1) as u16 // +1 for cursor
    };
    let height = (content_lines + 2).min(area.height.saturating_sub(2)); // +2 for borders
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let modal_area = ratatui::layout::Rect::new(x, y, width, height);

    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Send ");

    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);

    let text = format!("{input}_");
    let paragraph = Paragraph::new(text)
        .style(Style::default())
        .wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(paragraph, inner);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let content = if app.open_prompt_dir.is_some() {
        Line::from(vec![
            Span::styled("Open: ", Style::default().fg(Color::Cyan)),
            Span::styled("[c]", Style::default().fg(Color::White)),
            Span::styled("laude  ", Style::default().fg(Color::DarkGray)),
            Span::styled("[o]", Style::default().fg(Color::White)),
            Span::styled("pencode  ", Style::default().fg(Color::DarkGray)),
            Span::styled("code[x]", Style::default().fg(Color::White)),
            Span::styled("  ", Style::default().fg(Color::DarkGray)),
            Span::styled("c[u]", Style::default().fg(Color::White)),
            Span::styled("rsor  ", Style::default().fg(Color::DarkGray)),
            Span::styled("[t]", Style::default().fg(Color::White)),
            Span::styled("erminal  ", Style::default().fg(Color::DarkGray)),
            Span::styled("[e]", Style::default().fg(Color::White)),
            Span::styled("ditor  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc:cancel", Style::default().fg(Color::DarkGray)),
        ])
    } else if let Some(ref query) = app.search_input {
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(Color::Cyan)),
            Span::raw(query.as_str()),
            Span::styled("_", Style::default().fg(Color::Cyan)),
        ])
    } else if let Some(ref msg) = app.status_message {
        Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Yellow),
        ))
    } else {
        let hint = if app.focus == Focus::Notepad {
            "\u{270e} Notepad (vim) | m/Esc: unfocus | C-r: hide | v:select Enter:send to pane"
        } else if app.diff_mode {
            "j/k:nav  C-j/k:jump10  w:expand/collapse  d:close diff  q:quit"
        } else {
            match app.sidebar_focus {
                SidebarFocus::Tree => {
                    "j/k:nav  C-t:filter  /:search  Enter:switch  e:edit  d:diff  C-r:notepad m:focus  C:claude O:opencode X:codex U:cursor  T:term  a/r:accept/reject  s:send  o:open  q:quit"
                }
                SidebarFocus::Projects => {
                    "j/k:nav  C-h/C-l:pane  c/Enter:continue  n:new  t:terminal  m:notepad  x:remove  q:quit"
                }
            }
        };
        Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray)))
    };

    let bar = Paragraph::new(content).wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(bar, area);
}
