use dialoguer::Select;

use crate::commands::Ctx;
use crate::db::Db;
use crate::error::GroveError;
use crate::git;
use crate::output;

pub fn run(
    task_id: Option<&str>,
    force: bool,
    delete_branches: bool,
    interactive: bool,
    ctx: &Ctx,
) -> Result<(), GroveError> {
    let db = ctx.db;
    let json_mode = ctx.json_mode;
    let verbose = ctx.verbose;

    let resolved_id = match task_id {
        Some(id) => id.to_string(),
        None if interactive => interactive_select_task(db)?,
        None => {
            return Err(GroveError::General(
                "task_id is required (use -i for interactive mode)".to_string(),
            ));
        }
    };
    let task_id = &resolved_id;

    let task = db
        .get_task(task_id)?
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?;

    let mut warnings: Vec<String> = Vec::new();
    let mut repos_closed: Vec<String> = Vec::new();

    if !force {
        for task_repo in &task.repos {
            if task_repo.worktree_path.exists() {
                match git::has_uncommitted_changes(&task_repo.worktree_path, verbose) {
                    Ok(true) => {
                        return Err(GroveError::UncommittedChanges(format!(
                            "repo '{}' in task '{task_id}' has uncommitted changes. \
                             Use --force to close anyway.",
                            task_repo.repo_name
                        )));
                    }
                    Ok(false) => {}
                    Err(e) => {
                        warnings.push(format!(
                            "could not check status for '{}': {e}",
                            task_repo.repo_name
                        ));
                    }
                }
            }
        }
    }

    let all_repos = db.list_repos()?;

    // Resolve the task's herdr workspace by cwd BEFORE removing worktrees (the
    // cwd match needs the directory to still exist). Closed best-effort below.
    let workspace_id = task
        .repos
        .first()
        .and_then(|r| {
            crate::herdr::resolve_agent_for_cwd(&r.worktree_path)
                .ok()
                .flatten()
        })
        .and_then(|a| a.workspace_id);

    // Partial-failure safety (S4): on the non-force path, confirm every worktree
    // is gone BEFORE destroying anything else (tmux window, task dir, DB row).
    // A non-force removal failure (e.g. a locked worktree) aborts the whole close
    // with the task row, worktree dir, and tmux window all intact, so the task
    // stays fully re-closable. The force path force-removes the dir directly and
    // proceeds, preserving the N4 idempotency invariant.
    for task_repo in &task.repos {
        let bare_path = all_repos
            .iter()
            .find(|r| r.name == task_repo.repo_name)
            .map(|r| r.path.clone());

        match bare_path {
            Some(bp) if bp.exists() => {
                if let Err(e) = git::remove_worktree(&bp, &task_repo.worktree_path, verbose) {
                    if force {
                        let _ = std::fs::remove_dir_all(&task_repo.worktree_path);
                        warnings.push(format!(
                            "git worktree remove failed for '{}', removed directory directly: {e}",
                            task_repo.repo_name
                        ));
                    } else if task_repo.worktree_path.exists() {
                        // The worktree dir is still on disk and git refused to
                        // remove it (e.g. locked): abort with everything intact.
                        return Err(GroveError::General(format!(
                            "failed to remove worktree for '{}' in task '{task_id}': {e}. \
                             Nothing was destroyed; use --force to close anyway.",
                            task_repo.repo_name
                        )));
                    } else {
                        // Dir already gone: the worktree is effectively removed,
                        // so tolerate the error and keep close idempotent (N4).
                        warnings.push(format!(
                            "worktree for '{}' was already gone: {e}",
                            task_repo.repo_name
                        ));
                    }
                }
            }
            // Bare repo missing / repo not registered are not removal failures:
            // the worktree git metadata is already unusable, so skip and warn.
            _ => {}
        }
    }

    // Best-effort: close the task's herdr workspace if we resolved one. Ignore
    // errors — the git worktree teardown is what close guarantees.
    if let Some(ref ws) = workspace_id {
        if let Err(e) = crate::herdr::close_workspace(ws) {
            if verbose {
                eprintln!("Warning: failed to close herdr workspace '{ws}': {e}");
            }
        }
    }

    // Worktrees are now confirmed gone (or force-removed). Clean up the branch +
    // prune per repo, and account for skip-with-warning arms.
    for task_repo in &task.repos {
        let bare_path = all_repos
            .iter()
            .find(|r| r.name == task_repo.repo_name)
            .map(|r| r.path.clone());

        match bare_path {
            Some(bp) if bp.exists() => {
                repos_closed.push(task_repo.repo_name.clone());

                // Always clean up the task branch. By default use a safe delete
                // (merged-only) so unmerged work is preserved; `--delete-branches/-D`
                // force-deletes regardless. A detached worktree (`add --detach`)
                // records an empty branch — there is nothing to delete, and
                // asking git would only produce a bogus "not merged" warning.
                if task_repo.branch.is_empty() {
                    // detached checkout: no branch of ours to reclaim.
                } else if let Err(e) =
                    git::delete_branch(&bp, &task_repo.branch, delete_branches, verbose)
                {
                    if delete_branches {
                        warnings.push(format!(
                            "failed to delete branch '{}' from '{}': {e}",
                            task_repo.branch, task_repo.repo_name
                        ));
                    } else {
                        warnings.push(format!(
                            "branch '{}' in '{}' is not merged; kept it. \
                             Re-run with --delete-branches/-D to force-delete.",
                            task_repo.branch, task_repo.repo_name
                        ));
                    }
                }
                if let Err(e) = git::prune_worktrees(&bp, verbose) {
                    warnings.push(format!(
                        "failed to prune worktrees for '{}': {e}",
                        task_repo.repo_name
                    ));
                }
            }
            Some(_) => {
                let warn = format!(
                    "bare repo directory missing for '{}', skipping worktree removal",
                    task_repo.repo_name
                );
                if !json_mode {
                    eprintln!("Warning: {warn}");
                }
                warnings.push(warn);
                repos_closed.push(task_repo.repo_name.clone());
            }
            None => {
                let warn = format!(
                    "repo '{}' no longer registered, skipping worktree removal",
                    task_repo.repo_name
                );
                if !json_mode {
                    eprintln!("Warning: {warn}");
                }
                warnings.push(warn);
                repos_closed.push(task_repo.repo_name.clone());
            }
        }
    }

    // Tolerate an already-gone or unremovable path: the DB row must be cleared
    // regardless so close is idempotent and never strands a task row (N4).
    //
    // Guard the recursive delete on containment. `task.path` is not necessarily
    // one `init` validated — `migrate_state_json` imports paths from a legacy
    // `state.json` grove never checked — so a path pointing outside the tasks
    // dir must be refused, not deleted. Belt-and-suspenders with the identifier
    // fix; a recursive delete of an unvalidated path deserves its own guard.
    if task.path.exists() {
        if crate::validation::is_within(&task.path, &ctx.config.tasks_dir) {
            if let Err(e) = std::fs::remove_dir_all(&task.path) {
                warnings.push(format!("failed to remove task directory: {e}"));
            }
        } else {
            warnings.push(format!(
                "refused to remove task directory outside the tasks dir: {}",
                task.path.display()
            ));
        }
    }

    db.delete_task(task_id)?;

    let data = serde_json::json!({
        "task_id": task_id,
        "repos_closed": repos_closed,
        "warnings": warnings,
    });
    output::success(json_mode, &format!("Closed task '{task_id}'"), data);

    Ok(())
}

fn interactive_select_task(db: &Db) -> Result<String, GroveError> {
    let tasks = db.list_tasks()?;
    if tasks.is_empty() {
        return Err(GroveError::General("no active tasks to close".to_string()));
    }

    let display_items: Vec<String> = tasks
        .iter()
        .map(|task| {
            let repos: Vec<&str> = task.repos.iter().map(|r| r.repo_name.as_str()).collect();
            let stale = if task.is_stale() { " [stale]" } else { "" };
            format!("{} ({}){stale}", task.id, repos.join(", "))
        })
        .collect();

    let selection = Select::new()
        .with_prompt("Select task to close")
        .items(&display_items)
        .interact()
        .map_err(|e| GroveError::General(format!("interactive selection failed: {e}")))?;

    Ok(tasks[selection].id.clone())
}
