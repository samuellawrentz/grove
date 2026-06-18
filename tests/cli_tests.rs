mod integration;

use integration::helpers::TestFixture;
use predicates::prelude::*;

// ============================================================================
// Full Workflow Test
// ============================================================================

#[test]
fn full_workflow_register_sync_init_list_close() {
    let fix = TestFixture::new();
    let bare_a = fix.create_bare_repo("repo-a");
    let bare_b = fix.create_bare_repo("repo-b");

    // Register 2 repos
    fix.grove_cmd()
        .args(["register", "repo-a", bare_a.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["register", "repo-b", bare_b.to_str().unwrap()])
        .assert()
        .success();

    // Sync all
    fix.grove_cmd().args(["sync"]).assert().success();

    // Init task with both repos
    fix.grove_cmd()
        .args(["init", "TASK-1", "repo-a", "repo-b"])
        .assert()
        .success();

    // List shows the task
    fix.grove_cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-1"));

    // Close the task
    fix.grove_cmd().args(["close", "TASK-1"]).assert().success();

    // List is now empty
    fix.grove_cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No active tasks"));
}

// ============================================================================
// Register Tests
// ============================================================================

#[test]
fn register_bare_repo_and_verify_state() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Registered 'myrepo'"));

    // Verify db contains the repo
    let conn = rusqlite::Connection::open(&fix.db_path).unwrap();
    let name: String = conn
        .query_row("SELECT name FROM repos WHERE name = 'myrepo'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(name, "myrepo");
}

#[test]
fn register_idempotent_same_url() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");
    let url = bare.to_str().unwrap();

    fix.grove_cmd()
        .args(["register", "myrepo", url])
        .assert()
        .success();

    // Re-register same URL = exit 0
    fix.grove_cmd()
        .args(["register", "myrepo", url])
        .assert()
        .success()
        .stdout(predicate::str::contains("already registered"));
}

#[test]
fn register_conflict_different_url() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    // Re-register different URL = exit 6
    fix.grove_cmd()
        .args(["register", "myrepo", "/some/other/url"])
        .assert()
        .code(6);
}

// ============================================================================
// Sync Tests
// ============================================================================

#[test]
fn sync_registered_repos() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok"));
}

#[test]
fn sync_nonexistent_repo_exit_3() {
    let fix = TestFixture::new();

    fix.grove_cmd()
        .args(["sync", "nonexistent"])
        .assert()
        .code(3);
}

// ============================================================================
// Init Tests
// ============================================================================

#[test]
fn init_creates_worktrees_and_context() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    // Verify worktree directory exists
    let task_dir = fix.tasks_dir.join("TASK-1");
    assert!(task_dir.exists(), "task dir should exist");
    assert!(
        task_dir.join("myrepo").exists(),
        "worktree dir should exist"
    );
    assert!(
        task_dir.join("CONTEXT.md").exists(),
        "CONTEXT.md should exist"
    );

    // Verify CONTEXT.md has default template content
    let ctx = std::fs::read_to_string(task_dir.join("CONTEXT.md")).unwrap();
    assert!(ctx.contains("TASK-1"));
    assert!(ctx.contains("myrepo"));
}

#[test]
fn init_idempotent_same_repos() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    // Init again same repos = exit 0
    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already exists"));
}

#[test]
fn init_reuses_preexisting_branch() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    // Simulate a branch that already exists in the bare clone (e.g. fetched
    // from origin) before the task is created.
    std::process::Command::new("git")
        .args(["branch", "feature-x", "HEAD"])
        .current_dir(&bare)
        .output()
        .expect("failed to create branch");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    // init with --branch matching the existing branch must check it out,
    // not fail with "a branch named 'feature-x' already exists".
    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo", "--branch", "feature-x"])
        .assert()
        .success();

    // Worktree is on the reused branch.
    let wt = fix.tasks_dir.join("TASK-1").join("myrepo");
    let head = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(&wt)
        .output()
        .expect("failed to read branch");
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "feature-x");
}

