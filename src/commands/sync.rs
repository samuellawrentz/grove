use chrono::Utc;
use std::sync::{Arc, Condvar, Mutex};

use crate::config::GroveConfig;
use crate::db::Db;
use crate::error::GroveError;
use crate::git;
use crate::output;

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
        let mut count = self.count.lock().expect("mutex poisoned");
        while *count == 0 {
            count = self.cond.wait(count).expect("mutex poisoned");
        }
        *count -= 1;
    }

    fn release(&self) {
        let mut count = self.count.lock().expect("mutex poisoned");
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
    db: &Db,
    json_mode: bool,
    verbose: bool,
) -> Result<(), GroveError> {
    let all_repos = db.list_repos()?;

    let repos_to_sync: Vec<(String, std::path::PathBuf, String)> = if let Some(name) = repo_name {
        let entry = all_repos
            .iter()
            .find(|r| r.name == name)
            .ok_or_else(|| GroveError::RepoNotRegistered(format!("'{name}' is not registered")))?;
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
    let tracked_branches = config.git.tracked_branches.clone();
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
            let tracked_branches = tracked_branches.clone();
            s.spawn(move || {
                sem.acquire();
                if !json_mode {
                    eprintln!("Syncing {}...", name);
                }
                let result = git::fetch_repo(&path, prune, verbose).and_then(|()| {
                    for branch in &tracked_branches {
                        git::update_default_branch(&path, branch, verbose)?;
                    }
                    if !tracked_branches.iter().any(|b| b == &default_branch) {
                        git::update_default_branch(&path, &default_branch, verbose)?;
                    }
                    Ok(())
                });
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
                results.lock().expect("mutex poisoned").push(sync_result);
                sem.release();
            });
        }
    });

    let results = Arc::try_unwrap(results)
        .expect("arc still shared")
        .into_inner()
        .expect("mutex poisoned");

    // Update last_synced_at for successful repos
    let now = Utc::now();
    for r in &results {
        if r.ok {
            if let Some(mut entry) = db.get_repo(&r.repo)? {
                entry.last_synced_at = Some(now);
                db.upsert_repo(&entry)?;
            }
        }
    }

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
