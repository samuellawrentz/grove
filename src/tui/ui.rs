use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use super::app::{App, SidebarFocus};
use crate::agent::{AgentFilter, AgentState, AGENT_REGISTRY};
use crate::recents;

/// Draw the TUI frame.
pub(crate) fn draw(f: &mut Frame, app: &App) {
    let bar_height = 1;
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
        AgentFilter::Specific(kind) => {
            let name = AGENT_REGISTRY
                .iter()
                .find(|d| d.kind == *kind)
                .map(|d| d.display_name)
                .unwrap_or("?");
            format!(" Panes [{name}] ")
        }
        AgentFilter::NonAgent => " Panes [other] ".to_string(),
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
                let icon = match &pane.agent {
                    Some(info) => {
                        let def = AGENT_REGISTRY.iter().find(|d| d.kind == info.kind);
                        match (&info.state, def) {
                            (AgentState::Active, Some(d)) => d.icon_active,
                            (AgentState::Waiting, Some(d)) => d.icon_waiting,
                            (AgentState::NotRunning, Some(d)) => d.icon_not_running,
                            _ => "○",
                        }
                    }
                    None => "·",
                };

                let basename = pane
                    .pane_info
                    .current_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(".");
                let label = format!("  {icon} {} [{}]", basename, pane.pane_info.current_command,);

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
            let parent = entry
                .path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string());
            let time = recents::format_relative_time(entry.timestamp);
            let selected = focused && i == app.recents_cursor;
            let style = if selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };
            let dim = if selected {
                Style::default().fg(Color::DarkGray).bg(Color::DarkGray)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let mut spans = vec![Span::styled(format!("  {name}"), style)];
            if let Some(parent) = parent {
                spans.push(Span::styled(format!("  {parent}/"), dim));
            }
            spans.push(Span::styled(format!("  {time}"), dim));
            Line::from(spans)
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

    let filter_label = match &app.tree.agent_filter {
        AgentFilter::All => " [all] ".to_string(),
        AgentFilter::AnyAgent => " [agents] ".to_string(),
        AgentFilter::Specific(kind) => {
            let name = AGENT_REGISTRY
                .iter()
                .find(|d| d.kind == *kind)
                .map(|d| d.display_name)
                .unwrap_or("?");
            format!(" [{name}] ")
        }
        AgentFilter::NonAgent => " [other] ".to_string(),
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
        let hint = if app.diff_mode {
            "j/k:nav  C-j/k:jump10  w:expand/collapse  d:close diff  q:quit"
        } else {
            match app.sidebar_focus {
                SidebarFocus::Tree => {
                    "j/k:nav  C-t:filter  /:search  Enter:switch  e:edit  d:diff  C:claude O:opencode X:codex U:cursor  T:term  a/r:accept/reject  s:send  o:open  q:quit"
                }
                SidebarFocus::Recents => {
                    "j/k:nav  C-h/C-l:pane  C-t:filter  d:diff  c/Enter:continue  n:new  t:terminal  o:open  x:remove  q:quit"
                }
            }
        };
        Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray)))
    };

    let bar = Paragraph::new(content).wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(bar, area);
}
