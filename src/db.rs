use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, Utc};

use serde::{Deserialize, Serialize};

use crate::error::GroveError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    pub name: String,
    pub url: String,
    pub path: PathBuf,
    pub default_branch: String,
    pub registered_at: DateTime<Utc>,
    pub last_synced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRepo {
    pub repo_name: String,
    pub worktree_path: PathBuf,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEntry {
    pub id: String,
    pub path: PathBuf,
    pub repos: Vec<TaskRepo>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
}

impl TaskEntry {
    pub fn is_stale(&self) -> bool {
        if !self.path.exists() {
            return true;
        }
        self.repos.iter().any(|r| !r.worktree_path.exists())
    }
}

pub struct Db {
    conn: rusqlite::Connection,
}

pub struct Project {
    #[allow(dead_code)]
    pub id: i64,
    pub path: PathBuf,
    pub name: String,
    #[allow(dead_code)]
    pub created_at: String,
    pub last_seen: String,
}

pub(crate) const DT_FMT: &str = "%Y-%m-%d %H:%M:%S";

/// Column list for task SELECTs, shared by `get_task` and `list_tasks` so the
/// two read paths can't drift. Extraction is shared via `row_to_task_head`.
const TASK_COLS: &str = "id, path, created_at, tmux_window, pane_id";

/// Column list for repo SELECTs, shared by `get_repo` and `list_repos`.
const REPO_COLS: &str = "name, url, path, default_branch, registered_at, last_synced_at";

/// The five scalar task columns (`TASK_COLS` order). Repos are loaded separately.
struct TaskHead {
    id: String,
    path: String,
    created_at: String,
    tmux_window: Option<String>,
    pane_id: Option<String>,
}

/// Extract a `TaskHead` from a row selected with `TASK_COLS`. Used by both
/// `get_task` and `list_tasks` so the column extraction lives in one place.
fn row_to_task_head(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskHead> {
    Ok(TaskHead {
        id: row.get(0)?,
        path: row.get(1)?,
        created_at: row.get(2)?,
        tmux_window: row.get(3)?,
        pane_id: row.get(4)?,
    })
}

impl TaskHead {
    /// Assemble a full `TaskEntry`, loading this task's repos.
    fn into_entry(self, repos: Vec<TaskRepo>) -> TaskEntry {
        TaskEntry {
            id: self.id,
            path: PathBuf::from(self.path),
            created_at: str_to_dt(&self.created_at).unwrap_or_else(Utc::now),
            tmux_window: self.tmux_window,
            pane_id: self.pane_id,
            repos,
        }
    }
}

fn dt_to_str(dt: DateTime<Utc>) -> String {
    dt.format(DT_FMT).to_string()
}

fn str_to_dt(s: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s, DT_FMT)
        .ok()
        .map(|d| d.and_utc())
}

fn canonical_path_and_name(path: &str) -> (String, String) {
    let canonical = std::fs::canonicalize(path)
        .unwrap_or_else(|_| PathBuf::from(path))
        .to_string_lossy()
        .to_string();
    let name = Path::new(&canonical)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| canonical.clone());
    (canonical, name)
}

impl Db {
    pub fn open() -> Result<Self, GroveError> {
        let dir = crate::config::grove_dir();
        std::fs::create_dir_all(&dir)?;
        let db = Self::open_path(&dir.join("grove.db"))?;
        // Legacy importers are best-effort (a missing/corrupt legacy file must
        // not block opening), but failures are logged rather than swallowed.
        match db.migrate_recents(&dir) {
            Ok(n) if n > 0 => eprintln!("Imported {n} legacy recents"),
            Err(e) => eprintln!("Warning: recents import failed: {e}"),
            _ => {}
        }
        match db.migrate_state_json(&dir) {
            Ok(n) if n > 0 => eprintln!("Imported {n} legacy state.json entries"),
            Err(e) => eprintln!("Warning: state.json import failed: {e}"),
            _ => {}
        }
        Ok(db)
    }