#[test]
fn init_conflict_different_repos() {
    let fix = TestFixture::new();
    let bare_a = fix.create_bare_repo("repo-a");
    let bare_b = fix.create_bare_repo("repo-b");

    fix.grove_cmd()
        .args(["register", "repo-a", bare_a.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["register", "repo-b", bare_b.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "repo-a"])
        .assert()
        .success();

    // Init with different repos = exit 6
    fix.grove_cmd()
        .args(["init", "TASK-1", "repo-b"])
        .assert()
        .code(6);
}

/// Install a write-only fault: a BEFORE INSERT trigger that aborts. Reads still
/// pass, so the command reaches its terminal `Db::transaction` and fails there —
/// a genuine deterministic DB error, no production test hook.
fn inject_insert_fault(db_path: &std::path::Path, table: &str) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute_batch(&format!(
        "CREATE TRIGGER grove_fault BEFORE INSERT ON {table} \
         BEGIN SELECT RAISE(ABORT, 'injected'); END;"
    ))
    .unwrap();
}

fn remove_insert_fault(db_path: &std::path::Path) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute_batch("DROP TRIGGER grove_fault;").unwrap();
}

/// INIT-rollback-on-terminal-tx-failure (P0 / B1): when the terminal tx fails
/// after the worktree + CONTEXT.md exist, the DB rolls back AND the journal
/// unwinds task dir + worktree + branch; a clean re-init then succeeds.
#[test]
fn init_rollback_on_terminal_tx_failure() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("repo-a");
    fix.grove_cmd()
        .args(["register", "repo-a", bare.to_str().unwrap()])
        .assert()
        .success();

    inject_insert_fault(&fix.db_path, "task_repos");
    fix.grove_cmd()
        .args(["init", "TASK-1", "repo-a"])
        .assert()
        .failure();

    let task_dir = fix.tasks_dir.join("TASK-1");
    assert!(!task_dir.exists(), "journal must remove the task dir");
    let grove_bare = fix.repos_dir.join("repo-a.git");
    assert!(
        !branch_exists(&grove_bare, "TASK-1"),
        "journal must delete the freshly-created branch"
    );
    let conn = rusqlite::Connection::open(&fix.db_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks WHERE id = 'TASK-1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 0, "terminal tx must roll back the tasks row");

    remove_insert_fault(&fix.db_path);
    fix.grove_cmd()
        .args(["init", "TASK-1", "repo-a"])
        .assert()
        .success();
    assert!(
        task_dir.exists(),
        "clean re-init succeeds after the fault clears"
    );
}

