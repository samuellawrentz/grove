use crate::config::GroveConfig;
use crate::error::GroveError;
use crate::git;
use crate::output;
use crate::state::GroveState;

pub fn run(
    task_id: &str,
    force: bool,
    _config: &GroveConfig,
    state: &mut GroveState,
    json_mode: bool,
    verbose: bool,
) -> Result<(), GroveError> {
    let task = state
        .tasks
        .get(task_id)
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?
        .clone();

    let mut warnings: Vec<String> = Vec::new();
    let mut repos_closed: Vec<String> = Vec::new();

    // Check for uncommitted changes (unless --force)
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
                        // If we can't check status, treat as a warning but don't block
                        warnings.push(format!(
                            "could not check status for '{}': {e}",
                            task_repo.repo_name
                        ));
                    }
                }
            }
        }
    }

    // Remove each worktree
    for task_repo in &task.repos {
        let bare_path = state
            .repos
            .get(&task_repo.repo_name)
            .map(|r| r.path.clone());

        match bare_path {
            Some(bp) if bp.exists() => {
                if let Err(e) = git::remove_worktree(&bp, &task_repo.worktree_path, verbose) {
                    // Force removal: try removing the directory directly if git worktree remove fails
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
                // Bare repo dir missing — skip worktree removal, warn
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
                // Repo no longer in state — skip
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

    // Remove task directory
    if task.path.exists() {
        std::fs::remove_dir_all(&task.path)?;
    }

    // Update state
    state.tasks.remove(task_id);
    state.save()?;

    let data = serde_json::json!({
        "task_id": task_id,
        "repos_closed": repos_closed,
        "warnings": warnings,
    });
    output::success(json_mode, &format!("Closed task '{task_id}'"), data);

    Ok(())
}