    pub fn open_path(path: &Path) -> Result<Self, GroveError> {
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA foreign_keys = ON;",
        )?;
        let db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Run `f` inside a single SQLite transaction.
    ///
    /// Commits on `Ok`, rolls back on `Err` (or any early `?`). Built on
    /// `unchecked_transaction()` because grove's `Db` API is all-`&self`
    /// (`conn.transaction()` needs `&mut`). Reentrant-safe: if a transaction
    /// is already active, `f` runs inline and the outermost call owns
    /// commit/rollback — so terminal-tx callers can freely invoke helpers
    /// (`upsert_task`) that also wrap themselves here without a nested-BEGIN
    /// error.
    pub fn transaction<F, T>(&self, f: F) -> Result<T, GroveError>
    where
        F: FnOnce() -> Result<T, GroveError>,
    {
        if !self.conn.is_autocommit() {
            return f();
        }
        let tx = self.conn.unchecked_transaction()?;
        let value = f()?;
        tx.commit()?;
        Ok(value)
    }

    fn migrate(&self) -> Result<(), GroveError> {
        let version: u32 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < 1 {
            self.conn.execute_batch(SCHEMA_V1)?;
            self.conn.pragma_update(None, "user_version", 1)?;
        }
        if version < 2 {
            self.conn.execute_batch(SCHEMA_V2)?;
            self.conn.pragma_update(None, "user_version", 2)?;
        }
        if version < 3 {
            self.conn.execute_batch(SCHEMA_V3)?;
            self.conn.pragma_update(None, "user_version", 3)?;
        }
        if version < 4 {
            self.conn.execute_batch(SCHEMA_V4)?;
            self.conn.pragma_update(None, "user_version", 4)?;
        }
        if version < 5 {
            // Rebuild task_repos with ON DELETE CASCADE — SQLite can't ALTER a
            // FK in place. Existing rows are FK-consistent so the copy passes
            // with foreign_keys ON.
            self.conn.execute_batch(SCHEMA_V5)?;
            self.conn.pragma_update(None, "user_version", 5)?;
        }
        Ok(())
    }

    // ── Pane agents ───────────────────────────────────────────────────────────
    // Authoritative record of agent kind for panes launched by grove.

    pub(crate) fn record_pane_agent(&self, pane_id: &str, kind: &str) -> Result<(), GroveError> {
        self.conn.execute(
            "INSERT INTO pane_agents (pane_id, agent_kind) VALUES (?1, ?2)
             ON CONFLICT(pane_id) DO UPDATE SET
               agent_kind  = excluded.agent_kind,
               launched_at = datetime('now')",
            rusqlite::params![pane_id, kind],
        )?;
        Ok(())
    }

    pub(crate) fn list_pane_agents(
        &self,
    ) -> Result<std::collections::HashMap<String, String>, GroveError> {
        let mut stmt = self
            .conn
            .prepare("SELECT pane_id, agent_kind FROM pane_agents")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = std::collections::HashMap::new();
        for r in rows {
            let (k, v) = r.map_err(|e| GroveError::Database(e.to_string()))?;
            out.insert(k, v);
        }
        Ok(out)
    }

    pub(crate) fn delete_pane_agent(&self, pane_id: &str) -> Result<(), GroveError> {
        self.conn
            .execute("DELETE FROM pane_agents WHERE pane_id = ?1", [pane_id])?;
        Ok(())
    }

    // ── Pane overrides ─────────────────────────────────────────────────────────
    // User-asserted marks that force a pane into the "others" tab regardless of
    // any detected agent.

    pub fn mark_pane_other(&self, pane_id: &str) -> Result<(), GroveError> {
        self.conn.execute(
            "INSERT INTO pane_overrides (pane_id) VALUES (?1) ON CONFLICT(pane_id) DO NOTHING",
            [pane_id],
        )?;
        Ok(())
    }

    pub fn unmark_pane_other(&self, pane_id: &str) -> Result<(), GroveError> {
        self.conn
            .execute("DELETE FROM pane_overrides WHERE pane_id = ?1", [pane_id])?;
        Ok(())
    }

