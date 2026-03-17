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

    // Verify state file exists and contains the repo
    let state_contents = std::fs::read_to_string(&fix.state_path).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state_contents).unwrap();
    assert!(state["repos"]["myrepo"].is_object());
    assert_eq!(state["repos"]["myrepo"]["name"], "myrepo");
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

    // Manually register a fake repo with a nonexistent bare path in state
    // to trigger worktree creation failure on the second repo
    let state_contents = std::fs::read_to_string(&fix.state_path).unwrap();
    let mut state: serde_json::Value = serde_json::from_str(&state_contents).unwrap();
    state["repos"]["bad-repo"] = serde_json::json!({
        "name": "bad-repo",
        "url": "/nonexistent/path",
        "path": "/nonexistent/bare/path.git",
        "default_branch": "main",
        "registered_at": "2026-01-01T00:00:00Z",
        "last_synced_at": null,
    });
    std::fs::write(
        &fix.state_path,
        serde_json::to_string_pretty(&state).unwrap(),
    )
    .unwrap();

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

    // State should not contain the task
    let state_after = std::fs::read_to_string(&fix.state_path).unwrap();
    let state_after: serde_json::Value = serde_json::from_str(&state_after).unwrap();
    assert!(
        state_after["tasks"]["TASK-1"].is_null(),
        "state should not contain partially created task"
    );
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

    // State should not contain the task
    let state_contents = std::fs::read_to_string(&fix.state_path).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state_contents).unwrap();
    assert!(state["tasks"]["TASK-1"].is_null());
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
