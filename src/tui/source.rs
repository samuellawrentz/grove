use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::agent::{self, AgentState};
use crate::error::GroveError;
use crate::tmux::{self, PaneInfo};

/// Fetch all tmux panes.
pub(crate) fn fetch_panes(verbose: bool) -> Result<Vec<PaneInfo>, GroveError> {
    tmux::list_all_panes(verbose)
}

/// Fetch agent states from the external hook state file.
pub(crate) fn fetch_agent_states() -> Result<HashMap<String, AgentState>, GroveError> {
    agent::read_state_file()
}

/// Capture the visible content of a tmux pane.
pub(crate) fn fetch_preview(pane_id: &str, verbose: bool) -> Result<String, GroveError> {
    tmux::capture_pane(pane_id, verbose)
}

/// Fetch a directory listing for preview when cursor is on a group header.
/// Returns directories first (with `/` suffix), then files, sorted alphabetically.
pub(crate) fn fetch_directory_listing(path: &std::path::Path) -> Result<String, GroveError> {
    let entries = std::fs::read_dir(path)
        .map_err(|e| GroveError::General(format!("read_dir failed: {e}")))?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden files
        if name.starts_with('.') {
            continue;
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            dirs.push(format!("{name}/"));
        } else {
            files.push(name);
        }
    }

    dirs.sort();
    files.sort();

    let mut output = String::with_capacity((dirs.len() + files.len()) * 20);
    for d in &dirs {
        output.push_str(d);
        output.push('\n');
    }
    for f in &files {
        output.push_str(f);
        output.push('\n');
    }

    Ok(output)
}

const MAX_REPOS: usize = 5;
const MAX_LINES_PER_FILE: usize = 500;

/// A single diff line with its type.
#[derive(Clone)]
pub(crate) enum DiffLineKind {
    Added,
    Removed,
    Context,
    HunkHeader,
}

/// A parsed diff line with line numbers.
#[derive(Clone)]
pub(crate) struct DiffLine {
    pub kind: DiffLineKind,
    pub source_line: Option<usize>,
    pub target_line: Option<usize>,
    pub content: String,
}

/// A changed file with its diff content.
#[derive(Clone)]
pub(crate) struct DiffFile {
    pub name: String,
    pub added: usize,
    pub removed: usize,
    pub kind: char, // '+' new, '-' deleted, '~' modified
    pub lines: Vec<DiffLine>,
}

/// A repo's diff data.
#[derive(Clone)]
pub(crate) struct RepoDiff {
    pub path: String,
    pub files: Vec<DiffFile>,
}

/// Interactive diff state with per-file expand/collapse and cursor.
pub(crate) struct DiffState {
    pub repos: Vec<RepoDiff>,
    pub expanded: Vec<Vec<bool>>, // [repo_idx][file_idx]
    pub cursor: usize,            // flat row index
}

impl DiffState {
    pub fn new(repos: Vec<RepoDiff>) -> Self {
        let mut expanded: Vec<Vec<bool>> =
            repos.iter().map(|r| vec![false; r.files.len()]).collect();
        // Auto-expand first file
        for repo_expanded in &mut expanded {
            if !repo_expanded.is_empty() {
                repo_expanded[0] = true;
                break;
            }
        }
        DiffState {
            repos,
            expanded,
            cursor: 0,
        }
    }

    /// Update repo data while preserving cursor and expanded state.
    pub fn update(&mut self, repos: Vec<RepoDiff>) {
        // Rebuild expanded, preserving old state where file counts match
        let expanded: Vec<Vec<bool>> = repos
            .iter()
            .enumerate()
            .map(|(ri, r)| {
                if ri < self.expanded.len() && self.expanded[ri].len() == r.files.len() {
                    self.expanded[ri].clone()
                } else {
                    vec![false; r.files.len()]
                }
            })
            .collect();
        self.repos = repos;
        self.expanded = expanded;
        // Clamp cursor
        let total = self.total_rows();
        if total > 0 && self.cursor >= total {
            self.cursor = total - 1;
        }
    }

    /// Total visible rows.
    pub fn total_rows(&self) -> usize {
        let mut count = 0;
        for (ri, repo) in self.repos.iter().enumerate() {
            count += 1; // repo header
            if repo.files.is_empty() {
                count += 1; // "No changes"
            }
            for (fi, file) in repo.files.iter().enumerate() {
                count += 1; // file header
                if self.expanded[ri][fi] {
                    count += file.lines.len();
                }
            }
        }
        count
    }