/// ADD-rollback-worktree-on-tx-failure (P0 / B2): a failed terminal tx leaves
/// the new repo's worktree+branch gone and task_repos reflecting only the
/// original repo.
#[test]
fn add_rollback_worktree_on_tx_failure() {
    let fix = TestFixture::new();
    let bare_a = fix.create_bare_repo("repo-a");
    let bare_b = fix.create_bare_repo("repo-b");
    fix.grove_cmd()
        .args(["register", "repo-a", bare_a.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["register", "repo-b", bare_b.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["init", "TASK-1", "repo-a"])
        .assert()
        .success();

    inject_insert_fault(&fix.db_path, "task_repos");
    fix.grove_cmd()
        .args(["add", "TASK-1", "repo-b"])
        .assert()
        .failure();

    let wt_b = fix.tasks_dir.join("TASK-1").join("repo-b");
    assert!(!wt_b.exists(), "journal must remove the repo-b worktree");
    let grove_bare_b = fix.repos_dir.join("repo-b.git");
    assert!(
        !branch_exists(&grove_bare_b, "TASK-1"),
        "journal must delete the repo-b branch"
    );
    let conn = rusqlite::Connection::open(&fix.db_path).unwrap();
    let repos: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT repo_name FROM task_repos WHERE task_id = 'TASK-1'")
            .unwrap();
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    assert_eq!(
        repos,
        vec!["repo-a".to_string()],
        "tx rollback keeps repo-a only"
    );
}

/// REGISTER-rollback-bare-clone-on-tx-failure (P0 / N3): a failed terminal tx
/// removes the orphan bare clone so retry is not poisoned.
#[test]
fn register_rollback_bare_clone_on_tx_failure() {
    let fix = TestFixture::new();
    // Materialize the DB + schema so a trigger can be installed pre-register.
    fix.grove_cmd().args(["repos"]).assert().success();
    inject_insert_fault(&fix.db_path, "repos");

    let src = fix.create_bare_repo("src");
    let bare_path = fix.repos_dir.join("myrepo.git");
    fix.grove_cmd()
        .args(["register", "myrepo", src.to_str().unwrap()])
        .assert()
        .failure();
    assert!(
        !bare_path.exists(),
        "journal must remove the orphan bare clone"
    );

    remove_insert_fault(&fix.db_path);
    fix.grove_cmd()
        .args(["register", "myrepo", src.to_str().unwrap()])
        .assert()
        .success();
    assert!(bare_path.exists(), "clean re-register succeeds");
}

/// INIT-stale-recovery-preserves-unmerged (P0 / D6): stale re-init must use a
/// safe `-d`, preserving unmerged commits (warn) instead of `-D` destroying them.
#[test]
fn init_stale_recovery_preserves_unmerged() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");
    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    // Make an unmerged commit on the task branch inside its worktree.
    let wt = fix.tasks_dir.join("TASK-1").join("myrepo");
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&wt)
            .env("LC_ALL", "C")
            .output()
            .expect("git failed")
    };
    git(&["config", "user.email", "t@t.com"]);
    git(&["config", "user.name", "T"]);
    std::fs::write(wt.join("extra.txt"), "unmerged work").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "unmerged-commit"]);

    // Make the task stale by removing its directory (branch ref survives).
    std::fs::remove_dir_all(fix.tasks_dir.join("TASK-1")).unwrap();

    let out = fix
        .grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .output()
        .unwrap();
    assert!(out.status.success(), "re-init should still succeed");

    // The unmerged commit must survive: the rebuilt worktree reuses the branch.
    let wt2 = fix.tasks_dir.join("TASK-1").join("myrepo");
    let log = std::process::Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(&wt2)
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&log.stdout).contains("unmerged-commit"),
        "unmerged commit must be preserved on stale re-init"
    );
}

/// CLOSE-idempotent-on-missing-path (P0 / N4): close clears the DB row even when
/// the task path is already gone.
#[test]
fn close_idempotent_on_missing_path() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");
    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    std::fs::remove_dir_all(fix.tasks_dir.join("TASK-1")).unwrap();

    fix.grove_cmd().args(["close", "TASK-1"]).assert().success();

    let conn = rusqlite::Connection::open(&fix.db_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks WHERE id = 'TASK-1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        count, 0,
        "close must clear the row even with a missing path"
    );
}

// ============================================================================
// Sync partial-failure (Step 4 / N1)
// ============================================================================

/// Register one good repo, then inject a bad repo row whose bare path does not
/// exist so its fetch fails. Used by both partial-failure sync tests.
fn fixture_with_one_good_one_bad(fix: &TestFixture) {
    let bare_a = fix.create_bare_repo("repo-a");
    fix.grove_cmd()
        .args(["register", "repo-a", bare_a.to_str().unwrap()])
        .assert()
        .success();
    let conn = rusqlite::Connection::open(&fix.db_path).unwrap();
    conn.execute(
        "INSERT INTO repos (name, url, path, default_branch) \
         VALUES ('repo-bad', '/nonexistent/url', '/nonexistent/bare/path.git', 'main')",
        [],
    )
    .unwrap();
}

