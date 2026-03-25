use std::path::Path;
use std::process::Command;

use crate::error::GroveError;

/// Create a `Command` for git with LC_ALL=C always set.
/// If verbose is true, the caller should use `run_git` which logs command and exit code.
fn git_command(args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.env("LC_ALL", "C");
    cmd.args(args);
    cmd
}

/// Run a git command, optionally logging the command line and exit code.
/// Returns (stdout, stderr) on success, or GroveError on failure.
pub fn run_git(args: &[&str], cwd: Option<&Path>, verbose: bool) -> Result<String, GroveError> {
    let mut cmd = git_command(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    if verbose {
        eprintln!("[grove] git {}", args.join(" "));
    }

    let output = cmd
        .output()
        .map_err(|e| GroveError::General(format!("failed to run git {}: {e}", args.join(" "))))?;

    if verbose {
        eprintln!("[grove] exit code: {}", output.status.code().unwrap_or(-1));
    }

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(GroveError::General(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )))
    }
}

/// Clone a bare repository. Returns the default branch name.
pub fn bare_clone(url: &str, target_path: &Path, verbose: bool) -> Result<String, GroveError> {
    let target_str = target_path
        .to_str()
        .ok_or_else(|| GroveError::General("invalid path".to_string()))?;

    run_git(&["clone", "--bare", url, target_str], None, verbose)?;

    // Detect default branch via symbolic-ref
    let output = run_git(&["symbolic-ref", "HEAD"], Some(target_path), verbose)?;
    let refname = output.trim();
    // refs/heads/main -> main
    let branch = refname
        .strip_prefix("refs/heads/")
        .unwrap_or(refname)
        .to_string();

    Ok(branch)
}

/// Fetch all remotes for a bare repo. Optionally prune.
pub fn fetch_repo(bare_path: &Path, prune: bool, verbose: bool) -> Result<(), GroveError> {
    let mut args = vec!["fetch", "--all"];
    if prune {
        args.push("--prune");
    }
    run_git(&args, Some(bare_path), verbose)?;
    Ok(())
}

/// Fast-forward a local branch to match its remote tracking branch.
/// Runs `git update-ref refs/heads/<branch> refs/remotes/origin/<branch>`.
/// Silently skips if the remote ref doesn't exist.
pub fn update_default_branch(
    bare_path: &Path,
    branch: &str,
    verbose: bool,
) -> Result<(), GroveError> {
    let remote_ref = format!("refs/remotes/origin/{branch}");
    let local_ref = format!("refs/heads/{branch}");

    // Check remote ref exists before updating
    match run_git(
        &["rev-parse", "--verify", &remote_ref],
        Some(bare_path),
        verbose,
    ) {
        Ok(_) => {
            run_git(
                &["update-ref", &local_ref, &remote_ref],
                Some(bare_path),
                verbose,
            )?;
            Ok(())
        }
        Err(_) => Ok(()), // remote ref doesn't exist, skip
    }
}

/// Create a worktree from a bare repo.
/// Runs `git worktree add -b <branch> <worktree_path> <base_branch>`.
pub fn create_worktree(
    bare_path: &Path,
    worktree_path: &Path,
    branch: &str,
    base_branch: &str,
    verbose: bool,
) -> Result<(), GroveError> {
    let wt_str = worktree_path
        .to_str()
        .ok_or_else(|| GroveError::General("invalid worktree path".to_string()))?;

    run_git(
        &["worktree", "add", "-b", branch, wt_str, base_branch],
        Some(bare_path),
        verbose,
    )?;
    Ok(())
}

/// Remove a worktree from a bare repo.
/// Runs `git worktree remove <path>`.
pub fn remove_worktree(
    bare_path: &Path,
    worktree_path: &Path,
    verbose: bool,
) -> Result<(), GroveError> {
    let wt_str = worktree_path
        .to_str()
        .ok_or_else(|| GroveError::General("invalid worktree path".to_string()))?;

    run_git(&["worktree", "remove", wt_str], Some(bare_path), verbose)?;
    Ok(())
}

/// Delete a branch from a bare repo.
/// Runs `git branch -D <branch>`.
pub fn delete_branch(bare_path: &Path, branch: &str, verbose: bool) -> Result<(), GroveError> {
    run_git(&["branch", "-D", branch], Some(bare_path), verbose)?;
    Ok(())
}

/// Prune stale worktree references.
/// Runs `git worktree prune`.
pub fn prune_worktrees(bare_path: &Path, verbose: bool) -> Result<(), GroveError> {
    run_git(&["worktree", "prune"], Some(bare_path), verbose)?;
    Ok(())
}

/// Check if a worktree has uncommitted changes.
/// Runs `git -C <path> status --porcelain` and returns true if output is non-empty.
pub fn has_uncommitted_changes(worktree_path: &Path, verbose: bool) -> Result<bool, GroveError> {
    let wt_str = worktree_path
        .to_str()
        .ok_or_else(|| GroveError::General("invalid worktree path".to_string()))?;

    let output = run_git(&["-C", wt_str, "status", "--porcelain"], None, verbose)?;
    Ok(!output.trim().is_empty())
}