    /// Map flat cursor to (repo_idx, file_idx) if cursor is on a file header.
    pub fn cursor_file(&self) -> Option<(usize, usize)> {
        let mut row = 0;
        for (ri, repo) in self.repos.iter().enumerate() {
            if row == self.cursor {
                return None; // on repo header
            }
            row += 1;
            if repo.files.is_empty() {
                row += 1;
                continue;
            }
            for (fi, file) in repo.files.iter().enumerate() {
                if row == self.cursor {
                    return Some((ri, fi));
                }
                row += 1;
                if self.expanded[ri][fi] {
                    row += file.lines.len();
                }
            }
        }
        None
    }

    /// Toggle expand/collapse on the file under cursor.
    pub fn toggle_expand(&mut self) {
        if let Some((ri, fi)) = self.cursor_file() {
            self.expanded[ri][fi] = !self.expanded[ri][fi];
        }
    }

    /// Move cursor down by n rows, auto-expanding files when landing on them.
    pub fn move_down_by(&mut self, n: usize) {
        let total = self.total_rows();
        for _ in 0..n {
            if self.cursor + 1 >= total {
                break;
            }
            self.cursor += 1;
            // Recalculate total since expanding changes row count
            self.auto_expand_at_cursor();
        }
    }

    /// Move cursor down.
    pub fn move_down(&mut self) {
        self.move_down_by(1);
    }

    /// Move cursor up by n rows.
    pub fn move_up_by(&mut self, n: usize) {
        for _ in 0..n {
            if self.cursor == 0 {
                break;
            }
            self.cursor -= 1;
            self.auto_expand_at_cursor();
        }
    }

    /// Move cursor up.
    pub fn move_up(&mut self) {
        self.move_up_by(1);
    }

    /// If cursor is on a file header, expand it (and collapse the previous one).
    fn auto_expand_at_cursor(&mut self) {
        if let Some((ri, fi)) = self.cursor_file() {
            if !self.expanded[ri][fi] {
                // Collapse all other files
                for (r, repo_exp) in self.expanded.iter_mut().enumerate() {
                    for (f, exp) in repo_exp.iter_mut().enumerate() {
                        if r != ri || f != fi {
                            *exp = false;
                        }
                    }
                }
                // Recalculate cursor position after collapsing
                self.cursor = self.row_for_file(ri, fi);
                self.expanded[ri][fi] = true;
            }
        }
    }

    /// Get the flat row index for a specific file header.
    fn row_for_file(&self, target_ri: usize, target_fi: usize) -> usize {
        let mut row = 0;
        for (ri, repo) in self.repos.iter().enumerate() {
            row += 1; // repo header
            if repo.files.is_empty() {
                row += 1;
                continue;
            }
            for (fi, file) in repo.files.iter().enumerate() {
                if ri == target_ri && fi == target_fi {
                    return row;
                }
                row += 1;
                if self.expanded[ri][fi] {
                    row += file.lines.len();
                }
            }
        }
        row
    }

    /// Render to styled ratatui lines.
    pub fn render(&self) -> Vec<Line<'static>> {
        let style_repo = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let style_file = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let style_file_sel = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
            .bg(Color::DarkGray);
        let style_stats = Style::default().fg(Color::DarkGray);
        let style_hunk = Style::default().fg(Color::Cyan);
        let style_add = Style::default().fg(Color::Green);
        let style_del = Style::default().fg(Color::Red);
        let style_ctx = Style::default().fg(Color::DarkGray);
        let style_empty = Style::default().fg(Color::DarkGray);

        let mut lines: Vec<Line<'static>> = Vec::with_capacity(self.total_rows());
        let mut row = 0;

