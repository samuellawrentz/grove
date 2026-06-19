//! Interactive diff *view*: the `DiffState` cursor/expand model and its
//! styled-line rendering. Pure presentation over the `RepoDiff` data that
//! `source.rs` fetches — no git/process/fs access lives here.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::flat_rows::FlatRows;
use super::source::{DiffFile, DiffLineKind, RepoDiff};

/// Cap on rendered diff lines per file (keeps a huge file from blowing the
/// viewport); the `... truncated` marker is counted as one real row.
pub(crate) const MAX_LINES_PER_FILE: usize = 500;

/// Number of visible rows a single file occupies: its header, plus — when
/// expanded — the rendered diff lines (`MAX_LINES_PER_FILE` cap) and the
/// `... truncated` marker counted exactly when render emits it. This is the
/// single source of truth `total_rows`/`cursor_file`/`row_for_file`/`render`
/// all share, so counts and render can never disagree (truncation desync fix).
fn file_row_count(file: &DiffFile) -> usize {
    if !file.expanded {
        return 1; // header only
    }
    let n = file.lines.len();
    let shown = n.min(MAX_LINES_PER_FILE);
    let truncated = (n > MAX_LINES_PER_FILE) as usize;
    1 + shown + truncated
}

/// Interactive diff state with per-file expand/collapse and cursor.
pub(crate) struct DiffState {
    pub repos: Vec<RepoDiff>,
    pub cursor: usize, // flat row index
}

impl DiffState {
    pub fn new(mut repos: Vec<RepoDiff>) -> Self {
        // Auto-expand the first file (in the first repo that has any).
        for repo in &mut repos {
            if let Some(first) = repo.files.first_mut() {
                first.expanded = true;
                break;
            }
        }
        DiffState { repos, cursor: 0 }
    }

    /// Update repo data while preserving cursor and expanded state.
    /// Expansion is preserved per file by `(repo path, file name)`, so adding or
    /// removing files no longer collapses everything the user has open.
    pub fn update(&mut self, mut repos: Vec<RepoDiff>) {
        // Index prior expansion by (repo path, file name).
        let mut prior: std::collections::HashMap<(&str, &str), bool> =
            std::collections::HashMap::new();
        for repo in &self.repos {
            for file in &repo.files {
                prior.insert((repo.path.as_str(), file.name.as_str()), file.expanded);
            }
        }
        for repo in &mut repos {
            for file in &mut repo.files {
                if let Some(&was) = prior.get(&(repo.path.as_str(), file.name.as_str())) {
                    file.expanded = was;
                }
            }
        }
        self.repos = repos;
        // Clamp cursor
        let total = self.total_rows();
        if total > 0 && self.cursor >= total {
            self.cursor = total - 1;
        }
    }

    /// Total visible rows.
    pub fn total_rows(&self) -> usize {
        let mut count = 0;
        for repo in &self.repos {
            count += 1; // repo header
            if repo.files.is_empty() {
                count += 1; // "No changes"
            }
            for file in &repo.files {
                count += file_row_count(file);
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
                row += file_row_count(file);
            }
        }
        None
    }

    /// Toggle expand/collapse on the file under cursor.
    pub fn toggle_expand(&mut self) {
        if let Some((ri, fi)) = self.cursor_file() {
            self.repos[ri].files[fi].expanded = !self.repos[ri].files[fi].expanded;
        }
    }

    /// Move cursor down. Cursor/overrun math is the shared `FlatRows` trait.
    pub fn move_down(&mut self) {
        FlatRows::move_down_by(self, 1);
    }

    /// Move cursor up.
    pub fn move_up(&mut self) {
        FlatRows::move_up_by(self, 1);
    }

