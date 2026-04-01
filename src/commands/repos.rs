use crate::db::Db;
use crate::error::GroveError;
use crate::output;

pub fn run(db: &Db, json_mode: bool) -> Result<(), GroveError> {
    let repos = db.list_repos()?;
    if repos.is_empty() {
        let data = serde_json::json!({ "repos": [] });
        output::success(json_mode, "No repos registered", data);
        return Ok(());
    }

    if json_mode {
        let repo_list: Vec<serde_json::Value> = repos
            .iter()
            .map(|r| {
                let exists = r.path.exists();
                serde_json::json!({
                    "name": r.name,
                    "url": r.url,
                    "path": r.path,
                    "default_branch": r.default_branch,
                    "registered_at": r.registered_at,
                    "last_synced_at": r.last_synced_at,
                    "exists": exists,
                })
            })
            .collect();
        let data = serde_json::json!({ "repos": repo_list });
        output::success(true, "", data);
    } else {
        println!("{:<20} {:<40} {:<12} STATUS", "NAME", "URL", "BRANCH");
        for r in &repos {
            let status = if r.path.exists() { "ok" } else { "MISSING" };
            let synced = r
                .last_synced_at
                .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "never".to_string());
            println!(
                "{:<20} {:<40} {:<12} {} (synced: {})",
                r.name, r.url, r.default_branch, status, synced
            );
        }
    }

    Ok(())
}
