use crate::db::{Db, TaskRepo};
use crate::error::GroveError;
use crate::git;
use crate::output;
use crate::validation::validate_identifier;

#[allow(clippy::too_many_arguments)]
pub fn run(
    task_id: &str,
    repo_name: &str,
    branch: Option<&str>,
    base: Option<&str>,
    _config: &crate::config::GroveConfig,
    db: &Db,
    json_mode: bool,
    verbose: bool,
) -> Result<(), GroveError> {
    validate_identifier(repo_name, "repo")?;

    let mut task = db
        .get_task(task_id)?
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?;

    let already_in_task = task.repos.iter().any(|r| r.repo_name == repo_name);
    if already_in_task {
        return Err(GroveError::Conflict(format!(
            "repo '{repo_name}' is already in task '{task_id}'"
        )));
    }

    let branch_name = branch
        .map(String::from)
        .or_else(|| task.repos.first().map(|r| r.branch.clone()))
        .unwrap_or_else(|| task_id.to_string());
    let worktree_path = task.path.join(repo_name);

    let repo_entry = db
        .get_repo(repo_name)?
        .ok_or_else(|| GroveError::RepoNotRegistered(repo_name.to_string()))?;

    let bare_path = repo_entry.path.clone();
    let base_branch = base
        .map(String::from)
        .unwrap_or_else(|| repo_entry.default_branch.clone());

    git::create_worktree(
        &bare_path,
        &worktree_path,
        &branch_name,
        &base_branch,
        verbose,
    )?;

    task.repos.push(TaskRepo {
        repo_name: repo_name.to_string(),
        worktree_path: worktree_path.clone(),
        branch: branch_name.clone(),
    });
    db.upsert_task(&task)?;

    let data = serde_json::json!({
        "task_id": task_id,
        "repo": repo_name,
        "worktree_path": worktree_path,
        "branch": branch_name,
    });
    output::success(
        json_mode,
        &format!("Added repo '{repo_name}' to task '{task_id}' (branch: {branch_name})"),
        data,
    );

    Ok(())
}