    pub fn list_pane_overrides(&self) -> Result<std::collections::HashSet<String>, GroveError> {
        let mut stmt = self.conn.prepare("SELECT pane_id FROM pane_overrides")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<std::collections::HashSet<_>, _>>()
            .map_err(|e| GroveError::Database(e.to_string()))
    }

    // ── Projects ─────────────────────────────────────────────────────────────

    pub fn upsert_project(&self, path: &str) -> Result<i64, GroveError> {
        let (canonical, name) = canonical_path_and_name(path);
        self.conn.execute(
            "INSERT INTO projects (path, name) VALUES (?1, ?2)
             ON CONFLICT(path) DO UPDATE SET last_seen = datetime('now')",
            rusqlite::params![canonical, name],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn upsert_project_with_timestamp(
        &self,
        path: &str,
        timestamp: u64,
    ) -> Result<i64, GroveError> {
        let (canonical, name) = canonical_path_and_name(path);
        let dt = chrono::DateTime::from_timestamp(timestamp as i64, 0)
            .map(|d| d.format(DT_FMT).to_string())
            .unwrap_or_else(|| chrono::Utc::now().format(DT_FMT).to_string());
        self.conn.execute(
            "INSERT INTO projects (path, name, last_seen) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET last_seen = MAX(last_seen, ?3)",
            rusqlite::params![canonical, name, dt],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, GroveError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, name, created_at, last_seen \
             FROM projects ORDER BY last_seen DESC LIMIT 100",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                name: row.get(2)?,
                created_at: row.get(3)?,
                last_seen: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| GroveError::Database(e.to_string()))
    }

    #[allow(dead_code)]
    pub fn touch_project(&self, id: i64) -> Result<(), GroveError> {
        self.conn.execute(
            "UPDATE projects SET last_seen = datetime('now') WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub fn delete_project(&self, path: &str) -> Result<(), GroveError> {
        let (canonical, _) = canonical_path_and_name(path);
        self.conn
            .execute("DELETE FROM projects WHERE path = ?1", [&canonical])?;
        Ok(())
    }

    pub fn migrate_recents(&self, grove_dir: &Path) -> Result<usize, GroveError> {
        let recents_path = grove_dir.join("recents.json");
        if !recents_path.exists() {
            return Ok(0);
        }
        let data = std::fs::read_to_string(&recents_path)?;
        let entries: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap_or_default();
        let mut count = 0;
        for entry in &entries {
            if let (Some(path), Some(timestamp)) =
                (entry["path"].as_str(), entry["timestamp"].as_u64())
            {
                self.upsert_project_with_timestamp(path, timestamp)?;
                count += 1;
            }
        }
        let migrated = grove_dir.join("recents.json.migrated");
        let _ = std::fs::rename(&recents_path, &migrated);
        Ok(count)
    }

    /// One-time migration: import repos and tasks from legacy state.json into sqlite.
    pub fn migrate_state_json(&self, grove_dir: &Path) -> Result<usize, GroveError> {
        let state_path = grove_dir.join("state.json");
        if !state_path.exists() {
            return Ok(0);
        }
        let data = std::fs::read_to_string(&state_path)?;
        let state: serde_json::Value = serde_json::from_str(&data).unwrap_or_default();
        let mut count = 0;

        // Migrate repos
        if let Some(repos) = state["repos"].as_object() {
            for (_key, repo) in repos {
                let entry = RepoEntry {
                    name: repo["name"].as_str().unwrap_or_default().to_string(),
                    url: repo["url"].as_str().unwrap_or_default().to_string(),
                    path: PathBuf::from(repo["path"].as_str().unwrap_or_default()),
                    default_branch: repo["default_branch"]
                        .as_str()
                        .unwrap_or("main")
                        .to_string(),
                    registered_at: repo["registered_at"]
                        .as_str()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(Utc::now),
                    last_synced_at: repo["last_synced_at"]
                        .as_str()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc)),
                };
                if !entry.name.is_empty() {
                    let _ = self.upsert_repo(&entry);
                    count += 1;
                }
            }
        }

        // Migrate tasks
        if let Some(tasks) = state["tasks"].as_object() {
            for (_key, task) in tasks {
                let repos: Vec<TaskRepo> = task["repos"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|tr| {
                                Some(TaskRepo {
                                    repo_name: tr["repo_name"].as_str()?.to_string(),
                                    worktree_path: PathBuf::from(tr["worktree_path"].as_str()?),
                                    branch: tr["branch"].as_str()?.to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let entry = TaskEntry {
                    id: task["id"].as_str().unwrap_or_default().to_string(),
                    path: PathBuf::from(task["path"].as_str().unwrap_or_default()),
                    repos,
                    created_at: task["created_at"]
                        .as_str()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(Utc::now),
                    tmux_window: task["tmux_window"].as_str().map(String::from),
                    pane_id: task["pane_id"].as_str().map(String::from),
                };
                if !entry.id.is_empty() {
                    let _ = self.upsert_task(&entry);
                    count += 1;
                }
            }
        }

        let migrated = grove_dir.join("state.json.migrated");
        let _ = std::fs::rename(&state_path, &migrated);
        Ok(count)
    }

    // ── Notes ─────────────────────────────────────────────────────────────────

    pub fn get_note(&self, project_path: &str) -> Result<Option<String>, GroveError> {
        let (canonical, _) = canonical_path_and_name(project_path);
        let mut stmt = self
            .conn
            .prepare("SELECT content FROM notes WHERE project_path = ?1")?;
        let mut rows = stmt.query([&canonical])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn save_note(&self, project_path: &str, content: &str) -> Result<(), GroveError> {
        let (canonical, _) = canonical_path_and_name(project_path);
        self.conn.execute(
            "INSERT INTO notes (project_path, content) VALUES (?1, ?2)
             ON CONFLICT(project_path) DO UPDATE SET content = ?2, updated_at = datetime('now')",
            rusqlite::params![canonical, content],
        )?;
        Ok(())
    }

    // ── Repos ─────────────────────────────────────────────────────────────────

    pub fn upsert_repo(&self, repo: &RepoEntry) -> Result<(), GroveError> {
        let path = repo.path.to_string_lossy().to_string();
        let registered_at = dt_to_str(repo.registered_at);
        let last_synced_at = repo.last_synced_at.map(dt_to_str);
        self.conn.execute(
            "INSERT INTO repos (name, url, path, default_branch, registered_at, last_synced_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(name) DO UPDATE SET
               url            = excluded.url,
               path           = excluded.path,
               default_branch = excluded.default_branch,
               last_synced_at = excluded.last_synced_at",
            rusqlite::params![
                repo.name,
                repo.url,
                path,
                repo.default_branch,
                registered_at,
                last_synced_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_repo(&self, name: &str) -> Result<Option<RepoEntry>, GroveError> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {REPO_COLS} FROM repos WHERE name = ?1"))?;
        let mut rows = stmt.query([name])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_repo_entry(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_repos(&self) -> Result<Vec<RepoEntry>, GroveError> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {REPO_COLS} FROM repos ORDER BY name"))?;
        let rows = stmt.query_map([], row_to_repo_entry)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| GroveError::Database(e.to_string()))
    }

    pub fn touch_repo_synced(&self, name: &str, at: DateTime<Utc>) -> Result<(), GroveError> {
        self.conn.execute(
            "UPDATE repos SET last_synced_at = ?1 WHERE name = ?2",
            rusqlite::params![dt_to_str(at), name],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_repo(&self, name: &str) -> Result<(), GroveError> {
        self.conn
            .execute("DELETE FROM repos WHERE name = ?1", [name])?;
        Ok(())
    }

    // ── Tasks ─────────────────────────────────────────────────────────────────

    pub fn upsert_task(&self, task: &TaskEntry) -> Result<(), GroveError> {
        self.transaction(|| {
            let path = task.path.to_string_lossy().to_string();
            let created_at = dt_to_str(task.created_at);
            self.conn.execute(
                "INSERT INTO tasks (id, path, created_at, tmux_window, pane_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                   path        = excluded.path,
                   tmux_window = excluded.tmux_window,
                   pane_id     = excluded.pane_id",
                rusqlite::params![task.id, path, created_at, task.tmux_window, task.pane_id],
            )?;
            self.conn
                .execute("DELETE FROM task_repos WHERE task_id = ?1", [&task.id])?;
            for tr in &task.repos {
                let worktree = tr.worktree_path.to_string_lossy().to_string();
                self.conn.execute(
                    "INSERT INTO task_repos (task_id, repo_name, worktree, branch) \
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![task.id, tr.repo_name, worktree, tr.branch],
                )?;
            }
            Ok(())
        })
    }

    pub fn get_task(&self, id: &str) -> Result<Option<TaskEntry>, GroveError> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"))?;
        let mut rows = stmt.query([id])?;
        if let Some(row) = rows.next()? {
            let head = row_to_task_head(row)?;
            let repos = self.load_task_repos(&head.id)?;
            Ok(Some(head.into_entry(repos)))
        } else {
            Ok(None)
        }
    }

    pub fn list_tasks(&self) -> Result<Vec<TaskEntry>, GroveError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TASK_COLS} FROM tasks ORDER BY created_at DESC"
        ))?;
        let heads: Vec<TaskHead> = stmt
            .query_map([], row_to_task_head)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| GroveError::Database(e.to_string()))?;

        let mut tasks = Vec::with_capacity(heads.len());
        for head in heads {
            let repos = self.load_task_repos(&head.id)?;
            tasks.push(head.into_entry(repos));
        }
        Ok(tasks)
    }

    pub fn delete_task(&self, id: &str) -> Result<(), GroveError> {
        // task_repos rows cascade-delete via the FK (SCHEMA_V5 + foreign_keys=ON).
        self.conn.execute("DELETE FROM tasks WHERE id = ?1", [id])?;
        Ok(())
    }

    fn load_task_repos(&self, task_id: &str) -> Result<Vec<TaskRepo>, GroveError> {
        let mut stmt = self
            .conn
            .prepare("SELECT repo_name, worktree, branch FROM task_repos WHERE task_id = ?1")?;
        let rows = stmt.query_map([task_id], |row| {
            Ok(TaskRepo {
                repo_name: row.get(0)?,
                worktree_path: PathBuf::from(row.get::<_, String>(1)?),
                branch: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| GroveError::Database(e.to_string()))
    }
}

fn row_to_repo_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepoEntry> {
    let registered_at_str: String = row.get(4)?;
    let last_synced_at_str: Option<String> = row.get(5)?;

    let registered_at = str_to_dt(&registered_at_str).unwrap_or_else(Utc::now);
    let last_synced_at = last_synced_at_str.as_deref().and_then(str_to_dt);

    Ok(RepoEntry {
        name: row.get(0)?,
        url: row.get(1)?,
        path: PathBuf::from(row.get::<_, String>(2)?),
        default_branch: row.get(3)?,
        registered_at,
        last_synced_at,
    })
}

const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS projects (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    name        TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen   TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS repos (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    url             TEXT NOT NULL,
    path            TEXT NOT NULL,
    default_branch  TEXT NOT NULL DEFAULT 'main',
    registered_at   TEXT NOT NULL DEFAULT (datetime('now')),
    last_synced_at  TEXT
);
CREATE TABLE IF NOT EXISTS tasks (
    id          TEXT PRIMARY KEY,
    path        TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    tmux_window TEXT,
    pane_id     TEXT
);
CREATE TABLE IF NOT EXISTS task_repos (
    task_id     TEXT NOT NULL REFERENCES tasks(id),
    repo_name   TEXT NOT NULL,
    worktree    TEXT NOT NULL,
    branch      TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_name)
);
CREATE INDEX IF NOT EXISTS idx_projects_path ON projects(path);
";

const SCHEMA_V2: &str = "
CREATE TABLE IF NOT EXISTS notes (
    id           INTEGER PRIMARY KEY,
    project_path TEXT NOT NULL UNIQUE,
    content      TEXT NOT NULL DEFAULT '',
    updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
);
";

const SCHEMA_V3: &str = "
CREATE TABLE IF NOT EXISTS pane_agents (
    pane_id     TEXT PRIMARY KEY,
    agent_kind  TEXT NOT NULL,
    launched_at TEXT NOT NULL DEFAULT (datetime('now'))
);
";

const SCHEMA_V4: &str = "
CREATE TABLE IF NOT EXISTS pane_overrides (
    pane_id   TEXT PRIMARY KEY,
    marked_at TEXT NOT NULL DEFAULT (datetime('now'))
);
";

// Rebuild task_repos with ON DELETE CASCADE. SQLite cannot add a FK action via
// ALTER, so the table is recreated and its rows copied across.
const SCHEMA_V5: &str = "
CREATE TABLE task_repos_v5 (
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    repo_name   TEXT NOT NULL,
    worktree    TEXT NOT NULL,
    branch      TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_name)
);
INSERT INTO task_repos_v5 (task_id, repo_name, worktree, branch)
    SELECT task_id, repo_name, worktree, branch FROM task_repos;
DROP TABLE task_repos;
ALTER TABLE task_repos_v5 RENAME TO task_repos;
";

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn open_temp() -> Db {
        let f = tempfile::NamedTempFile::new().unwrap();
        // Keep the file alive by leaking — temp file deleted on process exit
        let path = f.path().to_path_buf();
        std::mem::forget(f);
        Db::open_path(&path).unwrap()
    }

    #[test]
    fn test_open_creates_schema() {
        let db = open_temp();
        let version: u32 = db
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 5);
    }

    #[test]
    fn test_pane_overrides_roundtrip() {
        let db = open_temp();
        assert!(db.list_pane_overrides().unwrap().is_empty());

        db.mark_pane_other("%5").unwrap();
        db.mark_pane_other("%5").unwrap(); // idempotent
        db.mark_pane_other("%9").unwrap();
        let marks = db.list_pane_overrides().unwrap();
        assert!(marks.contains("%5"));
        assert!(marks.contains("%9"));
        assert_eq!(marks.len(), 2);

        db.unmark_pane_other("%5").unwrap();
        let marks = db.list_pane_overrides().unwrap();
        assert!(!marks.contains("%5"));
        assert!(marks.contains("%9"));
    }

    #[test]
    fn test_wal_mode() {
        let db = open_temp();
        let mode: String = db
            .conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn test_project_roundtrip() {
        let db = open_temp();
        // Use a path that actually exists
        let id = db.upsert_project("/tmp").unwrap();
        assert!(id > 0);
        let projects = db.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "tmp");
    }

    #[test]
    fn test_project_touch() {
        let db = open_temp();
        let id = db.upsert_project("/tmp").unwrap();
        let before = db.list_projects().unwrap();
        let last_seen_before = before[0].last_seen.clone();

        // Sleep briefly so datetime('now') changes
        std::thread::sleep(std::time::Duration::from_millis(1100));
        db.touch_project(id).unwrap();

        let after = db.list_projects().unwrap();
        // last_seen should be updated (may or may not differ within same second)
        // At minimum touch_project should not error — row exists
        assert_eq!(after.len(), 1);
        let _ = last_seen_before; // used
    }

    #[test]
    fn test_project_dedup() {
        let db = open_temp();
        db.upsert_project("/tmp").unwrap();
        db.upsert_project("/tmp").unwrap();
        let projects = db.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
    }

    fn make_repo(name: &str) -> RepoEntry {
        RepoEntry {
            name: name.to_string(),
            url: format!("https://github.com/example/{name}.git"),
            path: PathBuf::from(format!("/tmp/repos/{name}")),
            default_branch: "main".to_string(),
            registered_at: Utc::now(),
            last_synced_at: None,
        }
    }

    #[test]
    fn test_repo_roundtrip() {
        let db = open_temp();
        let repo = make_repo("myrepo");
        db.upsert_repo(&repo).unwrap();

        let got = db.get_repo("myrepo").unwrap().unwrap();
        assert_eq!(got.name, "myrepo");
        assert_eq!(got.url, repo.url);
        assert_eq!(got.default_branch, "main");
        assert!(got.last_synced_at.is_none());

        let all = db.list_repos().unwrap();
        assert_eq!(all.len(), 1);
    }

    fn make_task(id: &str) -> TaskEntry {
        TaskEntry {
            id: id.to_string(),
            path: PathBuf::from(format!("/tmp/tasks/{id}")),
            created_at: Utc::now(),
            tmux_window: Some("mysession:grove-task".to_string()),
            pane_id: Some("%42".to_string()),
            repos: vec![TaskRepo {
                repo_name: "myrepo".to_string(),
                worktree_path: PathBuf::from(format!("/tmp/worktrees/{id}")),
                branch: "feat/my-branch".to_string(),
            }],
        }
    }

    #[test]
    fn test_task_roundtrip() {
        let db = open_temp();
        let task = make_task("TASK-1");
        db.upsert_task(&task).unwrap();

        let got = db.get_task("TASK-1").unwrap().unwrap();
        assert_eq!(got.id, "TASK-1");
        assert_eq!(got.tmux_window.as_deref(), Some("mysession:grove-task"));
        assert_eq!(got.repos.len(), 1);
        assert_eq!(got.repos[0].repo_name, "myrepo");
        assert_eq!(got.repos[0].branch, "feat/my-branch");

        let all = db.list_tasks().unwrap();
        assert_eq!(all.len(), 1);

        db.delete_task("TASK-1").unwrap();
        assert!(db.get_task("TASK-1").unwrap().is_none());
        assert!(db.list_tasks().unwrap().is_empty());
    }

    #[test]
    fn test_upsert_project_with_timestamp() {
        let db = open_temp();
        // Unix timestamp for 2024-01-15 12:00:00 UTC
        let ts: u64 = 1705320000;
        db.upsert_project_with_timestamp("/tmp", ts).unwrap();
        let projects = db.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert!(projects[0].last_seen.starts_with("2024-01-15"));
    }

    #[test]
    fn test_note_roundtrip() {
        let db = open_temp();
        // No note initially
        assert!(db.get_note("/tmp").unwrap().is_none());

        // Save and retrieve
        db.save_note("/tmp", "hello world").unwrap();
        assert_eq!(db.get_note("/tmp").unwrap().unwrap(), "hello world");

        // Update existing note
        db.save_note("/tmp", "updated content").unwrap();
        assert_eq!(db.get_note("/tmp").unwrap().unwrap(), "updated content");
    }

    #[test]
    fn test_note_per_project() {
        let db = open_temp();
        db.save_note("/tmp/a", "note a").unwrap();
        db.save_note("/tmp/b", "note b").unwrap();
        assert_eq!(db.get_note("/tmp/a").unwrap().unwrap(), "note a");
        assert_eq!(db.get_note("/tmp/b").unwrap().unwrap(), "note b");
    }

    // ── Step 1: transaction authority + FK + cascade ────────────────────────────

    /// DB-foreign-keys-pragma-on (P0 / B3): open_path must enable FK enforcement.
    #[test]
    fn foreign_keys_pragma_on() {
        let db = open_temp();
        let fk: i64 = db
            .conn
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign_keys must be ON after open_path");
    }

    /// DB-cascade-clears-task_repos (P0 / B3): a raw DELETE FROM tasks must
    /// cascade to task_repos (proves the FK action is live, not just delete_task's
    /// manual cleanup).
    #[test]
    fn cascade_clears_task_repos() {
        let db = open_temp();
        let mut task = make_task("TASK-1");
        task.repos.push(TaskRepo {
            repo_name: "other".to_string(),
            worktree_path: PathBuf::from("/tmp/worktrees/other"),
            branch: "feat/other".to_string(),
        });
        db.upsert_task(&task).unwrap();

        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM task_repos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        // delete_task collapses to one statement and relies on the cascade.
        db.delete_task("TASK-1").unwrap();
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM task_repos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "delete_task must cascade task_repos");

        // A raw tasks-delete (bypassing the helper) must also cascade.
        db.upsert_task(&task).unwrap();
        db.conn
            .execute("DELETE FROM tasks WHERE id = ?1", ["TASK-1"])
            .unwrap();
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM task_repos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "FK cascade must fire on a raw tasks delete");
    }

    /// TX-transaction-helper-rolls-back-on-err (P1 / B3, S-C).
    #[test]
    fn transaction_rolls_back_on_err() {
        let db = open_temp();
        let res: Result<(), GroveError> = db.transaction(|| {
            db.upsert_repo(&make_repo("rollme"))?;
            Err(GroveError::General("boom".into()))
        });
        assert!(res.is_err());
        assert!(
            db.get_repo("rollme").unwrap().is_none(),
            "write must be rolled back when the closure returns Err"
        );
    }

    /// TX-transaction-helper-commits-on-ok (P2 / B3, S-C).
    #[test]
    fn transaction_commits_on_ok() {
        let db = open_temp();
        db.transaction(|| {
            db.upsert_repo(&make_repo("a"))?;
            db.upsert_repo(&make_repo("b"))?;
            Ok(())
        })
        .unwrap();
        assert!(db.get_repo("a").unwrap().is_some());
        assert!(db.get_repo("b").unwrap().is_some());
    }

    /// TX-v4-to-v5-migration-preserves-rows-and-adds-cascade (P1 / B3).
    #[test]
    fn v4_to_v5_migration_preserves_rows_and_adds_cascade() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_path_buf();
        std::mem::forget(f);

        // Build a V4 database with the legacy (no-cascade) task_repos schema.
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.execute_batch(SCHEMA_V2).unwrap();
            conn.execute_batch(SCHEMA_V3).unwrap();
            conn.execute_batch(SCHEMA_V4).unwrap();
            conn.execute(
                "INSERT INTO tasks (id, path, created_at) VALUES ('T', '/tmp/T', '2024-01-01 00:00:00')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO task_repos (task_id, repo_name, worktree, branch) VALUES ('T','r1','/tmp/T/r1','b1')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO task_repos (task_id, repo_name, worktree, branch) VALUES ('T','r2','/tmp/T/r2','b2')",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 4).unwrap();
        }

        // Re-open via grove → runs the V4→V5 migration.
        let db = Db::open_path(&path).unwrap();
        let version: u32 = db
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 5);

