use chrono::Utc;
use dialoguer::{Input, MultiSelect};

use crate::agent;
use crate::commands::rollback::StepJournal;
use crate::commands::Ctx;
use crate::config::GroveConfig;
use crate::db::{Db, TaskEntry, TaskRepo};
use crate::error::GroveError;
use crate::git;
use crate::output;
use crate::tmux;
use crate::validation::validate_identifier;

pub struct InitOptions<'a> {
    pub repos: &'a [String],
    pub context: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub base: Option<&'a str>,
    pub interactive: bool,
    pub no_tmux: bool,
    pub no_claude: bool,
    pub no_agent: bool,
    pub no_attach: bool,
    pub agent: Option<&'a str>,
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

    // Merge no_claude || no_agent into a single effective flag for the agent
    // launch decision (preserves the old main.rs behavior).
    let no_claude = opts.no_claude || opts.no_agent;

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

    let mut tmux_window: Option<String> = None;
    let mut pane_id: Option<String> = None;
    let mut launched_kind: Option<agent::AgentKind> = None;

    if !opts.no_tmux {
        if !tmux::is_tmux_available() {
            if verbose {
                eprintln!("Warning: tmux not available, skipping window creation");
            }
        } else if !tmux::is_inside_tmux() {
            if verbose {
                eprintln!("Warning: not inside tmux, skipping window creation");
            }
        } else {
            match create_tmux_window(task_id, &task_dir, opts, no_claude, config, verbose) {
                Ok((window, pane, launched_agent, created)) => {
                    if created {
                        journal.tmux_window(&window);
                    }
                    tmux_window = Some(window);
                    pane_id = Some(pane);
                    launched_kind = launched_agent;
                }
                Err(e) => {
                    eprintln!("Warning: tmux window creation failed: {e}");
                }
            }
        }
    }

    let task_entry = TaskEntry {
        id: task_id.to_string(),
        path: task_dir.clone(),
        repos: task_repos,
        created_at: now,
        tmux_window: tmux_window.clone(),
        pane_id: pane_id.clone(),
    };

    // Terminal DB transaction: all DB writes go here, last, so a failure rolls
    // itself back (no DB-inverse closures in the journal) and the journal then
    // unwinds the external state. On success, commit() disarms the journal.
    db.transaction(|| {
        if let (Some(kind), Some(pane)) = (launched_kind, pane_id.as_deref()) {
            crate::agent::PaneAgentStore::new(db).record(pane, kind)?;
        }
        db.upsert_task(&task_entry)?;
        db.upsert_project(&task_dir.to_string_lossy())?;
        Ok(())
    })?;
    journal.commit();

    if let Some(ref target) = tmux_window {
        if config.auto_attach && !opts.no_attach {
            let _ = tmux::select_window(target, verbose);
        }
    }

    let data = serde_json::json!({
        "task_id": task_id,
        "path": task_dir,
        "repos": &resolved_repos,
        "branch": branch_name,
        "tmux_window": tmux_window,
        "pane_id": pane_id,
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

/// Returns `(window_target, pane_id, launched_kind, created)`. `created` is
/// false when an existing window was reused, so the caller does not journal a
/// kill-window undo for a window it did not create.
fn create_tmux_window(
    task_id: &str,
    task_dir: &std::path::Path,
    opts: &InitOptions,
    no_claude: bool,
    config: &GroveConfig,
    verbose: bool,
) -> Result<(String, String, Option<agent::AgentKind>, bool), GroveError> {
    let session = tmux::current_session(verbose)?;
    let window_name = format!("{}-{}", config.tmux.session_prefix, task_id);
    let window_target = format!("{session}:{window_name}");

    let (pane_id, created) = if tmux::window_exists(&session, &window_name, verbose) {
        if verbose {
            eprintln!("tmux window '{window_name}' already exists, reusing");
        }
        // Resolve the reused window's pane from the real pane list. `display-message
        // -t <target>` cannot be trusted here: for a target that does not exist it
        // answers with the *active* pane and exits 0.
        let panes = tmux::list_all_panes(verbose)?;
        let pane = tmux::locate_task_pane(&panes, None, Some(&window_target), task_dir)
            .ok_or_else(|| {
                GroveError::TmuxNotRunning(format!(
                    "could not find pane for window '{window_name}'"
                ))
            })?;
        (pane.pane_id.clone(), false)
    } else {
        let pane_id = tmux::new_named_window(&session, &window_name, task_dir, verbose)?;
        (pane_id, true)
    };

    let mut launched_kind: Option<agent::AgentKind> = None;
    if !no_claude && config.auto_launch_claude {
        let agent_name = opts.agent.unwrap_or("claude");
        let cmd = config.resolved_agent_command(agent_name);
        // Target the pane directly — stable regardless of window renaming.
        agent::launch_in_pane(&pane_id, &cmd, verbose)?;
        launched_kind =
            agent::AgentKind::parse(agent_name).or_else(|| agent::AgentKind::from_command(&cmd));
    }

    Ok((window_target, pane_id, launched_kind, created))
}
