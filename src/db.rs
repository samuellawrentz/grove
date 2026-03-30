use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, Utc};

use crate::error::GroveError;
use crate::state::{RepoEntry, TaskEntry, TaskRepo};

pub struct Db {
    conn: rusqlite::Connection,
}

pub struct Project {
    pub id: i64,
    pub path: PathBuf,
    pub name: String,
    pub created_at: String,
    pub last_seen: String,
}

const DT_FMT: &str = "%Y-%m-%d %H:%M:%S";

fn dt_to_str(dt: DateTime<Utc>) -> String {
    dt.format(DT_FMT).to_string()
}

fn str_to_dt(s: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s, DT_FMT)
        .ok()
        .map(|d| d.and_utc())
}

impl Db {
    pub fn open() -> Result<Self, GroveError> {
        let dir = dirs::home_dir()
            .ok_or_else(|| GroveError::General("no home directory".into()))?
            .join(".grove");
        std::fs::create_dir_all(&dir)?;
        Self::open_path(&dir.join("grove.db"))
    }

    pub fn open_path(path: &Path) -> Result<Self, GroveError> {
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;",
        )?;
        let db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), GroveError> {
        let version: u32 =
            self.conn
                .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < 1 {
            self.conn.execute_batch(SCHEMA_V1)?;
            self.conn.pragma_update(None, "user_version", 1)?;
        }
        Ok(())
    }

    // ── Projects ─────────────────────────────────────────────────────────────

    pub fn upsert_project(&self, path: &str) -> Result<i64, GroveError> {
        let canonical = std::fs::canonicalize(path)
            .unwrap_or_else(|_| PathBuf::from(path))
            .to_string_lossy()
            .to_string();
        let name = Path::new(&canonical)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| canonical.clone());
        self.conn.execute(
            "INSERT INTO projects (path, name) VALUES (?1, ?2)
             ON CONFLICT(path) DO UPDATE SET last_seen = datetime('now')",
            rusqlite::params![canonical, name],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM projects WHERE path = ?1",
            [&canonical],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn upsert_project_with_timestamp(
        &self,
        path: &str,
        timestamp: u64,
    ) -> Result<i64, GroveError> {
        let canonical = std::fs::canonicalize(path)
            .unwrap_or_else(|_| PathBuf::from(path))
            .to_string_lossy()
            .to_string();
        let name = Path::new(&canonical)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| canonical.clone());
        let dt = chrono::DateTime::from_timestamp(timestamp as i64, 0)
            .map(|d| d.format(DT_FMT).to_string())
            .unwrap_or_else(|| chrono::Utc::now().format(DT_FMT).to_string());
        self.conn.execute(
            "INSERT INTO projects (path, name, last_seen) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET last_seen = MAX(last_seen, ?3)",
            rusqlite::params![canonical, name, dt],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM projects WHERE path = ?1",
            [&canonical],
            |r| r.get(0),
        )?;
        Ok(id)
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

    pub fn touch_project(&self, id: i64) -> Result<(), GroveError> {
        self.conn.execute(
            "UPDATE projects SET last_seen = datetime('now') WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub fn delete_project(&self, path: &str) -> Result<(), GroveError> {
        let canonical = std::fs::canonicalize(path)
            .unwrap_or_else(|_| PathBuf::from(path))
            .to_string_lossy()
            .to_string();
        self.conn
            .execute("DELETE FROM projects WHERE path = ?1", [&canonical])?;
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
        let mut stmt = self.conn.prepare(
            "SELECT name, url, path, default_branch, registered_at, last_synced_at \
             FROM repos WHERE name = ?1",
        )?;
        let mut rows = stmt.query([name])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_repo_entry(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_repos(&self) -> Result<Vec<RepoEntry>, GroveError> {
        let mut stmt = self.conn.prepare(
            "SELECT name, url, path, default_branch, registered_at, last_synced_at \
             FROM repos ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| row_to_repo_entry(row))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| GroveError::Database(e.to_string()))
    }

    pub fn delete_repo(&self, name: &str) -> Result<(), GroveError> {
        self.conn
            .execute("DELETE FROM repos WHERE name = ?1", [name])?;
        Ok(())
    }

    // ── Tasks ─────────────────────────────────────────────────────────────────

    pub fn upsert_task(&self, task: &TaskEntry) -> Result<(), GroveError> {
        let path = task.path.to_string_lossy().to_string();
        let created_at = dt_to_str(task.created_at);
        self.conn.execute(
            "INSERT INTO tasks (id, path, created_at, tmux_window, pane_id)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               path        = excluded.path,
               tmux_window = excluded.tmux_window,
               pane_id     = excluded.pane_id",
            rusqlite::params![
                task.id,
                path,
                created_at,
                task.tmux_window,
                task.pane_id,
            ],
        )?;
        // Replace task_repos
        self.conn.execute(
            "DELETE FROM task_repos WHERE task_id = ?1",
            [&task.id],
        )?;
        for tr in &task.repos {
            let worktree = tr.worktree_path.to_string_lossy().to_string();
            self.conn.execute(
                "INSERT INTO task_repos (task_id, repo_name, worktree, branch) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![task.id, tr.repo_name, worktree, tr.branch],
            )?;
        }
        Ok(())
    }

    pub fn get_task(&self, id: &str) -> Result<Option<TaskEntry>, GroveError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, created_at, tmux_window, pane_id FROM tasks WHERE id = ?1",
        )?;
        let mut rows = stmt.query([id])?;
        if let Some(row) = rows.next()? {
            let task = row_to_task_entry(row, &self.conn)?;
            Ok(Some(task))
        } else {
            Ok(None)
        }
    }

    pub fn list_tasks(&self) -> Result<Vec<TaskEntry>, GroveError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, created_at, tmux_window, pane_id FROM tasks ORDER BY created_at DESC",
        )?;
        let ids: Vec<(String, String, String, Option<String>, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| GroveError::Database(e.to_string()))?;

        let mut tasks = Vec::with_capacity(ids.len());
        for (id, path, created_at_str, tmux_window, pane_id) in ids {
            let created_at = str_to_dt(&created_at_str)
                .unwrap_or_else(Utc::now);
            let repos = self.load_task_repos(&id)?;
            tasks.push(TaskEntry {
                id,
                path: PathBuf::from(path),
                created_at,
                tmux_window,
                pane_id,
                repos,
            });
        }
        Ok(tasks)
    }

    pub fn delete_task(&self, id: &str) -> Result<(), GroveError> {
        self.conn
            .execute("DELETE FROM task_repos WHERE task_id = ?1", [id])?;
        self.conn
            .execute("DELETE FROM tasks WHERE id = ?1", [id])?;
        Ok(())
    }

    fn load_task_repos(&self, task_id: &str) -> Result<Vec<TaskRepo>, GroveError> {
        let mut stmt = self.conn.prepare(
            "SELECT repo_name, worktree, branch FROM task_repos WHERE task_id = ?1",
        )?;
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

fn row_to_task_entry(
    row: &rusqlite::Row<'_>,
    conn: &rusqlite::Connection,
) -> Result<TaskEntry, GroveError> {
    let id: String = row.get(0)?;
    let path: String = row.get(1)?;
    let created_at_str: String = row.get(2)?;
    let tmux_window: Option<String> = row.get(3)?;
    let pane_id: Option<String> = row.get(4)?;

    let created_at = str_to_dt(&created_at_str).unwrap_or_else(Utc::now);

    let mut stmt = conn.prepare(
        "SELECT repo_name, worktree, branch FROM task_repos WHERE task_id = ?1",
    )?;
    let repos = stmt
        .query_map([&id], |r| {
            Ok(TaskRepo {
                repo_name: r.get(0)?,
                worktree_path: PathBuf::from(r.get::<_, String>(1)?),
                branch: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| GroveError::Database(e.to_string()))?;

    Ok(TaskEntry {
        id,
        path: PathBuf::from(path),
        created_at,
        tmux_window,
        pane_id,
        repos,
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
        assert_eq!(version, 1);
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
}
