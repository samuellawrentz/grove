use chrono::Utc;
use std::sync::{Arc, Condvar, Mutex};

use crate::config::GroveConfig;
use crate::error::GroveError;
use crate::git;
use crate::output;
use crate::state::GroveState;

/// Simple counting semaphore using Mutex + Condvar.
struct Semaphore {
    count: Mutex<usize>,
    cond: Condvar,
}

impl Semaphore {
    fn new(permits: usize) -> Self {
        Self {
            count: Mutex::new(permits),
            cond: Condvar::new(),
        }
    }

    fn acquire(&self) {
        let mut count = self.count.lock().unwrap();
        while *count == 0 {
            count = self.cond.wait(count).unwrap();
        }
        *count -= 1;
    }

    fn release(&self) {
        let mut count = self.count.lock().unwrap();
        *count += 1;
        self.cond.notify_one();
    }
}

#[derive(Debug)]
struct SyncResult {
    repo: String,
    ok: bool,
    error: Option<String>,
}

pub fn run(
    repo_name: Option<&str>,
    config: &GroveConfig,
    state: &mut GroveState,
    json_mode: bool,
    verbose: bool,
) -> Result<(), GroveError> {
    let repos_to_sync: Vec<(String, std::path::PathBuf, String)> = if let Some(name) = repo_name {
        let entry = state
            .repos
            .get(name)
            .ok_or_else(|| GroveError::RepoNotRegistered(format!("'{name}' is not registered")))?;
        vec![(
            entry.name.clone(),
            entry.path.clone(),
            entry.default_branch.clone(),
        )]
    } else {
        if state.repos.is_empty() {
            let data = serde_json::json!({ "results": [] });
            output::success(json_mode, "No repos to sync", data);
            return Ok(());
        }
        let mut repos: Vec<_> = state
            .repos
            .values()
            .map(|r| (r.name.clone(), r.path.clone(), r.default_branch.clone()))
            .collect();
        repos.sort_by(|a, b| a.0.cmp(&b.0));
        repos
    };

    let prune = config.git.fetch_prune;
    let max_parallel = config.max_parallel_syncs;
    let results = Arc::new(Mutex::new(Vec::new()));
    let semaphore = Arc::new(Semaphore::new(max_parallel));

    std::thread::scope(|s| {
        for (name, path, default_branch) in &repos_to_sync {
            let results = Arc::clone(&results);
            let sem = Arc::clone(&semaphore);
            let name = name.clone();
            let path = path.clone();
            let default_branch = default_branch.clone();
            s.spawn(move || {
                sem.acquire();
                if !json_mode {
                    eprintln!("Syncing {}...", name);
                }
                let result = git::fetch_repo(&path, prune, verbose)
                    .and_then(|()| git::update_default_branch(&path, &default_branch, verbose));
                let sync_result = match result {
                    Ok(()) => SyncResult {
                        repo: name.clone(),
                        ok: true,
                        error: None,
                    },
                    Err(e) => SyncResult {
                        repo: name.clone(),
                        ok: false,
                        error: Some(e.to_string()),
                    },
                };
                results.lock().unwrap().push(sync_result);
                sem.release();
            });
        }
    });

    let results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();

    // Update last_synced_at for successful repos
    let now = Utc::now();
    for r in &results {
        if r.ok {
            if let Some(entry) = state.repos.get_mut(&r.repo) {
                entry.last_synced_at = Some(now);
            }
        }
    }
    state.save()?;

    // Output
    let all_ok = results.iter().all(|r| r.ok);
    let result_data: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            let mut obj = serde_json::json!({
                "repo": r.repo,
                "ok": r.ok,
            });
            if let Some(ref e) = r.error {
                obj.as_object_mut()
                    .unwrap()
                    .insert("error".to_string(), serde_json::json!(e));
            }
            obj
        })
        .collect();

    let data = serde_json::json!({ "results": result_data });

    if json_mode {
        output::success(true, "", data);
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
        if all_ok {
            println!("All repos synced successfully");
        } else {
            println!("Some repos failed to sync");
        }
    }

    Ok(())
}
