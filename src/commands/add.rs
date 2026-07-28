use crate::commands::rollback::StepJournal;
use crate::commands::Ctx;
use crate::db::TaskRepo;
use crate::error::GroveError;
use crate::output;
use crate::validation::{validate_commitish, validate_identifier, validate_ref_name};

pub fn run(
    task_id: &str,
    repo_name: &str,
    branch: Option<&str>,
    base: Option<&str>,
    dir: Option<&str>,
    detach: Option<&str>,
    ctx: &Ctx,
) -> Result<(), GroveError> {
    let db = ctx.db;
    let json_mode = ctx.json_mode;
    let verbose = ctx.verbose;

    validate_identifier(repo_name, "repo")?;
    // Branch and base are ref names, not path segments: they legitimately carry
    // `/` (`user/ser-1234-thing`), so they get the ref-name rules, NOT the
    // single-path-component rule below.
    if let Some(b) = branch {
        validate_ref_name(b, "branch")?;
    }
    if let Some(b) = base {
        validate_ref_name(b, "base")?;
    }
    if let Some(c) = detach {
        validate_commitish(c, "commit")?;
    }
    // The alias is a caller-supplied path segment joined onto the task dir, and
    // close() later hands that path to remove_dir_all — same hazard class as the
    // task-id, so it goes through the same single-component validation.
    if let Some(d) = dir {
        validate_identifier(d, "dir")?;
    }

    let mut task = db
        .get_task(task_id)?
        .ok_or_else(|| GroveError::TaskNotFound(task_id.to_string()))?;

    // Without --dir the old rule stands: a repo goes into a task once. A second
    // worktree of the same repo has to be asked for explicitly.
    if dir.is_none() && task.repos.iter().any(|r| r.repo_name == repo_name) {
        return Err(GroveError::Conflict(format!(
            "repo '{repo_name}' is already in task '{task_id}'"
        )));
    }
    let dir_name = dir.unwrap_or(repo_name);
    if task.repos.iter().any(|r| r.dir_name() == dir_name) {
        return Err(GroveError::Conflict(format!(
            "task '{task_id}' already has a worktree at '{dir_name}'"
        )));
    }

    // A detached worktree is on no branch at all, and an empty branch is how the
    // record says so — close() then knows there is nothing to delete, and the
    // next `add` inherits the task's real branch, not this hole.
    let branch_name = if detach.is_some() {
        String::new()
    } else {
        branch
            .map(String::from)
            .or_else(|| {
                task.repos
                    .iter()
                    .find(|r| !r.branch.is_empty())
                    .map(|r| r.branch.clone())
            })
            .unwrap_or_else(|| task_id.to_string())
    };
    let worktree_path = task.path.join(dir_name);

    let repo_entry = db
        .get_repo(repo_name)?
        .ok_or_else(|| GroveError::RepoNotRegistered(repo_name.to_string()))?;

    let bare_path = repo_entry.path.clone();
    let base_branch = base
        .map(String::from)
        .unwrap_or_else(|| repo_entry.default_branch.clone());

    // Journal the worktree (B2: add had zero rollback); the DB write goes last
    // inside a terminal tx so a failure rolls back and the journal unwinds.
    let mut journal = StepJournal::new(verbose);
    match detach {
        Some(commit) => crate::commands::util::provision_detached_worktree(
            &mut journal,
            &bare_path,
            &worktree_path,
            commit,
            verbose,
        )?,
        None => crate::commands::util::provision_worktree(
            &mut journal,
            &bare_path,
            &worktree_path,
            &branch_name,
            &base_branch,
            verbose,
        )?,
    }

    task.repos.push(TaskRepo {
        repo_name: repo_name.to_string(),
        worktree_path: worktree_path.clone(),
        branch: branch_name.clone(),
    });
    db.transaction(|| db.upsert_task(&task))?;
    journal.commit();

    let data = serde_json::json!({
        "task_id": task_id,
        "repo": repo_name,
        "dir": dir_name,
        "worktree_path": worktree_path,
        "branch": branch_name,
        "detached": detach,
    });
    // Name the directory only when it is not the repo name, so the default
    // message is byte-identical to what callers saw before --dir existed.
    let at = if dir_name == repo_name {
        String::new()
    } else {
        format!(" at '{dir_name}'")
    };
    let checkout = match detach {
        Some(commit) => format!("detached at {commit}"),
        None => format!("branch: {branch_name}"),
    };
    output::success(
        json_mode,
        &format!("Added repo '{repo_name}' to task '{task_id}'{at} ({checkout})"),
        data,
    );

    Ok(())
}