/// SYNC-json-ok-false-on-partial (P0 / N1): the JSON envelope must report
/// `ok:false` and still list both repos when one fails.
#[test]
fn sync_json_ok_false_on_partial() {
    let fix = TestFixture::new();
    fixture_with_one_good_one_bad(&fix);

    let output = fix.grove_cmd().args(["--json", "sync"]).output().unwrap();

    // Partial failure: nonzero exit and (because the command also returns Err)
    // a trailing error doc, so parse only the first JSON value off the stream.
    assert!(!output.status.success(), "partial sync must exit nonzero");
    let json: serde_json::Value = serde_json::Deserializer::from_slice(&output.stdout)
        .into_iter::<serde_json::Value>()
        .next()
        .expect("expected a results envelope on stdout")
        .expect("results envelope must be valid JSON");

    assert_eq!(json["ok"], false, "envelope ok must be false on partial");
    let results = json["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2, "both repos reported");
    let bad = results
        .iter()
        .find(|r| r["repo"] == "repo-bad")
        .expect("repo-bad present");
    assert_eq!(bad["ok"], false);
    assert!(bad["error"].is_string(), "failed repo carries an error");
    let good = results
        .iter()
        .find(|r| r["repo"] == "repo-a")
        .expect("repo-a present");
    assert_eq!(good["ok"], true);
    assert!(good.get("error").is_none(), "ok repo omits error field");
}

/// SYNC-partial-failure-exit-nonzero (P0 / N1): human-mode sync over a failing
/// repo must exit nonzero while still reporting the good repo.
#[test]
fn sync_partial_failure_exit_nonzero() {
    let fix = TestFixture::new();
    fixture_with_one_good_one_bad(&fix);

    fix.grove_cmd()
        .args(["sync"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("repo-a ok"))
        .stdout(predicate::str::contains("repo-bad FAILED"));
}

#[test]
fn init_stale_state_recreates() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    // Delete the task directory to make it stale
    let task_dir = fix.tasks_dir.join("TASK-1");
    std::fs::remove_dir_all(&task_dir).unwrap();

    // Re-init should detect stale state and recreate
    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    assert!(task_dir.exists(), "task dir should be recreated");
    assert!(
        task_dir.join("myrepo").exists(),
        "worktree should be recreated"
    );
}

#[test]
fn init_nonexistent_repo_exit_3() {
    let fix = TestFixture::new();

    fix.grove_cmd()
        .args(["init", "TASK-1", "nonexistent"])
        .assert()
        .code(3);
}

#[test]
fn init_partial_failure_rollback() {
    let fix = TestFixture::new();
    let bare_a = fix.create_bare_repo("repo-a");

    fix.grove_cmd()
        .args(["register", "repo-a", bare_a.to_str().unwrap()])
        .assert()
        .success();

    // Manually register a fake repo with a nonexistent bare path in db
    // to trigger worktree creation failure on the second repo
    {
        let conn = rusqlite::Connection::open(&fix.db_path).unwrap();
        conn.execute(
            "INSERT INTO repos (name, url, path, default_branch) VALUES ('bad-repo', '/nonexistent/path', '/nonexistent/bare/path.git', 'main')",
            [],
        ).unwrap();
    }

    // Init with repo-a (good) + bad-repo (will fail)
    fix.grove_cmd()
        .args(["init", "TASK-1", "repo-a", "bad-repo"])
        .assert()
        .failure();

    // Verify rollback: task directory should not exist
    let task_dir = fix.tasks_dir.join("TASK-1");
    assert!(
        !task_dir.exists(),
        "task dir should be cleaned up after partial failure"
    );

    // DB should not contain the task
    let conn = rusqlite::Connection::open(&fix.db_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks WHERE id = 'TASK-1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 0, "db should not contain partially created task");
}

// ============================================================================
// Close Tests
// ============================================================================

#[test]
fn close_existing_task() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    let task_dir = fix.tasks_dir.join("TASK-1");
    assert!(task_dir.exists());

    fix.grove_cmd().args(["close", "TASK-1"]).assert().success();

    assert!(!task_dir.exists(), "task dir should be removed after close");

    // DB should not contain the task
    let conn = rusqlite::Connection::open(&fix.db_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks WHERE id = 'TASK-1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 0, "task should be removed from db after close");
}

/// Returns true if `branch` exists in the bare repo at `bare`.
fn branch_exists(bare: &std::path::Path, branch: &str) -> bool {
    let out = std::process::Command::new("git")
        .args(["-C", bare.to_str().unwrap(), "branch", "--list", branch])
        .output()
        .expect("git branch --list failed");
    !String::from_utf8_lossy(&out.stdout).trim().is_empty()
}

#[test]
fn close_deletes_merged_branch_by_default() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    let grove_bare = fix.repos_dir.join("myrepo.git");
    assert!(
        branch_exists(&grove_bare, "TASK-1"),
        "branch should exist after init"
    );

    // A fresh task branch points at main's commit => merged => safe-deleted by default.
    fix.grove_cmd().args(["close", "TASK-1"]).assert().success();

    assert!(
        !branch_exists(&grove_bare, "TASK-1"),
        "merged branch should be deleted by default close"
    );
}

#[test]
fn close_keeps_unmerged_branch_without_force() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    // Advance the task branch beyond main so it is unmerged.
    let worktree = fix.tasks_dir.join("TASK-1").join("myrepo");
    std::fs::write(worktree.join("new.txt"), "work").unwrap();
    let commit = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&worktree)
            .args(args)
            .output()
            .expect("git failed");
    };
    commit(&["add", "."]);
    commit(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@test.com",
        "commit",
        "-m",
        "unmerged work",
    ]);

    // Default close keeps the unmerged branch (warns) but still succeeds.
    fix.grove_cmd().args(["close", "TASK-1"]).assert().success();
    assert!(
        branch_exists(&fix.repos_dir.join("myrepo.git"), "TASK-1"),
        "unmerged branch should be preserved by default close"
    );
}

