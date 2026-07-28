use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::app::{App, Overlay};

/// Draw the recents launcher: one list pane + a status/hint bar.
pub(crate) fn draw(f: &mut Frame, app: &App) {
    let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(f.area());
    draw_recents(f, app, outer[0]);
    draw_status_bar(f, app, outer[1]);
}

fn draw_recents(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Recents ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let indices = app.filtered_indices();
    if indices.is_empty() {
        let msg = if app.recents.is_empty() {
            "No recents yet"
        } else {
            "No matches"
        };
        let empty = Paragraph::new(msg)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        f.render_widget(empty, inner);
        return;
    }

    let cursor = app.cursor.min(indices.len() - 1);
    let lines: Vec<Line> = indices
        .iter()
        .enumerate()
        .map(|(display_idx, &real_idx)| {
            let project = &app.recents[real_idx];
            let selected = display_idx == cursor;
            let name_style = if selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };
            let dim = if selected {
                Style::default().fg(Color::Gray).bg(Color::DarkGray)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(vec![
                Span::styled(format!("  {}", project.name), name_style),
                Span::styled(format!("  {}", shorten_path(&project.path)), dim),
                Span::styled(
                    format!("  {}", format_relative_time(&project.last_seen)),
                    dim,
                ),
            ])
        })
        .collect();

    // Scroll so the cursor stays visible.
    let visible = inner.height as usize;
    let start = if cursor >= visible {
        cursor - visible + 1
    } else {
        0
    };
    let end = (start + visible).min(lines.len());
    let widget = Paragraph::new(lines[start..end].to_vec());
    f.render_widget(widget, inner);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let content = match app.overlay {
        Overlay::OpenChoice { .. } => Line::from(vec![
            Span::styled("Open: ", Style::default().fg(Color::Cyan)),
            Span::styled("[c]", Style::default().fg(Color::White)),
            Span::styled("laude  ", Style::default().fg(Color::DarkGray)),
            Span::styled("[t]", Style::default().fg(Color::White)),
            Span::styled("erminal  ", Style::default().fg(Color::DarkGray)),
            Span::styled("[e]", Style::default().fg(Color::White)),
            Span::styled("ditor  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc:cancel", Style::default().fg(Color::DarkGray)),
        ]),
        Overlay::Search(ref query) => Line::from(vec![
            Span::styled("/ ", Style::default().fg(Color::Cyan)),
            Span::raw(query.as_str()),
            Span::styled("_", Style::default().fg(Color::Cyan)),
        ]),
        Overlay::None => {
            if let Some(ref msg) = app.status_message {
                Line::from(Span::styled(
                    msg.as_str(),
                    Style::default().fg(Color::Yellow),
                ))
            } else {
                Line::from(Span::styled(
                    "j/k:nav  /:search  Enter:claude  o:open  n:new task  q:quit",
                    Style::default().fg(Color::DarkGray),
                ))
            }
        }
    };
    f.render_widget(Paragraph::new(content), area);
}

/// Home-relative, tail-truncated path for compact display.
fn shorten_path(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    let s = match std::env::var("HOME") {
        Ok(home) if s.starts_with(&home) => format!("~{}", &s[home.len()..]),
        _ => s.to_string(),
    };
    const MAX: usize = 48;
    if s.chars().count() > MAX {
        let tail: String = s
            .chars()
            .rev()
            .take(MAX - 1)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("…{tail}")
    } else {
        s
    }
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