        for (ri, repo) in self.repos.iter().enumerate() {
            let repo_style = if row == self.cursor {
                style_repo.bg(Color::DarkGray)
            } else {
                style_repo
            };
            lines.push(Line::from(Span::styled(
                format!("━━━ {} ━━━", repo.path),
                repo_style,
            )));
            row += 1;

            if repo.files.is_empty() {
                lines.push(Line::from(Span::styled("  No changes", style_empty)));
                row += 1;
                continue;
            }

            for (fi, file) in repo.files.iter().enumerate() {
                let is_expanded = self.expanded[ri][fi];
                let arrow = if is_expanded { "▼" } else { "▶" };
                let is_selected = row == self.cursor;
                let fs = if is_selected {
                    style_file_sel
                } else {
                    style_file
                };
                let ss = if is_selected {
                    style_stats.bg(Color::DarkGray)
                } else {
                    style_stats
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("  {arrow} {}{}", file.kind, file.name), fs),
                    Span::styled(format!(" (+{} -{})", file.added, file.removed), ss),
                ]));
                row += 1;

                if is_expanded {
                    for (line_count, dl) in file.lines.iter().enumerate() {
                        if line_count >= MAX_LINES_PER_FILE {
                            lines.push(Line::from(Span::styled("    ... truncated", style_empty)));
                            row += 1;
                            break;
                        }
                        let on_cursor = row == self.cursor;
                        let bg = if on_cursor {
                            Some(Color::DarkGray)
                        } else {
                            None
                        };

                        let (base_style, prefix) = match &dl.kind {
                            DiffLineKind::Added => (style_add, "+"),
                            DiffLineKind::Removed => (style_del, "-"),
                            DiffLineKind::Context => (style_ctx, " "),
                            DiffLineKind::HunkHeader => (style_hunk, ""),
                        };

                        // Line number gutter
                        let lineno = match &dl.kind {
                            DiffLineKind::Removed => dl
                                .source_line
                                .map(|n| format!("{:>4}      ", n))
                                .unwrap_or_else(|| "          ".to_string()),
                            DiffLineKind::Added => dl
                                .target_line
                                .map(|n| format!("     {:>4} ", n))
                                .unwrap_or_else(|| "          ".to_string()),
                            DiffLineKind::Context => {
                                let s = dl
                                    .source_line
                                    .map(|n| format!("{:>4}", n))
                                    .unwrap_or_else(|| "    ".to_string());
                                let t = dl
                                    .target_line
                                    .map(|n| format!("{:>4}", n))
                                    .unwrap_or_else(|| "    ".to_string());
                                format!("{} {} ", s, t)
                            }
                            DiffLineKind::HunkHeader => "          ".to_string(),
                        };

                        let mut spans: Vec<Span<'static>> = Vec::new();
                        let gutter_style = if let Some(c) = bg {
                            Style::default().fg(Color::DarkGray).bg(c)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };
                        spans.push(Span::styled(lineno, gutter_style));

                        let s = if let Some(c) = bg {
                            base_style.bg(c)
                        } else {
                            base_style
                        };
                        if matches!(dl.kind, DiffLineKind::HunkHeader) {
                            spans.push(Span::styled(dl.content.clone(), s));
                        } else {
                            spans.push(Span::styled(prefix.to_string(), s));
                            spans.push(Span::styled(dl.content.clone(), s));
                        }

                        lines.push(Line::from(spans));
                        row += 1;
                    }
                }
            }
        }

        lines
    }
}

/// Find git repos in a directory, parse diffs into structured data.
pub(crate) fn fetch_git_diffs(dir: &Path) -> Result<Vec<RepoDiff>, GroveError> {
    let mut repos: Vec<std::path::PathBuf> = Vec::new();

    if dir.join(".git").exists() {
        repos.push(dir.to_path_buf());
    }

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if repos.len() >= MAX_REPOS {
                break;
            }
            let path = entry.path();
            if path.is_dir() && path.join(".git").exists() {
                repos.push(path);
            }
        }
    }

    let mut result = Vec::new();

    for repo in &repos {
        let name = repo.to_string_lossy().to_string();
        let diff = Command::new("git").args(["-C", &name, "diff"]).output();

        let files = match diff {
            Ok(d) => {
                let diff_out = String::from_utf8_lossy(&d.stdout).to_string();
                if diff_out.is_empty() {
                    Vec::new()
                } else {
                    parse_diff_files(&diff_out)
                }
            }
            Err(_) => Vec::new(),
        };

        result.push(RepoDiff { path: name, files });
    }

    Ok(result)
}

fn parse_diff_files(diff_str: &str) -> Vec<DiffFile> {
    let parsed = unidiff::PatchSet::from_str(diff_str);
    let Ok(patchset) = parsed else {
        return Vec::new();
    };

    patchset
        .into_iter()
        .map(|file| {
            let name = if file.target_file == "/dev/null" {
                file.source_file.trim_start_matches("a/").to_string()
            } else {
                file.target_file.trim_start_matches("b/").to_string()
            };
            let kind = if file.source_file == "/dev/null" {
                '+'
            } else if file.target_file == "/dev/null" {
                '-'
            } else {
                '~'
            };
            let added = file.added();
            let removed = file.removed();

            let mut lines = Vec::new();
            for hunk in file.into_iter() {
                lines.push(DiffLine {
                    kind: DiffLineKind::HunkHeader,
                    source_line: None,
                    target_line: None,
                    content: hunk.section_header.clone(),
                });
                for dl in hunk.into_iter() {
                    let lk = if dl.is_added() {
                        DiffLineKind::Added
                    } else if dl.is_removed() {
                        DiffLineKind::Removed
                    } else {
                        DiffLineKind::Context
                    };
                    lines.push(DiffLine {
                        kind: lk,
                        source_line: dl.source_line_no,
                        target_line: dl.target_line_no,
                        content: dl.value.trim_end_matches('\n').to_string(),
                    });
                }
            }

            DiffFile {
                name,
                added,
                removed,
                kind,
                lines,
            }
        })
        .collect()
}
