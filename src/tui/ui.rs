use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::app::{App, Focus};
use crate::claude::ClaudeState;

/// Draw the TUI frame.
pub(crate) fn draw(f: &mut Frame, app: &App) {
    let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(f.area());

    let panels = Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(outer[0]);

    draw_tree(f, app, panels[0]);
    draw_preview(f, app, panels[1]);
    draw_status_bar(f, app, outer[1]);
}

fn draw_tree(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = matches!(app.focus, Focus::Tree);
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
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
        let header_style = if row_idx == app.tree.cursor {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::DarkGray)
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

                let style = if row_idx == app.tree.cursor {
                    Style::default().bg(Color::DarkGray)
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

fn draw_preview(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = matches!(app.focus, Focus::Preview);
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = if let Some(pane_id) = app.tree.selected_pane_id() {
        format!(" Preview -- {pane_id} ")
    } else {
        " Preview ".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let preview = Paragraph::new(app.preview_content.as_str())
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(preview, area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let content = if let Some(ref input) = app.prompt_input {
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
        Line::from(Span::styled(
            "j/k:nav  h/l:fold  Tab:panel  Enter:switch  a:accept  r:reject  s:send  n:new  q:quit",
            Style::default().fg(Color::DarkGray),
        ))
    };

    let bar = Paragraph::new(content);
    f.render_widget(bar, area);
}
