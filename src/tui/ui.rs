use edtui::{EditorTheme, EditorView};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::Frame;

use super::app::{App, Focus, Overlay, SidebarFocus};
use crate::agent::{AgentFilter, AgentState, AGENT_REGISTRY, TERMINAL_ICON};

/// Draw the TUI frame.
pub(crate) fn draw(f: &mut Frame, app: &mut App) {
    let bar_height = 1;
    let outer =
        Layout::vertical([Constraint::Min(0), Constraint::Length(bar_height)]).split(f.area());

    let panels = if app.ui.show_notepad {
        Layout::horizontal([
            Constraint::Percentage(20),
            Constraint::Percentage(50),
            Constraint::Percentage(30),
        ])
        .split(outer[0])
    } else {
        Layout::horizontal([Constraint::Percentage(25), Constraint::Percentage(75)]).split(outer[0])
    };

    // Split sidebar into tree (top) and projects (bottom)
    let sidebar =
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).split(panels[0]);

    draw_tree(f, app, sidebar[0]);
    draw_projects(f, app, sidebar[1]);
    draw_preview(f, app, panels[1]);
    if app.ui.show_notepad {
        draw_notepad(f, app, panels[2]);
    }
    draw_status_bar(f, app, outer[1]);

    // Draw prompt modal overlay on top of everything
    if let Overlay::Prompt(ref input) = app.ui.overlay {
        draw_prompt_modal(f, input);
    }
}

fn draw_tree(f: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let focused = app.ui.sidebar_focus == SidebarFocus::Tree;
    let border_color = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let pane_title = match &app.tree.agent_filter {
        AgentFilter::AnyAgent => " Panes [agents] ".to_string(),
        AgentFilter::Others => " Panes [others] ".to_string(),
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

    // Build rendered lines from the compacted, filter-aware display rows so
    // cursor space and lines space cannot diverge under a search filter. The
    // highlighted row is the one whose display index equals the cursor's
    // display index (mapped via the same seam).
    let rows = app.tree.display_rows();
    let cursor_display = app.tree.cursor_display_index();
    let mut lines: Vec<Line> = Vec::with_capacity(rows.len());

    for (display_idx, row) in rows.iter().enumerate() {
        let selected = focused && Some(display_idx) == cursor_display;
        match row.kind {
            crate::tui::tree::DisplayKind::Group(gi) => {
                let group = &app.tree.groups[gi];
                let arrow = if group.expanded { "▼" } else { "▶" };
                let header_style = if selected {
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
            }
            crate::tui::tree::DisplayKind::Pane(gi, pi) => {
                let pane = &app.tree.groups[gi].panes[pi];
                let (icon, icon_color) = if pane.forced_other {
                    // User-marked as "other" (automation/script): pin icon.
                    ("󰐃", Color::Magenta)
                } else {
                    match &pane.agent {
                        Some(info) => {
                            let def = AGENT_REGISTRY.iter().find(|d| d.kind == info.kind);
                            let icon = def.map(|d| d.icon).unwrap_or(TERMINAL_ICON);
                            let color = match info.state {
                                AgentState::Active => Color::Green,
                                AgentState::Waiting => Color::Yellow,
                                AgentState::Idle => Color::Cyan,
                                AgentState::Unknown | AgentState::NotRunning => Color::DarkGray,
                            };
                            (icon, color)
                        }
                        None => (TERMINAL_ICON, Color::DarkGray),
                    }
                };

                let basename = pane
                    .pane_info
                    .current_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(".");
                let label = format!("  {} [{}]", basename, pane.pane_info.current_command);

                let style = if selected {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };
                let icon_style = if selected {
                    Style::default().fg(icon_color).bg(Color::DarkGray)
                } else {
                    Style::default().fg(icon_color)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {icon} "), icon_style),
                    Span::styled(label, style),
                ]));
            }
        }
    }

    // Apply scroll offset in display space, clamped so the cursor stays visible
    // and the offset never runs past the compacted line set.
    let visible_height = inner.height as usize;
    app.tree.clamp_scroll(visible_height);
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
    let focused = app.ui.sidebar_focus == SidebarFocus::Projects;
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

    let filtered_indices = app.filtered_project_indices();

    if filtered_indices.is_empty() {
        let msg = if app.projects.list.is_empty() {
            "No projects yet"
        } else {
            "No matches"
        };
        let empty = Paragraph::new(msg)
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

    let lines: Vec<Line> = filtered_indices
        .iter()
        .enumerate()
        .map(|(display_idx, &real_idx)| {
            let project = &app.projects.list[real_idx];
            let is_active = active_paths.contains(project.path.to_string_lossy().as_ref());
            let time = format_relative_time(&project.last_seen);
            let selected = focused && display_idx == app.projects.cursor;
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
    let start = if app.projects.cursor >= visible_height {
        app.projects.cursor - visible_height + 1
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
        AgentFilter::AnyAgent => " [agents] ".to_string(),
        AgentFilter::Others => " [others] ".to_string(),
    };
    let title = if app.preview.diff_mode {
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
    let text = if app.preview.diff_mode {
        if let Some(ref ds) = app.preview.diff_state {
            ratatui::text::Text::from(ds.render())
        } else {
            ratatui::text::Text::raw("No diff data")
        }
    } else {
        use ansi_to_tui::IntoText as _;
        app.preview
            .content
            .into_text()
            .unwrap_or_else(|_| ratatui::text::Text::raw(&app.preview.content))
    };
    let line_count = text.lines.len();
    let visible_height = inner.height as usize;
    let scroll = if app.preview.diff_mode {
        if let Some(ref ds) = app.preview.diff_state {
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
        max_scroll.saturating_sub(app.preview.scroll_up)
    };
    let preview = Paragraph::new(text).block(block).scroll((scroll, 0));
    f.render_widget(preview, area);
}

fn draw_notepad(f: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let focused = app.ui.focus == Focus::Notepad;
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

    let border_color = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
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
    let content = match app.ui.overlay {
        Overlay::OpenChoice { .. } => Line::from(vec![
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
        ]),
        Overlay::Search { ref query, .. } => Line::from(vec![
            Span::styled("/ ", Style::default().fg(Color::Cyan)),
            Span::raw(query.as_str()),
            Span::styled("_", Style::default().fg(Color::Cyan)),
        ]),
        Overlay::Prompt(_) | Overlay::None => status_or_hint(app),
    };

    let bar = Paragraph::new(content).wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(bar, area);
}

/// The non-overlay status bar content: a transient status message, else the
/// context-sensitive key hints.
fn status_or_hint(app: &App) -> Line<'_> {
    if let Some(ref msg) = app.ui.status_message {
        Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Yellow),
        ))
    } else {
        let hint = if app.ui.focus == Focus::Notepad {
            "\u{270e} Notepad (vim) | m/Esc: unfocus | C-r: hide | v:select Enter:send to pane"
        } else if app.preview.diff_mode {
            "j/k:nav  C-j/k:jump10  w:expand/collapse  d:close diff  q:quit"
        } else {
            match app.ui.sidebar_focus {
                SidebarFocus::Tree => {
                    "j/k:nav  C-t:filter  /:search  Enter:switch  e:edit  d:diff  C-r:notepad m:focus  C:claude O:opencode X:codex U:cursor  T:term  M:mark-other  a/r:accept/reject  s:send  o:open  q:quit"
                }
                SidebarFocus::Projects => {
                    "j/k:nav  C-h/C-l:pane  c/Enter:continue  n:new  t:terminal  m:notepad  x:remove  q:quit"
                }
            }
        };
        Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray)))
    }
}