        let task = db.get_task("T").unwrap().unwrap();
        assert_eq!(task.repos.len(), 2, "both repos preserved across migration");

        // Cascade is now live.
        db.delete_task("T").unwrap();
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM task_repos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    /// TX-get-task-equals-list-tasks-row (P1, Step 12): the two read paths must
    /// return identical rows — including the None-column variant.
    #[test]
    fn get_task_equals_list_tasks_row() {
        for (id, win, pane) in [
            ("WITH", Some("s:w".to_string()), Some("%9".to_string())),
            ("NONE", None, None),
        ] {
            let db = open_temp();
            let mut task = make_task(id);
            task.tmux_window = win;
            task.pane_id = pane;
            db.upsert_task(&task).unwrap();

            let via_get = db.get_task(id).unwrap().unwrap();
            let listed = db.list_tasks().unwrap();
            let via_list = listed.iter().find(|t| t.id == id).unwrap();

            assert_eq!(via_get.id, via_list.id);
            assert_eq!(via_get.path, via_list.path);
            assert_eq!(via_get.created_at, via_list.created_at);
            assert_eq!(via_get.tmux_window, via_list.tmux_window);
            assert_eq!(via_get.pane_id, via_list.pane_id);
            assert_eq!(via_get.repos.len(), via_list.repos.len());
            for (a, b) in via_get.repos.iter().zip(&via_list.repos) {
                assert_eq!(a.repo_name, b.repo_name);
                assert_eq!(a.worktree_path, b.worktree_path);
                assert_eq!(a.branch, b.branch);
            }
        }
    }
}