    /// If cursor is on a file header, expand it (and collapse the previous one).
    fn auto_expand_at_cursor(&mut self) {
        if let Some((ri, fi)) = self.cursor_file() {
            if !self.repos[ri].files[fi].expanded {
                // Collapse all other files
                for (r, repo) in self.repos.iter_mut().enumerate() {
                    for (f, file) in repo.files.iter_mut().enumerate() {
                        if r != ri || f != fi {
                            file.expanded = false;
                        }
                    }
                }
                // Recalculate cursor position after collapsing
                self.cursor = self.row_for_file(ri, fi);
                self.repos[ri].files[fi].expanded = true;
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
                row += file_row_count(file);
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

        for repo in &self.repos {
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

            for file in &repo.files {
                let is_expanded = file.expanded;
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

impl FlatRows for DiffState {
    fn total(&self) -> usize {
        self.total_rows()
    }
    fn cursor(&self) -> usize {
        self.cursor
    }
    fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor;
    }
    /// Auto-expand the file the cursor just landed on (collapsing siblings).
    fn on_step(&mut self) {
        self.auto_expand_at_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::source::DiffLine;

    fn diff_line() -> DiffLine {
        DiffLine {
            kind: DiffLineKind::Context,
            source_line: Some(1),
            target_line: Some(1),
            content: "x".to_string(),
        }
    }

    fn file(name: &str, n_lines: usize, expanded: bool) -> DiffFile {
        DiffFile {
            name: name.to_string(),
            added: 0,
            removed: 0,
            kind: '~',
            lines: (0..n_lines).map(|_| diff_line()).collect(),
            expanded,
        }
    }

    /// DIFF-truncation-count-eq-render (P0 / Step 11): total_rows must equal the
    /// rendered row count even past MAX_LINES_PER_FILE — the truncated marker is
    /// a real counted row. Red on old code: total counted all 600 lines while
    /// render stopped at 500 + one "... truncated".
    #[test]
    fn diff_truncation_count_eq_render() {
        // 600 lines: render shows 500 + truncated, total must agree.
        let ds = DiffState {
            repos: vec![RepoDiff {
                path: "r".to_string(),
                files: vec![file("a.rs", 600, true)],
            }],
            cursor: 0,
        };
        assert_eq!(ds.total_rows(), ds.render().len());
        // Boundary: exactly 500 lines → no truncation marker, still equal.
        let ds500 = DiffState {
            repos: vec![RepoDiff {
                path: "r".to_string(),
                files: vec![file("a.rs", 500, true)],
            }],
            cursor: 0,
        };
        assert_eq!(ds500.total_rows(), ds500.render().len());
    }

    /// DIFF-pagedown-never-overruns-total (P0 / Step 11): a paged move that
    /// auto-expands onto a small file (collapsing the large file0 mid-loop) must
    /// never push the cursor past the (shrunken) total. Red on old code: it
    /// cached `total` once against the large layout and kept incrementing.
    #[test]
    fn diff_pagedown_never_overruns_total() {
        // file0 = 600 lines auto-expanded; one small collapsed file last.
        // Layout (flat rows): repo hdr 0; big hdr 1; big 500 lines + trunc
        // (2..502); small hdr 503. total = 1 + 502 + 1 = 504.
        let ds_files = vec![file("big.rs", 600, false), file("small.rs", 3, false)];
        let mut ds = DiffState::new(vec![RepoDiff {
            path: "r".to_string(),
            files: ds_files,
        }]);
        // Park the cursor on big.rs's last visible row, just before small.rs's
        // header. The page steps onto small.rs (row 503), auto_expand collapses
        // the 600-line big.rs (total 504 → 6) and resets the cursor low — then
        // the loop keeps stepping. Old code cached total=504 once and walks the
        // cursor far past the now-6-row set; the fix recomputes each step.
        ds.cursor = ds.total_rows() - 2; // 502: last row of big.rs
        ds.move_down_by(10);

        let total = ds.total_rows();
        assert!(
            ds.cursor < total,
            "cursor {} overran total {}",
            ds.cursor,
            total
        );
        // render() emits exactly total_rows() lines, so any cursor < total is a
        // real rendered row.
        assert!(ds.cursor < ds.render().len(), "cursor on an unrendered row");
        assert!(ds.cursor_file().is_some() || ds.cursor < total);
    }

    /// DIFF-update-expansion-survives-count-change (P0 / Step 11 / S2): update()
    /// must preserve a file's expansion by name across a file add/remove. Red on
    /// old code: the count mismatch (2→3) threw away the whole repo's expanded
    /// vec, snapping b.rs shut.
    #[test]
    fn diff_update_expansion_survives_count_change() {
        let mut ds = DiffState::new(vec![RepoDiff {
            path: "r".to_string(),
            files: vec![file("a.rs", 10, false), file("b.rs", 20, false)],
        }]);
        // Expand b.rs explicitly (new() only auto-expands the first file).
        ds.repos[0].files[1].expanded = true;
        assert!(ds.repos[0].files[1].expanded);

        // Update adds c.rs (count 2 → 3).
        ds.update(vec![RepoDiff {
            path: "r".to_string(),
            files: vec![
                file("a.rs", 10, false),
                file("b.rs", 20, false),
                file("c.rs", 5, false),
            ],
        }]);

        let b = ds.repos[0]
            .files
            .iter()
            .find(|f| f.name == "b.rs")
            .expect("b.rs still present");
        assert!(b.expanded, "b.rs expansion must survive the count change");
    }
}
