use dialoguer::Select;

use crate::config::GroveConfig;
use crate::db::Db;
use crate::error::GroveError;
use crate::git;
use crate::output;

#[allow(clippy::too_many_arguments)]
pub fn run(
    task_id: Option<&str>,
    force: bool,
    delete_branches: bool,
    interactive: bool,
    _config: &GroveConfig,
    db: &Db,
    json_mode: bool,
    verbose: bool,
) -> Result<(), GroveError> {
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

    if let Some(ref target) = task.tmux_window {
        if let Err(e) = crate::tmux::kill_window(target, verbose) {
            if verbose {
                eprintln!("Warning: failed to kill tmux window: {e}");
            }
        }
    }

    let all_repos = db.list_repos()?;

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
                    } else {
                        warnings.push(format!(
                            "failed to remove worktree for '{}': {e}",
                            task_repo.repo_name
                        ));
                    }
                }
                repos_closed.push(task_repo.repo_name.clone());
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

    if delete_branches {
        for task_repo in &task.repos {
            let bare_path = all_repos
                .iter()
                .find(|r| r.name == task_repo.repo_name)
                .map(|r| r.path.clone());

            if let Some(bp) = bare_path {
                if bp.exists() {
                    if let Err(e) = git::delete_branch(&bp, &task_repo.branch, verbose) {
                        warnings.push(format!(
                            "failed to delete branch '{}' from '{}': {e}",
                            task_repo.branch, task_repo.repo_name
                        ));
                    }
                    if let Err(e) = git::prune_worktrees(&bp, verbose) {
                        warnings.push(format!(
                            "failed to prune worktrees for '{}': {e}",
                            task_repo.repo_name
                        ));
                    }
                }
            }
        }
    }

    if task.path.exists() {
        std::fs::remove_dir_all(&task.path)?;
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