#[test]
fn close_force_deletes_unmerged_branch() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    let worktree = fix.tasks_dir.join("TASK-1").join("myrepo");
    std::fs::write(worktree.join("new.txt"), "work").unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&worktree)
            .args(args)
            .output()
            .expect("git failed");
    };
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@test.com",
        "commit",
        "-m",
        "unmerged work",
    ]);

    // -D force-deletes the unmerged branch.
    fix.grove_cmd()
        .args(["close", "-D", "TASK-1"])
        .assert()
        .success();
    assert!(
        !branch_exists(&fix.repos_dir.join("myrepo.git"), "TASK-1"),
        "unmerged branch should be force-deleted with -D"
    );
}

#[test]
fn close_nonexistent_task_exit_2() {
    let fix = TestFixture::new();

    fix.grove_cmd()
        .args(["close", "nonexistent"])
        .assert()
        .code(2);
}

#[test]
fn close_uncommitted_changes_exit_5() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    // Create an uncommitted file in the worktree
    let worktree = fix.tasks_dir.join("TASK-1").join("myrepo");
    std::fs::write(worktree.join("dirty.txt"), "uncommitted change").unwrap();

    fix.grove_cmd().args(["close", "TASK-1"]).assert().code(5);

    // With --force, should succeed
    fix.grove_cmd()
        .args(["close", "--force", "TASK-1"])
        .assert()
        .success();
}

#[test]
fn close_missing_bare_repo_warns_but_continues() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    // Delete the bare repo directory
    std::fs::remove_dir_all(&bare).unwrap();

    // Close should warn but succeed (with --force to skip uncommitted check issues)
    fix.grove_cmd()
        .args(["close", "--force", "TASK-1"])
        .assert()
        .success();

    let task_dir = fix.tasks_dir.join("TASK-1");
    assert!(
        !task_dir.exists(),
        "task dir should be removed even with missing bare repo"
    );
}

