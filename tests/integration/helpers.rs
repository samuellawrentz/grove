use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// A test fixture that sets up isolated repos_dir, tasks_dir, and grove_dir
/// in a temporary directory. Creates fixture bare repos with initial commits.
pub struct TestFixture {
    pub tmp_dir: TempDir,
    pub repos_dir: PathBuf,
    pub tasks_dir: PathBuf,
    #[allow(dead_code)]
    pub grove_dir: PathBuf,
    pub config_path: PathBuf,
    pub db_path: PathBuf,
}

impl TestFixture {
    /// Create a new test fixture with isolated directories.
    pub fn new() -> Self {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let repos_dir = tmp_dir.path().join("repos");
        let tasks_dir = tmp_dir.path().join("tasks");
        let grove_dir = tmp_dir.path().join(".grove");

        std::fs::create_dir_all(&repos_dir).unwrap();
        std::fs::create_dir_all(&tasks_dir).unwrap();
        std::fs::create_dir_all(&grove_dir).unwrap();

        let config_path = grove_dir.join("config.json");
        let db_path = grove_dir.join("grove.db");

        // Write config pointing to our temp dirs
        let config = serde_json::json!({
            "repos_dir": repos_dir,
            "tasks_dir": tasks_dir,
            "auto_launch_claude": false,
        });
        std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        Self {
            tmp_dir,
            repos_dir,
            tasks_dir,
            grove_dir,
            config_path,
            db_path,
        }
    }

    /// Create a fixture bare repo with an initial commit, returning its path.
    pub fn create_bare_repo(&self, name: &str) -> PathBuf {
        let work_dir = self.tmp_dir.path().join(format!("_work_{name}"));
        std::fs::create_dir_all(&work_dir).unwrap();

        // Init a regular repo, make a commit, then clone --bare
        run_cmd("git", &["init", "-b", "main"], Some(&work_dir));
        run_cmd(
            "git",
            &["config", "user.email", "test@test.com"],
            Some(&work_dir),
        );
        run_cmd("git", &["config", "user.name", "Test"], Some(&work_dir));

        let readme = work_dir.join("README.md");
        std::fs::write(&readme, format!("# {name}\n")).unwrap();
        run_cmd("git", &["add", "."], Some(&work_dir));
        run_cmd("git", &["commit", "-m", "initial commit"], Some(&work_dir));

        // Clone as bare
        let bare_path = self.tmp_dir.path().join(format!("{name}.git"));
        run_cmd(
            "git",
            &[
                "clone",
                "--bare",
                work_dir.to_str().unwrap(),
                bare_path.to_str().unwrap(),
            ],
            None,
        );

        bare_path
    }

    /// Get the grove binary command with environment set to use our temp dirs.
    pub fn grove_cmd(&self) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("grove").expect("failed to find grove binary");
        cmd.env("GROVE_CONFIG", &self.config_path);
        cmd.env("GROVE_REPOS_DIR", &self.repos_dir);
        cmd.env("GROVE_TASKS_DIR", &self.tasks_dir);
        cmd.env("HOME", self.tmp_dir.path());
        cmd
    }
}

fn run_cmd(program: &str, args: &[&str], cwd: Option<&Path>) {
    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd.env("LC_ALL", "C");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd.output().expect("failed to run command");
    assert!(
        output.status.success(),
        "{} {} failed: {}",
        program,
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}
