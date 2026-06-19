//! Small shared helpers for command implementations.

use std::path::Path;

use crate::commands::rollback::StepJournal;
use crate::error::GroveError;
use crate::git;

/// Provision a git worktree and journal its undo.
///
/// `init` and `add` both create a branch/worktree under rollback protection.
/// This owns the rollback-sensitive sequence so both call sites stay identical:
/// detect whether the branch is freshly created, create the worktree, then
/// register the worktree undo — passing the branch name only when it was created
/// here so the journal force-deletes it on failure (a reused branch survives).
pub fn provision_worktree(
    journal: &mut StepJournal,
    bare_path: &Path,
    worktree_path: &Path,
    branch: &str,
    base: &str,
    verbose: bool,
) -> Result<(), GroveError> {
    let created_branch = !git::branch_exists(bare_path, branch, verbose);
    git::create_worktree(bare_path, worktree_path, branch, base, verbose)?;
    journal.worktree(bare_path, worktree_path, created_branch.then_some(branch));
    Ok(())
}

/// Find a registered repo by name in an already-loaded list.
///
/// init/close/sync legitimately `list_repos()` once and iterate; this dedups
/// the inline `.iter().find(|r| r.name == name)` scans where a missing repo is
/// genuinely an error. Sites that tolerate "not found" keep their own
/// `Option`-returning `.find`.
pub fn resolve_repo<'a>(
    repos: &'a [crate::db::RepoEntry],
    name: &str,
) -> Result<&'a crate::db::RepoEntry, crate::error::GroveError> {
    repos
        .iter()
        .find(|r| r.name == name)
        .ok_or_else(|| crate::error::GroveError::RepoNotRegistered(name.to_string()))
}