// ============================================================================
// List Tests
// ============================================================================

#[test]
fn list_shows_active_tasks() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-1"))
        .stdout(predicate::str::contains("myrepo"));
}

#[test]
fn list_empty_state() {
    let fix = TestFixture::new();

    fix.grove_cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No active tasks"));
}

#[test]
fn list_stale_task_flagged() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    // Delete task directory to make it stale
    std::fs::remove_dir_all(fix.tasks_dir.join("TASK-1")).unwrap();

    fix.grove_cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("STALE"));
}

// ============================================================================
// JSON Output Tests
// ============================================================================

#[test]
fn json_register() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    let output = fix
        .grove_cmd()
        .args(["--json", "register", "myrepo", bare.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["name"], "myrepo");
}

#[test]
fn json_repos() {
    let fix = TestFixture::new();

    let output = fix.grove_cmd().args(["--json", "repos"]).output().unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert!(json["repos"].is_array());
}

#[test]
fn json_sync() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    let output = fix.grove_cmd().args(["--json", "sync"]).output().unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert!(json["results"].is_array());
}

#[test]
fn json_init() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    let output = fix
        .grove_cmd()
        .args(["--json", "init", "TASK-1", "myrepo"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["task_id"], "TASK-1");
}

#[test]
fn json_list() {
    let fix = TestFixture::new();

    let output = fix.grove_cmd().args(["--json", "list"]).output().unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert!(json["tasks"].is_array());
}

#[test]
fn json_close() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    let output = fix
        .grove_cmd()
        .args(["--json", "close", "TASK-1"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["task_id"], "TASK-1");
}

// ============================================================================
// JSON Error Output Tests
// ============================================================================

#[test]
fn json_error_register_conflict() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["--json", "register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    let output = fix
        .grove_cmd()
        .args(["--json", "register", "myrepo", "/other/url"])
        .output()
        .unwrap();

    assert_eq!(output.status.code().unwrap(), 6);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["exit_code"], 6);
}

#[test]
fn json_error_task_not_found() {
    let fix = TestFixture::new();

    let output = fix
        .grove_cmd()
        .args(["--json", "close", "nonexistent"])
        .output()
        .unwrap();

    assert_eq!(output.status.code().unwrap(), 2);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["exit_code"], 2);
}

#[test]
fn json_error_repo_not_registered() {
    let fix = TestFixture::new();

    let output = fix
        .grove_cmd()
        .args(["--json", "init", "TASK-1", "nonexistent"])
        .output()
        .unwrap();

    assert_eq!(output.status.code().unwrap(), 3);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["exit_code"], 3);
}

#[test]
fn json_error_uncommitted_changes() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    // Create uncommitted changes
    let worktree = fix.tasks_dir.join("TASK-1").join("myrepo");
    std::fs::write(worktree.join("dirty.txt"), "dirty").unwrap();

    let output = fix
        .grove_cmd()
        .args(["--json", "close", "TASK-1"])
        .output()
        .unwrap();

    assert_eq!(output.status.code().unwrap(), 5);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["exit_code"], 5);
}

// ============================================================================
// Exit Code Tests
// ============================================================================

#[test]
fn exit_code_2_task_not_found() {
    let fix = TestFixture::new();
    fix.grove_cmd().args(["close", "nope"]).assert().code(2);
}

#[test]
fn exit_code_3_repo_not_registered() {
    let fix = TestFixture::new();
    fix.grove_cmd().args(["sync", "nope"]).assert().code(3);
}

#[test]
fn exit_code_5_uncommitted_changes() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo"])
        .assert()
        .success();

    let worktree = fix.tasks_dir.join("TASK-1").join("myrepo");
    std::fs::write(worktree.join("new.txt"), "data").unwrap();

    fix.grove_cmd().args(["close", "TASK-1"]).assert().code(5);
}

