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

/// Ensure the fetch refspec is configured for a bare repo.
fn ensure_fetch_refspec(bare_path: &Path, verbose: bool) -> Result<(), GroveError> {
    let refspec = run_git(
        &["config", "--get", "remote.origin.fetch"],
        Some(bare_path),
        verbose,
    );
    if refspec.is_err() || refspec.as_deref().map(str::trim).unwrap_or("").is_empty() {
        run_git(
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
            Some(bare_path),
            verbose,
        )?;
    }
    Ok(())
}

/// Build the `git clone --bare` argument vector with `--` before the url and
/// target, so a url/target beginning with `-` can never be parsed as a flag.
/// Factored out as a pure seam so the `--` guard is unit-testable.
fn bare_clone_args<'a>(url: &'a str, target: &'a str) -> Vec<&'a str> {
    vec!["clone", "--bare", "--", url, target]
}

/// Clone a bare repository. Returns the default branch name.
pub fn bare_clone(url: &str, target_path: &Path, verbose: bool) -> Result<String, GroveError> {
    let target_str = target_path
        .to_str()
        .ok_or_else(|| GroveError::General("invalid path".to_string()))?;

    run_git(&bare_clone_args(url, target_str), None, verbose)?;

    // Configure fetch refspec so `git fetch` populates refs/remotes/origin/*
    ensure_fetch_refspec(target_path, verbose)?;

    // Initial fetch to populate remote tracking refs
    run_git(&["fetch", "origin"], Some(target_path), verbose)?;

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
/// Ensures the fetch refspec is configured (repairs bare repos created before this fix).
pub fn fetch_repo(bare_path: &Path, prune: bool, verbose: bool) -> Result<(), GroveError> {
    // Ensure fetch refspec exists (self-healing for pre-fix bare repos)
    ensure_fetch_refspec(bare_path, verbose)?;

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
///
/// WARNING: `update-ref` moves the ref UNCONDITIONALLY — it is not ff-only. If
/// the local default branch has diverged (carries commits not in origin), those
/// local-only commits are silently discarded (orphaned). This is safe for the
/// bare-repo default branch grove manages (never committed to directly), but is
/// a footgun. See SYNC-diverged-default-branch test for the pinned behavior.
/// TODO: harden to a ff-only update (`merge-base --is-ancestor` guard) if any
/// caller ever commits to the local default ref.
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

/// Returns true if a local branch already exists in the bare repo.
pub fn branch_exists(bare_path: &Path, branch: &str, verbose: bool) -> bool {
    run_git(
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        Some(bare_path),
        verbose,
    )
    .is_ok()
}

/// Returns true if a remote-tracking branch origin/<branch> exists in the bare repo.
pub fn branch_exists_remote(bare_path: &Path, branch: &str, verbose: bool) -> bool {
    run_git(
        &[
            "rev-parse",
            "--verify",
            &format!("refs/remotes/origin/{branch}"),
        ],
        Some(bare_path),
        verbose,
    )
    .is_ok()
}

/// Create a worktree from a bare repo.
/// If `branch` already exists locally, checks it out:
/// `git worktree add <worktree_path> <branch>`.
/// Otherwise creates it: `git worktree add -b <branch> <worktree_path> <base_branch>`.
/// Sets upstream tracking to origin/<base_branch>.
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

    if branch_exists(bare_path, branch, verbose) {
        run_git(
            &["worktree", "add", wt_str, branch],
            Some(bare_path),
            verbose,
        )?;
    } else {
        run_git(
            &["worktree", "add", "-b", branch, wt_str, base_branch],
            Some(bare_path),
            verbose,
        )?;
    }

    // Set upstream tracking so `git pull` works in the worktree.
    // Prefer the branch's own remote (origin/<branch>) when it exists,
    // falling back to origin/<base_branch> for freshly created branches.
    let own_remote = format!("origin/{branch}");
    let remote_branch = if branch_exists_remote(bare_path, branch, verbose) {
        own_remote
    } else {
        format!("origin/{base_branch}")
    };
    let _ = run_git(
        &["branch", "--set-upstream-to", &remote_branch, branch],
        Some(bare_path),
        verbose,
    );

    Ok(())
}

/// Create a worktree checked out DETACHED at `commit_ish`.
/// Runs `git worktree add --detach <worktree_path> <commit-ish>`.
///
/// No branch is created or claimed, which is the point: git refuses to check a
/// branch out in two worktrees at once, so a detached checkout is the only way
/// for a task to hold two worktrees sitting on the same branch's commits.
pub fn create_worktree_detached(
    bare_path: &Path,
    worktree_path: &Path,
    commit_ish: &str,
    verbose: bool,
) -> Result<(), GroveError> {
    let wt_str = worktree_path
        .to_str()
        .ok_or_else(|| GroveError::General("invalid worktree path".to_string()))?;

    run_git(
        &["worktree", "add", "--detach", wt_str, commit_ish],
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
/// Runs `git branch -d <branch>` (safe: merged-only) or `-D` when `force` is set.
/// The safe variant deletes branches merged into their upstream or HEAD and
/// fails on unmerged branches, preserving unmerged work.
pub fn delete_branch(
    bare_path: &Path,
    branch: &str,
    force: bool,
    verbose: bool,
) -> Result<(), GroveError> {
    let flag = if force { "-D" } else { "-d" };
    run_git(&["branch", flag, branch], Some(bare_path), verbose)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// ARG-register-url-double-dash (S25): the bare-clone arg vector must place
    /// `--` before the url so a url beginning with `-` is treated as a positional
    /// argument, never a git flag.
    #[test]
    fn bare_clone_args_double_dash_before_url() {
        let args = bare_clone_args("-oProxyCommand=evil", "/tmp/x.git");
        let dd = args.iter().position(|a| *a == "--").expect("-- present");
        let url = args
            .iter()
            .position(|a| *a == "-oProxyCommand=evil")
            .expect("url present");
        assert!(dd < url, "-- must come before the url: {args:?}");
    }

    /// Run git, asserting success (test fixture setup only).
    fn git(args: &[&str], cwd: &Path) {
        let out = git_command(args)
            .current_dir(cwd)
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Build a normal repo with one commit on `main`, then clone --bare.
    /// Returns (tmp, work_dir, bare_path). `tmp` must outlive usage.
    fn setup_bare() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&["init", "-b", "main"], &work);
        git(&["config", "user.email", "test@test.com"], &work);
        git(&["config", "user.name", "Test"], &work);
        std::fs::write(work.join("README.md"), "# repo\n").unwrap();
        git(&["add", "."], &work);
        git(&["commit", "-m", "initial commit"], &work);

        let bare = tmp.path().join("repo.git");
        git(
            &[
                "clone",
                "--bare",
                work.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
            tmp.path(),
        );
        ensure_fetch_refspec(&bare, false).unwrap();
        run_git(&["fetch", "origin"], Some(&bare), false).unwrap();
        (tmp, work, bare)
    }

    #[cfg(unix)]
    fn non_utf8_path() -> PathBuf {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(OsStr::from_bytes(&[0x66, 0x80, 0x80]))
    }

    #[test]
    fn git_delete_d_preserves_unmerged() {
        let (_tmp, _work, bare) = setup_bare();
        // Branch from main, add an unmerged commit (in a worktree).
        let wt = _tmp.path().join("wt");
        create_worktree(&bare, &wt, "feature", "main", false).unwrap();
        git(&["config", "user.email", "test@test.com"], &wt);
        git(&["config", "user.name", "Test"], &wt);
        std::fs::write(wt.join("f.txt"), "x").unwrap();
        git(&["add", "."], &wt);
        git(&["commit", "-m", "unmerged"], &wt);
        // Detach the worktree so the branch can be deleted, but keep its commits.
        remove_worktree(&bare, &wt, false).unwrap();

        let res = delete_branch(&bare, "feature", false, false);
        assert!(res.is_err(), "safe delete of unmerged branch must fail");
        assert!(branch_exists(&bare, "feature", false), "branch preserved");
    }

    #[test]
    fn git_delete_force_removes() {
        let (_tmp, _work, bare) = setup_bare();
        let wt = _tmp.path().join("wt");
        create_worktree(&bare, &wt, "feature", "main", false).unwrap();
        git(&["config", "user.email", "test@test.com"], &wt);
        git(&["config", "user.name", "Test"], &wt);
        std::fs::write(wt.join("f.txt"), "x").unwrap();
        git(&["add", "."], &wt);
        git(&["commit", "-m", "unmerged"], &wt);
        remove_worktree(&bare, &wt, false).unwrap();

        delete_branch(&bare, "feature", true, false).unwrap();
        assert!(!branch_exists(&bare, "feature", false), "branch removed");
    }

    #[test]
    fn git_remove_worktree() {
        let (_tmp, _work, bare) = setup_bare();
        let wt = _tmp.path().join("wt");
        create_worktree(&bare, &wt, "feature", "main", false).unwrap();
        assert!(wt.join("README.md").exists());

        remove_worktree(&bare, &wt, false).unwrap();
        prune_worktrees(&bare, false).unwrap();
        assert!(!wt.exists(), "worktree dir gone after remove");
    }

    #[test]
    fn git_uncommitted_true_false() {
        let (_tmp, _work, bare) = setup_bare();
        let wt = _tmp.path().join("wt");
        create_worktree(&bare, &wt, "feature", "main", false).unwrap();

        assert!(!has_uncommitted_changes(&wt, false).unwrap(), "clean");
        std::fs::write(wt.join("dirty.txt"), "y").unwrap();
        assert!(has_uncommitted_changes(&wt, false).unwrap(), "dirty");
    }

    #[test]
    fn git_create_worktree() {
        let (_tmp, _work, bare) = setup_bare();
        let wt = _tmp.path().join("wt");
        create_worktree(&bare, &wt, "feature", "main", false).unwrap();

        assert!(wt.join("README.md").exists(), "worktree checked out");
        assert!(branch_exists(&bare, "feature", false), "branch created");
        let head = run_git(
            &[
                "-C",
                wt.to_str().unwrap(),
                "rev-parse",
                "--abbrev-ref",
                "HEAD",
            ],
            None,
            false,
        )
        .unwrap();
        assert_eq!(head.trim(), "feature");
    }

    #[cfg(unix)]
    #[test]
    fn git_invalid_utf8_path_errs() {
        let bad = non_utf8_path();
        assert!(bad.to_str().is_none(), "fixture path must be non-utf8");
        let bare = Path::new("/tmp/does-not-matter.git");

        let e = create_worktree(bare, &bad, "b", "main", false).unwrap_err();
        assert!(matches!(e, GroveError::General(ref m) if m.contains("invalid worktree path")));

        let e = remove_worktree(bare, &bad, false).unwrap_err();
        assert!(matches!(e, GroveError::General(ref m) if m.contains("invalid worktree path")));

        let e = has_uncommitted_changes(&bad, false).unwrap_err();
        assert!(matches!(e, GroveError::General(ref m) if m.contains("invalid worktree path")));
    }

    #[test]
    fn sync_diverged_default_branch() {
        // SYNC-diverged-default-branch: local refs/heads/main carries a commit
        // not in origin/main (diverged). update_default_branch does an
        // UNCONDITIONAL update-ref, so the local-only commit is OVERWRITTEN
        // (discarded) — this test pins that current, lossy behavior.
        let (_tmp, work, bare) = setup_bare();

        // origin (the bare's remote-tracking) is at the initial commit.
        let origin_head = run_git(
            &["rev-parse", "refs/remotes/origin/main"],
            Some(&bare),
            false,
        )
        .unwrap()
        .trim()
        .to_string();

        // Forge a local-only commit on the bare's refs/heads/main, diverging it
        // from origin/main. Use the work repo to produce a new commit object,
        // then point the bare's local main at it.
        std::fs::write(work.join("local.txt"), "local-only").unwrap();
        git(&["add", "."], &work);
        git(&["commit", "-m", "local-only divergence"], &work);
        let local_only = run_git(&["rev-parse", "HEAD"], Some(&work), false)
            .unwrap()
            .trim()
            .to_string();
        // Push the commit OBJECT into the bare under refs/heads/main (a forced
        // local divergence) WITHOUT touching refs/remotes/origin/main.
        run_git(
            &[
                "push",
                "--force",
                bare.to_str().unwrap(),
                "HEAD:refs/heads/main",
            ],
            Some(&work),
            false,
        )
        .unwrap();
        assert_ne!(local_only, origin_head, "precondition: diverged");

        update_default_branch(&bare, "main", false).unwrap();

        let after = run_git(&["rev-parse", "refs/heads/main"], Some(&bare), false)
            .unwrap()
            .trim()
            .to_string();
        // PINNED: local main is reset to origin/main; the local-only commit is
        // discarded (orphaned). See the WARNING/TODO on update_default_branch.
        assert_eq!(after, origin_head, "local main overwritten to origin/main");
        assert_ne!(after, local_only, "local-only commit discarded");
    }
}
