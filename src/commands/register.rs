use chrono::Utc;

use crate::config::GroveConfig;
use crate::db::{Db, RepoEntry};
use crate::error::GroveError;
use crate::git;
use crate::output;
use crate::validation::validate_identifier;

pub fn run(
    name: &str,
    url: &str,
    config: &GroveConfig,
    db: &Db,
    json_mode: bool,
    verbose: bool,
) -> Result<(), GroveError> {
    validate_identifier(name, "repo name")?;

    // Check if already registered
    if let Some(existing) = db.get_repo(name)? {
        if existing.url == url {
            let data = serde_json::json!({
                "name": existing.name,
                "url": existing.url,
                "path": existing.path,
                "default_branch": existing.default_branch,
                "already_registered": true,
            });
            output::success(
                json_mode,
                &format!("Repository '{name}' already registered"),
                data,
            );
            return Ok(());
        } else {
            return Err(GroveError::Conflict(format!(
                "repository '{name}' already registered with different URL: {}",
                existing.url
            )));
        }
    }

    std::fs::create_dir_all(&config.repos_dir)?;

    let bare_path = config.repos_dir.join(format!("{name}.git"));
    if bare_path.exists() {
        return Err(GroveError::General(format!(
            "bare repo directory already exists: {}",
            bare_path.display()
        )));
    }

    let default_branch = git::bare_clone(url, &bare_path, verbose)?;

    let entry = RepoEntry {
        name: name.to_string(),
        url: url.to_string(),
        path: bare_path.clone(),
        default_branch: default_branch.clone(),
        registered_at: Utc::now(),
        last_synced_at: None,
    };

    db.upsert_repo(&entry)?;

    let data = serde_json::json!({
        "name": name,
        "url": url,
        "path": bare_path,
        "default_branch": default_branch,
        "already_registered": false,
    });
    output::success(
        json_mode,
        &format!("Registered '{name}' (default branch: {default_branch})"),
        data,
    );

    Ok(())
}
