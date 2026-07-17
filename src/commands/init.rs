use chrono::Utc;
use dialoguer::{Input, MultiSelect};

use crate::commands::rollback::StepJournal;
use crate::commands::Ctx;
use crate::db::{Db, TaskEntry, TaskRepo};
use crate::error::GroveError;
use crate::git;
use crate::output;
use crate::validation::validate_identifier;

pub struct InitOptions<'a> {
    pub repos: &'a [String],
    pub context: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub base: Option<&'a str>,
    pub interactive: bool,
}

fn interactive_prompt(
    task_id: &str,
    cli_repos: &[String],
    cli_branch: Option<&str>,
    db: &Db,
) -> Result<(Vec<String>, String), GroveError> {
    let all_repos = db.list_repos()?;
    if all_repos.is_empty() {
        return Err(GroveError::General(
            "no repos registered. Use `grove register` first.".to_string(),
        ));
    }

    let selected_repos = if cli_repos.is_empty() {
        let repo_names: Vec<String> = all_repos.iter().map(|r| r.name.clone()).collect();

        let selections = MultiSelect::new()
            .with_prompt("Select repos for this task")
            .items(&repo_names)
            .interact()
            .map_err(|e| GroveError::General(format!("interactive selection failed: {e}")))?;

        if selections.is_empty() {
            return Err(GroveError::General("no repos selected".to_string()));
        }

        selections
            .into_iter()
            .map(|i| repo_names[i].clone())
            .collect()
    } else {
        cli_repos.to_vec()
    };

    let branch = if let Some(b) = cli_branch {
        b.to_string()
    } else {
        Input::new()
            .with_prompt("Branch name")
            .default(task_id.to_string())
            .interact_text()
            .map_err(|e| GroveError::General(format!("interactive input failed: {e}")))?
    };

    Ok((selected_repos, branch))
}

