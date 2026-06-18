use std::path::Path;

use chrono::Utc;
use serde::Serialize;

use crate::commands::Ctx;
use crate::error::GroveError;
use crate::git;
use crate::output;

#[derive(Debug, Serialize)]
struct SyncResult {
    repo: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Fetch one repo and fast-forward its tracked branches. Never panics or
/// returns Err — failures are captured into the `SyncResult` so a single bad
/// repo cannot abort the whole run.
fn sync_one(
    name: &str,
    path: &Path,
    default_branch: &str,
    tracked_branches: &[String],
    prune: bool,
    json_mode: bool,
    verbose: bool,
) -> SyncResult {
    if !json_mode {
        eprintln!("Syncing {name}...");
    }
    let result = git::fetch_repo(path, prune, verbose).and_then(|()| {
        for branch in tracked_branches {
            git::update_default_branch(path, branch, verbose)?;
        }
        if !tracked_branches.iter().any(|b| b == default_branch) {
            git::update_default_branch(path, default_branch, verbose)?;
        }
        Ok(())
    });
    match result {
        Ok(()) => SyncResult {
            repo: name.to_string(),
            ok: true,
            error: None,
        },
        Err(e) => SyncResult {
            repo: name.to_string(),
            ok: false,
            error: Some(e.to_string()),
        },
    }
}

pub fn run(repo_name: Option<&str>, ctx: &Ctx) -> Result<(), GroveError> {
    let config = ctx.config;
    let db = ctx.db;
    let json_mode = ctx.json_mode;
    let verbose = ctx.verbose;

    let all_repos = db.list_repos()?;

    let repos_to_sync: Vec<(String, std::path::PathBuf, String)> = if let Some(name) = repo_name {
        let entry = crate::commands::util::resolve_repo(&all_repos, name)?;
        vec![(
            entry.name.clone(),
            entry.path.clone(),
            entry.default_branch.clone(),
        )]
    } else {
        if all_repos.is_empty() {
            let data = serde_json::json!({ "results": [] });
            output::success(json_mode, "No repos to sync", data);
            return Ok(());
        }
        all_repos
            .iter()
            .map(|r| (r.name.clone(), r.path.clone(), r.default_branch.clone()))
            .collect()
    };

    let prune = config.git.fetch_prune;
    let tracked_branches = &config.git.tracked_branches;
    // `.max(1)` removes the deadlock structurally even if the config clamp is
    // ever bypassed; chunked fan-out bounds concurrency without a semaphore.
    let max_parallel = config.max_parallel_syncs.max(1);

    let mut results: Vec<SyncResult> = Vec::with_capacity(repos_to_sync.len());
    for chunk in repos_to_sync.chunks(max_parallel) {
        let chunk_results: Vec<SyncResult> = std::thread::scope(|s| {
            let handles: Vec<(String, _)> = chunk
                .iter()
                .map(|(name, path, default_branch)| {
                    let handle = s.spawn(|| {
                        sync_one(
                            name,
                            path,
                            default_branch,
                            tracked_branches,
                            prune,
                            json_mode,
                            verbose,
                        )
                    });
                    (name.clone(), handle)
                })
                .collect();
            handles
                .into_iter()
                .map(|(name, h)| {
                    h.join().unwrap_or_else(|_| SyncResult {
                        repo: name,
                        ok: false,
                        error: Some("sync worker panicked".to_string()),
                    })
                })
                .collect()
        });
        results.extend(chunk_results);
    }

    let all_ok = results.iter().all(|r| r.ok);

    // Emit results BEFORE persisting last_synced_at, so a touch failure cannot
    // suppress the report the caller is waiting on.
    if json_mode {
        let data = serde_json::json!({ "results": &results });
        output::envelope(json_mode, all_ok, "", data);
    } else {
        for r in &results {
            if r.ok {
                println!("  {} ok", r.repo);
            } else {
                println!(
                    "  {} FAILED: {}",
                    r.repo,
                    r.error.as_deref().unwrap_or("unknown error")
                );
            }
        }
        println!(
            "{}",
            if all_ok {
                "All repos synced successfully"
            } else {
                "Some repos failed to sync"
            }
        );
    }

    // Persist last_synced_at for successful repos; collect (don't abort on) errors.
    let now = Utc::now();
    for r in &results {
        if r.ok {
            if let Err(e) = db.touch_repo_synced(&r.repo, now) {
                eprintln!("Warning: failed to record sync time for '{}': {e}", r.repo);
            }
        }
    }

    if all_ok {
        Ok(())
    } else {
        Err(GroveError::General(
            "one or more repos failed to sync".to_string(),
        ))
    }
}
