//! Data-fetch layer for the TUI: tmux panes/preview, agent state, directory
//! listings, and git-diff fetch+parse. The interactive diff *view* that renders
//! these `RepoDiff` structures lives in `super::diff_view`.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::str::FromStr;

use crate::agent::{self, AgentKind, AgentState};
use crate::error::GroveError;
use crate::tmux::{self, PaneInfo};

// Re-export the diff view so existing `source::DiffState` callers are unchanged.
pub(crate) use super::diff_view::DiffState;

// ───────────────────────────── data fetch ──────────────────────────────────

/// Fetch all tmux panes.
pub(crate) fn fetch_panes(verbose: bool) -> Result<Vec<PaneInfo>, GroveError> {
    tmux::list_all_panes(verbose)
}

/// Fetch agent states from the external hook state file.
pub(crate) fn fetch_agent_states() -> Result<HashMap<String, AgentState>, GroveError> {
    agent::read_state_file()
}

/// Fetch pane_id -> AgentKind declared by hooks in the state file.
pub(crate) fn fetch_state_kinds() -> HashMap<String, AgentKind> {
    agent::read_state_kinds()
}

/// Capture a bounded window of a tmux pane's content (last `lines` rows).
pub(crate) fn fetch_preview(
    pane_id: &str,
    lines: usize,
    verbose: bool,
) -> Result<String, GroveError> {
    tmux::capture_with_args(&super::app::capture_args(pane_id, lines), verbose)
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

// ─────────────────────────── diff data model ───────────────────────────────

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
    /// Whether this file's diff lines are currently shown.
    pub expanded: bool,
}

/// A repo's diff data.
#[derive(Clone)]
pub(crate) struct RepoDiff {
    pub path: String,
    pub files: Vec<DiffFile>,
}

// ──────────────────────────── refetch gate ─────────────────────────────────

/// Whether the diff should be refetched given a freshly-computed token and the
/// last one. True (refetch) when there is no prior token or the token changed;
/// false (skip) when identical. Pure so the gate is unit-testable.
pub(crate) fn diff_token_changed(new: Option<&String>, last: Option<&String>) -> bool {
    new != last
}

/// Compute a cheap dirty token for a diff target directory from each contained
/// repo's HEAD oid and porcelain status. Unchanged token => no diff changed, so
/// the per-tick `git diff` reparse can be skipped. Returns None if no git repo.
pub(crate) fn diff_dirty(dir: &Path) -> Option<String> {
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
    if repos.is_empty() {
        return None;
    }
    let mut token = String::new();
    for repo in &repos {
        let name = repo.to_string_lossy().to_string();
        let head = Command::new("git")
            .args(["-C", &name, "rev-parse", "HEAD"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let status = Command::new("git")
            .args(["-C", &name, "status", "--porcelain"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        token.push_str(&name);
        token.push('|');
        token.push_str(&head);
        token.push('|');
        token.push_str(&status);
        token.push('\n');
    }
    Some(token)
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
        let diff = Command::new("git")
            .args(["-C", &name, "diff"])
            .output()
            .map_err(|e| GroveError::General(format!("failed to run git diff in {name}: {e}")))?;

        // A non-zero exit means the diff could not be computed (broken repo, not
        // a git repo, etc.). That is an error, NOT a genuinely-empty "no changes"
        // diff — surface it instead of rendering it as clean.
        if !diff.status.success() {
            let stderr = String::from_utf8_lossy(&diff.stderr);
            return Err(GroveError::General(format!(
                "git diff failed in {name}: {}",
                stderr.trim()
            )));
        }

        let diff_out = String::from_utf8_lossy(&diff.stdout).to_string();
        let files = if diff_out.is_empty() {
            Vec::new()
        } else {
            parse_diff_files(&diff_out)
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
                expanded: false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DIFF-skip-when-unchanged (S15): the refetch gate compares a cheap
    /// HEAD+index token. Equal tokens => unchanged (no refetch); differing
    /// tokens => changed (refetch). Tested directly on the token comparison.
    /// DIFF-broken-repo-not-no-changes (S25): a dir that looks like a repo
    /// (`.git` present) but on which `git diff` exits non-zero is a broken repo,
    /// not a clean "no changes" tree. The fetch must surface an Err, never an
    /// empty success.
    #[test]
    fn diff_broken_repo_is_error_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        // A bogus `.git` directory makes this look like a repo to the scan but
        // makes `git diff` fail (not a valid git directory).
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git").join("notgit"), "x").unwrap();

        let result = fetch_git_diffs(tmp.path());
        assert!(
            result.is_err(),
            "broken repo must surface as Err, got {:?}",
            result.map(|r| r.len())
        );
    }

    #[test]
    fn diff_skip_when_unchanged() {
        let a = "headA|idxA".to_string();
        let b = "headB|idxA".to_string();
        assert!(!diff_token_changed(Some(&a), Some(&a)));
        assert!(diff_token_changed(Some(&b), Some(&a)));
        // First fetch (no prior token) always counts as changed.
        assert!(diff_token_changed(Some(&a), None));
    }
}