/// Create a task: worktrees + CONTEXT.md, nothing more.
///
/// Multiplexer/agent orchestration is herdr's job now. `init` only provisions
/// the git worktrees and emits the JSON (`task_id`, `path`, `repos`, `branch`) a
/// herdr-side launcher consumes to build the workspace and start agents.
pub fn run(task_id: Option<&str>, opts: &InitOptions, ctx: &Ctx) -> Result<(), GroveError> {
    let config = ctx.config;
    let db = ctx.db;
    let json_mode = ctx.json_mode;
    let verbose = ctx.verbose;

    let resolved_task_id = match task_id {
        Some(id) => id.to_string(),
        None => {
            if !opts.interactive {
                return Err(GroveError::General(
                    "task_id is required (use -i for interactive mode)".to_string(),
                ));
            }
            dialoguer::Input::new()
                .with_prompt("Task ID")
                .interact_text()
                .map_err(|e| GroveError::General(format!("interactive input failed: {e}")))?
        }
    };
    let task_id = resolved_task_id.as_str();

    validate_identifier(task_id, "task-id")?;

    let (resolved_repos, resolved_branch) = if opts.interactive {
        interactive_prompt(task_id, opts.repos, opts.branch, db)?
    } else {
        if opts.repos.is_empty() {
            return Err(GroveError::General(
                "at least one repo must be specified (use -i for interactive mode)".to_string(),
            ));
        }
        let b = opts.branch.unwrap_or(task_id).to_string();
        (opts.repos.to_vec(), b)
    };

    // Validate all repo names are registered
    let all_repos = db.list_repos()?;
    for repo_name in &resolved_repos {
        if !all_repos.iter().any(|r| r.name == *repo_name) {
            return Err(GroveError::RepoNotRegistered(repo_name.clone()));
        }
    }

    // Idempotency: check if task already exists
    if let Some(existing) = db.get_task(task_id)? {
        if existing.is_stale() {
            eprintln!(
                "Warning: task '{task_id}' has stale state (directories missing). Re-creating."
            );
            for task_repo in &existing.repos {
                if let Some(repo_entry) = all_repos.iter().find(|r| r.name == task_repo.repo_name) {
                    if repo_entry.path.exists() {
                        let _ =
                            git::run_git(&["worktree", "prune"], Some(&repo_entry.path), verbose);
                        // Safe delete (-d): preserve unmerged commits. Force
                        // removal stays an explicit, user-driven action (close -D).
                        if let Err(e) =
                            git::delete_branch(&repo_entry.path, &task_repo.branch, false, verbose)
                        {
                            eprintln!(
                                "Warning: kept unmerged branch '{}' for repo '{}' during stale re-init: {e}",
                                task_repo.branch, task_repo.repo_name
                            );
                        }
                    }
                }
            }
            db.delete_task(task_id)?;
        } else {
            let mut existing_repos: Vec<&str> = existing
                .repos
                .iter()
                .map(|r| r.repo_name.as_str())
                .collect();
            existing_repos.sort();

            let mut requested_repos: Vec<&str> =
                resolved_repos.iter().map(|s| s.as_str()).collect();
            requested_repos.sort();

            if existing_repos == requested_repos {
                let data = serde_json::json!({
                    "task_id": task_id,
                    "path": existing.path,
                    "repos": &existing.repos.iter().map(|r| r.repo_name.as_str()).collect::<Vec<_>>(),
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

    let branch_name = &resolved_branch;
    let task_dir = config.tasks_dir.join(task_id);
    std::fs::create_dir_all(&task_dir)?;

    // Journal external side-effects; DB writes go last inside a terminal tx.
    let mut journal = StepJournal::new(verbose);
    journal.dir(&task_dir);

    let mut task_repos: Vec<TaskRepo> = Vec::new();
    for repo_name in &resolved_repos {
        let repo_entry = crate::commands::util::resolve_repo(&all_repos, repo_name)?;
        let bare_path = &repo_entry.path;
        let base_branch = opts.base.unwrap_or(&repo_entry.default_branch);
        let worktree_path = task_dir.join(repo_name);

        crate::commands::util::provision_worktree(
            &mut journal,
            bare_path,
            &worktree_path,
            branch_name,
            base_branch,
            verbose,
        )?;

        task_repos.push(TaskRepo {
            repo_name: repo_name.clone(),
            worktree_path,
            branch: branch_name.to_string(),
        });
    }

    let now = Utc::now();

    let context_content = if let Some(ctx) = opts.context {
        ctx.to_string()
    } else {
        format!(
            "# Task: {task_id}\n\n\
             **Repos:** {}\n\
             **Created:** {}\n\n\
             ## Description\n\n\
             _Add task description here._\n",
            resolved_repos.join(", "),
            now.format("%Y-%m-%d")
        )
    };
    std::fs::write(task_dir.join("CONTEXT.md"), &context_content)?;

    let task_entry = TaskEntry {
        id: task_id.to_string(),
        path: task_dir.clone(),
        repos: task_repos,
        created_at: now,
        // Vestigial columns: herdr owns pane/agent state now, resolved by cwd.
        tmux_window: None,
        pane_id: None,
    };

    // Terminal DB transaction: all DB writes go here, last, so a failure rolls
    // itself back and the journal then unwinds the external state. On success,
    // commit() disarms the journal.
    db.transaction(|| {
        db.upsert_task(&task_entry)?;
        db.upsert_project(&task_dir.to_string_lossy())?;
        Ok(())
    })?;
    journal.commit();

    let data = serde_json::json!({
        "task_id": task_id,
        "path": task_dir,
        "repos": &resolved_repos,
        "branch": branch_name,
        "already_existed": false,
    });
    output::success(
        json_mode,
        &format!(
            "Created task '{task_id}' with repos: {} (branch: {branch_name})",
            resolved_repos.join(", ")
        ),
        data,
    );

    Ok(())
}