#[test]
fn exit_code_6_conflict() {
    let fix = TestFixture::new();
    let bare_a = fix.create_bare_repo("repo-a");
    let bare_b = fix.create_bare_repo("repo-b");

    fix.grove_cmd()
        .args(["register", "repo-a", bare_a.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["register", "repo-b", bare_b.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "repo-a"])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "repo-b"])
        .assert()
        .code(6);
}

// ============================================================================
// Verbose Flag Tests
// ============================================================================

#[test]
fn verbose_prints_git_commands() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    let output = fix
        .grove_cmd()
        .args(["--verbose", "register", "myrepo", bare.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("git") && stderr.contains("clone"),
        "verbose should print git clone command, got stderr: {stderr}"
    );
}

#[test]
fn verbose_sync_prints_fetch_command() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    let output = fix
        .grove_cmd()
        .args(["--verbose", "sync", "myrepo"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("git") && stderr.contains("fetch"),
        "verbose should print git fetch command, got stderr: {stderr}"
    );
}

// ============================================================================
// Repo Name Validation Tests (dots, underscores)
// ============================================================================

#[test]
fn register_name_with_dots() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("my.repo");

    fix.grove_cmd()
        .args(["register", "my.repo", bare.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Registered 'my.repo'"));
}

#[test]
fn register_name_with_underscores() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("my_repo");

    fix.grove_cmd()
        .args(["register", "my_repo", bare.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Registered 'my_repo'"));
}

#[test]
fn register_name_with_invalid_chars_fails() {
    let fix = TestFixture::new();

    fix.grove_cmd()
        .args(["register", "my/repo", "https://example.com"])
        .assert()
        .code(1);

    fix.grove_cmd()
        .args(["register", "my repo", "https://example.com"])
        .assert()
        .code(1);
}

// ============================================================================
// Init with --context flag
// ============================================================================

#[test]
fn init_with_custom_context() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args([
            "init",
            "TASK-1",
            "myrepo",
            "--context",
            "Fix the login bug in auth module",
        ])
        .assert()
        .success();

    let ctx = std::fs::read_to_string(fix.tasks_dir.join("TASK-1").join("CONTEXT.md")).unwrap();
    assert!(
        ctx.contains("Fix the login bug in auth module"),
        "CONTEXT.md should contain custom context text"
    );
}

// ============================================================================
// Init with --branch flag
// ============================================================================

#[test]
fn init_with_custom_branch() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    fix.grove_cmd()
        .args(["init", "TASK-1", "myrepo", "--branch", "feature-login"])
        .assert()
        .success()
        .stdout(predicate::str::contains("branch: feature-login"));

    // Verify the branch exists by checking the worktree is on the right branch
    let worktree = fix.tasks_dir.join("TASK-1").join("myrepo");
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&worktree)
        .output()
        .unwrap();
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(branch, "feature-login");

    fix.grove_cmd()
        .args(["close", "--force", "TASK-1"])
        .assert()
        .success();
}

#[test]
fn init_with_custom_branch_json() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");

    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();

    let output = fix
        .grove_cmd()
        .args([
            "--json",
            "init",
            "TASK-1",
            "myrepo",
            "--branch",
            "my-feature",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["branch"], "my-feature");
}

/// CLI-init-no-id-no-interactive-errors (P2 / A4): `grove init` with no task id
/// and no -i must error cleanly (not hang on a stdin prompt).
#[test]
fn init_no_id_no_interactive_errors() {
    let fix = TestFixture::new();
    let bare = fix.create_bare_repo("myrepo");
    fix.grove_cmd()
        .args(["register", "myrepo", bare.to_str().unwrap()])
        .assert()
        .success();
    fix.grove_cmd()
        .args(["init"]) // no task id, no -i, no repos
        .assert()
        .failure()
        .stderr(predicates::prelude::predicate::str::contains("required"));
}
