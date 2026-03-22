use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::app::{App, SidebarFocus};
use crate::claude::ClaudeState;
use crate::recents;

/// Draw the TUI frame.
pub(crate) fn draw(f: &mut Frame, app: &App) {
    let has_input = app.prompt_input.is_some() || app.search_input.is_some();
    let bar_height = if has_input { 3 } else { 1 };
    let outer =
        Layout::vertical([Constraint::Min(0), Constraint::Length(bar_height)]).split(f.area());

    let panels = Layout::horizontal([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(outer[0]);

    // Split sidebar into tree (top) and recents (bottom)
    let sidebar =
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).split(panels[0]);

    draw_tree(f, app, sidebar[0]);
    draw_recents(f, app, sidebar[1]);
    draw_preview(f, app, panels[1]);
    draw_status_bar(f, app, outer[1]);
}

fn draw_tree(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = app.sidebar_focus == SidebarFocus::Tree;
    let border_color = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(" Panes ");

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
                let icon = match &pane.claude_state {
                    Some(ClaudeState::Waiting) => "◉",
                    Some(ClaudeState::Active) => "●",
                    Some(ClaudeState::NotRunning) => "○",
                    None => "·",
                };

                let label = format!(
                    "  {icon} {}:{} [{}]",
                    pane.pane_info.session_name,
                    pane.pane_info.window_index,
                    pane.pane_info.current_command,
                );

                let matches = app.tree.pane_matches(pane, &group.name);
                let style = if focused && row_idx == app.tree.cursor {
                    Style::default().bg(Color::DarkGray)
                } else if !matches {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(label, style)));
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

fn draw_recents(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = app.sidebar_focus == SidebarFocus::Recents;
    let border_color = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(" Recents ");

    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.recents.is_empty() {
        let empty = Paragraph::new("No recent sessions")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(empty, inner);
        return;
    }

    let lines: Vec<Line> = app
        .recents
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let name = entry
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| entry.path.to_string_lossy().to_string());
            let time = recents::format_relative_time(entry.timestamp);
            let label = format!("  {name}  {time}");
            let style = if focused && i == app.recents_cursor {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };
            Line::from(Span::styled(label, style))
        })
        .collect();

    let visible_height = inner.height as usize;
    let start = if app.recents_cursor >= visible_height {
        app.recents_cursor - visible_height + 1
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

    let title = if let Some(pane_id) = app.tree.selected_pane_id() {
        format!(" Preview -- {pane_id} ")
    } else {
        " Preview ".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner = block.inner(area);
    use ansi_to_tui::IntoText as _;
    let text = app
        .preview_content
        .into_text()
        .unwrap_or_else(|_| ratatui::text::Text::raw(&app.preview_content));
    let line_count = text.lines.len();
    let visible_height = inner.height as usize;
    let max_scroll = if line_count > visible_height {
        (line_count - visible_height) as u16
    } else {
        0
    };
    let scroll = max_scroll.saturating_sub(app.preview_scroll_up);
    let preview = Paragraph::new(text).block(block).scroll((scroll, 0));
    f.render_widget(preview, area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let content = if let Some(ref query) = app.search_input {
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(Color::Cyan)),
            Span::raw(query.as_str()),
            Span::styled("_", Style::default().fg(Color::Cyan)),
        ])
    } else if let Some(ref input) = app.prompt_input {
        Line::from(vec![
            Span::styled("Send: ", Style::default().fg(Color::Cyan)),
            Span::raw(input.as_str()),
            Span::styled("_", Style::default().fg(Color::Cyan)),
        ])
    } else if let Some(ref msg) = app.status_message {
        Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Yellow),
        ))
    } else {
        let hint = match app.sidebar_focus {
            SidebarFocus::Tree => {
                "j/k:nav  C-h/C-l:pane  H/L:fold  /:search  Enter:switch  e:edit  C:claude  T:terminal  a/r:accept/reject  s:send  n:new  N:open  t:term  x:close  q:quit"
            }
            SidebarFocus::Recents => {
                "j/k:nav  C-h/C-l:pane  c/Enter:continue  n:new session  t:term  N:open  x:remove  q:quit"
            }
        };
        Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray)))
    };

    let bar = Paragraph::new(content).wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(bar, area);
}
