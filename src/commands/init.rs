use chrono::Utc;

use crate::config::GroveConfig;
use crate::error::GroveError;
use crate::git;
use crate::output;
use crate::state::{GroveState, TaskEntry, TaskRepo};

/// Validate task-id: non-empty, filesystem-safe [a-zA-Z0-9._-]+
fn validate_task_id(task_id: &str) -> Result<(), GroveError> {
    if task_id.is_empty() {
        return Err(GroveError::General("task-id cannot be empty".to_string()));
    }
    if !task_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(GroveError::General(format!(
            "invalid task-id '{task_id}': must match [a-zA-Z0-9._-]+"
        )));
    }
    Ok(())
}

/// Check if an existing task entry is stale (task dir or any worktree path missing on disk).
fn is_stale(task: &TaskEntry) -> bool {
    if !task.path.exists() {
        return true;
    }
    for repo in &task.repos {
        if !repo.worktree_path.exists() {
            return true;
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    task_id: &str,
    repos: &[String],
    context: Option<&str>,
    branch: Option<&str>,
    base: Option<&str>,
    config: &GroveConfig,
    state: &mut GroveState,
    json_mode: bool,
    verbose: bool,
) -> Result<(), GroveError> {
    validate_task_id(task_id)?;

    if repos.is_empty() {
        return Err(GroveError::General(
            "at least one repo must be specified".to_string(),
        ));
    }

    // Validate all repo names are registered
    for repo_name in repos {
        if !state.repos.contains_key(repo_name) {
            return Err(GroveError::RepoNotRegistered(repo_name.clone()));
        }
    }

    // Idempotency: check if task already exists in state
    if let Some(existing) = state.tasks.get(task_id) {
        if is_stale(existing) {
            // Stale entry — clean up orphaned worktree refs and branches, then proceed
            eprintln!(
                "Warning: task '{task_id}' has stale state (directories missing). Re-creating."
            );
            // Clean up orphaned git worktree entries and branches
            for task_repo in &existing.repos {
                if let Some(repo_entry) = state.repos.get(&task_repo.repo_name)
                    && repo_entry.path.exists()
                {
                    // Prune stale worktree references
                    let _ = git::run_git(&["worktree", "prune"], Some(&repo_entry.path), verbose);
                    // Delete the orphaned branch so it can be re-created
                    let _ = git::run_git(
                        &["branch", "-D", &task_repo.branch],
                        Some(&repo_entry.path),
                        verbose,
                    );
                }
            }
            state.tasks.remove(task_id);
        } else {
            // Non-stale: check if repo list matches
            let mut existing_repos: Vec<&str> = existing
                .repos
                .iter()
                .map(|r| r.repo_name.as_str())
                .collect();
            existing_repos.sort();

            let mut requested_repos: Vec<&str> = repos.iter().map(|s| s.as_str()).collect();
            requested_repos.sort();

            if existing_repos == requested_repos {
                // Idempotent: same repos, return existing task info
                let repo_names: Vec<&str> = existing
                    .repos
                    .iter()
                    .map(|r| r.repo_name.as_str())
                    .collect();
                let data = serde_json::json!({
                    "task_id": task_id,
                    "path": existing.path,
                    "repos": repo_names,
                    "created_at": existing.created_at,
                    "already_existed": true,
                });
                output::success(json_mode, &format!("Task '{task_id}' already exists"), data);
                return Ok(());
            } else {
                return Err(GroveError::Conflict(format!(
                    "Task '{task_id}' already exists with different repos. \
                     Use `grove close {task_id}` then re-init to change repos."
                )));
            }
        }
    }

    let branch_name = branch.unwrap_or(task_id);
    let task_dir = config.tasks_dir.join(task_id);

    // Create task directory
    std::fs::create_dir_all(&task_dir)?;

    // Create worktrees with rollback on failure
    let mut created_worktrees: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new(); // (bare_path, worktree_path)
    let mut task_repos: Vec<TaskRepo> = Vec::new();

    let create_result = (|| -> Result<(), GroveError> {
        for repo_name in repos {
            let repo_entry = state.repos.get(repo_name).unwrap(); // validated above
            let bare_path = &repo_entry.path;
            let base_branch = base.unwrap_or(&repo_entry.default_branch);
            let worktree_path = task_dir.join(repo_name);

            git::create_worktree(bare_path, &worktree_path, branch_name, base_branch, verbose)?;

            created_worktrees.push((bare_path.clone(), worktree_path.clone()));
            task_repos.push(TaskRepo {
                repo_name: repo_name.clone(),
                worktree_path,
                branch: branch_name.to_string(),
            });
        }
        Ok(())
    })();

    if let Err(e) = create_result {
        // Rollback: remove all worktrees created so far
        for (bare_path, worktree_path) in created_worktrees.iter().rev() {
            let _ = git::remove_worktree(bare_path, worktree_path, verbose);
        }
        // Remove task directory
        let _ = std::fs::remove_dir_all(&task_dir);
        return Err(e);
    }

    // Write CONTEXT.md
    let context_content = if let Some(ctx) = context {
        ctx.to_string()
    } else {
        let repo_names: Vec<&str> = repos.iter().map(|s| s.as_str()).collect();
        let date = Utc::now().format("%Y-%m-%d");
        format!(
            "# Task: {task_id}\n\n\
             **Repos:** {}\n\
             **Created:** {date}\n\n\
             ## Description\n\n\
             _Add task description here._\n",
            repo_names.join(", ")
        )
    };
    std::fs::write(task_dir.join("CONTEXT.md"), &context_content)?;

    // Update state only after all worktrees succeeded
    let task_entry = TaskEntry {
        id: task_id.to_string(),
        path: task_dir.clone(),
        repos: task_repos,
        created_at: Utc::now(),
    };
    state.tasks.insert(task_id.to_string(), task_entry);
    state.save()?;

    let repo_names: Vec<&str> = repos.iter().map(|s| s.as_str()).collect();
    let data = serde_json::json!({
        "task_id": task_id,
        "path": task_dir,
        "repos": repo_names,
        "branch": branch_name,
        "already_existed": false,
    });
    output::success(
        json_mode,
        &format!(
            "Created task '{task_id}' with repos: {} (branch: {branch_name})",
            repo_names.join(", ")
        ),
        data,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_task_id_valid() {
        assert!(validate_task_id("TASK-1").is_ok());
        assert!(validate_task_id("my.task").is_ok());
        assert!(validate_task_id("my_task").is_ok());
        assert!(validate_task_id("a").is_ok());
        assert!(validate_task_id("ABC-123").is_ok());
    }

    #[test]
    fn test_validate_task_id_invalid() {
        assert!(validate_task_id("").is_err());
        assert!(validate_task_id("my/task").is_err());
        assert!(validate_task_id("my task").is_err());
        assert!(validate_task_id("my@task").is_err());
    }
}
