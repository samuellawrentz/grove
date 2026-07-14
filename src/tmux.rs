use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::error::GroveError;

/// Information about a single tmux pane.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct PaneInfo {
    pub pane_id: String,
    pub session_name: String,
    pub window_index: u32,
    pub window_name: String,
    pub current_path: PathBuf,
    pub current_command: String,
    pub start_command: String,
    pub pid: u32,
    pub activity: u64,
}

/// Run a tmux command, optionally logging the command line and exit code.
/// Returns stdout on success, or GroveError on failure.
pub fn run_tmux(args: &[&str], verbose: bool) -> Result<String, GroveError> {
    if verbose {
        eprintln!("[grove] tmux {}", args.join(" "));
    }

    let output = Command::new("tmux").args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GroveError::TmuxNotRunning("tmux binary not found".to_string())
        } else {
            GroveError::General(format!("failed to run tmux: {e}"))
        }
    })?;

    if verbose {
        eprintln!("[grove] exit code: {}", output.status.code().unwrap_or(-1));
    }

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(GroveError::TmuxNotRunning(format!(
            "tmux {} failed: {}",
            args.join(" "),
            stderr.trim()
        )))
    }
}

/// Check if the tmux binary is available.
pub fn is_tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Check if we are running inside a tmux session.
pub fn is_inside_tmux() -> bool {
    std::env::var("TMUX").is_ok_and(|v| !v.is_empty())
}

/// Get the name of the current tmux session. Only works inside tmux.
pub fn current_session(verbose: bool) -> Result<String, GroveError> {
    run_tmux(&["display-message", "-p", "#{session_name}"], verbose)
}

/// Process-global cache of the current session name. It never changes for a
/// running TUI, so resolve it once instead of querying tmux every refresh.
static SESSION_CACHE: OnceLock<Option<String>> = OnceLock::new();

/// Resolve the current session at most once per process, memoizing via the given
/// cell. Pure over the resolver so the once-only semantics are unit-testable.
pub(crate) fn current_session_once(
    cache: &OnceLock<Option<String>>,
    resolver: impl FnOnce() -> Option<String>,
) -> Option<String> {
    cache.get_or_init(resolver).clone()
}

/// Cached `current_session`: resolves via tmux only on first call.
pub fn current_session_cached(verbose: bool) -> Option<String> {
    current_session_once(&SESSION_CACHE, || current_session(verbose).ok())
}

/// Create a named window in a specific session with a working directory.
/// Returns the new window's pane id.
pub fn new_named_window(
    session: &str,
    window_name: &str,
    cwd: &Path,
    verbose: bool,
) -> Result<String, GroveError> {
    new_named_window_with(session, window_name, cwd, |args| run_tmux(args, verbose))
}

/// Window options that stop tmux from renaming a window out from under grove.
/// A program in the pane emitting a title escape would otherwise rewrite the
/// name, orphaning every `session:grove-<task>` target grove recorded.
const NAME_PINNING_OPTIONS: [(&str, &str); 2] =
    [("automatic-rename", "off"), ("allow-rename", "off")];

/// Pure over the tmux runner so the issued command sequence is unit-testable.
pub(crate) fn new_named_window_with<F>(
    session: &str,
    window_name: &str,
    cwd: &Path,
    mut run: F,
) -> Result<String, GroveError>
where
    F: FnMut(&[&str]) -> Result<String, GroveError>,
{
    let cwd_str = cwd
        .to_str()
        .ok_or_else(|| GroveError::General("invalid path for tmux window".to_string()))?;

    // Colon-qualify the session target. A bare `-t <session>` is ambiguous when the
    // session is named numerically (e.g. "0"): tmux parses it as a window-index target,
    // collides with an occupied index, and the window is never created. The trailing
    // colon forces a session target so tmux picks the next free, base-index-aware index.
    let target = format!("{session}:");

    // Ask for the ids up front: addressing the window by `@id` and the pane by
    // `%id` is immune to any later renaming or reindexing.
    let created = run(&[
        "new-window",
        "-t",
        &target,
        "-n",
        window_name,
        "-c",
        cwd_str,
        "-P",
        "-F",
        "#{window_id}\t#{pane_id}",
    ])?;

    let (window_id, pane_id) = created.trim().split_once('\t').ok_or_else(|| {
        GroveError::General(format!("tmux new-window returned no ids: {created:?}"))
    })?;

    for (option, value) in NAME_PINNING_OPTIONS {
        run(&["set-option", "-w", "-t", window_id, option, value])?;
    }

    Ok(pane_id.to_string())
}

/// Switch to a window within the current session.
pub fn select_window(target: &str, verbose: bool) -> Result<(), GroveError> {
    run_tmux(&["select-window", "-t", target], verbose)?;
    Ok(())
}

/// Check if a window exists in a session by listing window names.
pub fn window_exists(session: &str, window_name: &str, verbose: bool) -> bool {
    run_tmux(
        &["list-windows", "-t", session, "-F", "#{window_name}"],
        verbose,
    )
    .map(|output| output.lines().any(|line| line == window_name))
    .unwrap_or(false)
}

/// Send text to a tmux pane using literal mode, then press Enter.
/// Two separate tmux calls to avoid key interpretation issues.
///
/// The pause matters: an agent TUI receiving a large literal burst is still
/// redrawing its input box when the Enter arrives, and a submit that lands
/// mid-paste gets absorbed as a newline. The prompt then sits in the box,
/// unsent — which looks exactly like an agent that ignored you.
pub fn send_keys(target: &str, text: &str, verbose: bool) -> Result<(), GroveError> {
    run_tmux(&["send-keys", "-t", target, "-l", text], verbose)?;
    std::thread::sleep(std::time::Duration::from_millis(150));
    run_tmux(&["send-keys", "-t", target, "Enter"], verbose)?;
    Ok(())
}

/// Kill a tmux window.
pub fn kill_window(target: &str, verbose: bool) -> Result<(), GroveError> {
    run_tmux(&["kill-window", "-t", target], verbose)?;
    Ok(())
}

/// Get the pane ID of the *current* pane.
///
/// Deliberately takes no target. `display-message -t <target>` answers with the
/// active pane and exits 0 when the target does not exist, so it cannot be used to
/// resolve — or prove the existence of — another task's pane. Use
/// [`locate_task_pane`] against [`list_all_panes`] for that.
pub fn current_pane_id(verbose: bool) -> Result<String, GroveError> {
    run_tmux(&["display-message", "-p", "#{pane_id}"], verbose)
}

/// Find a task's live pane among all panes.
///
/// Liveness must never be decided with `tmux display-message -t <target>`: for a
/// target that does not exist tmux answers with the *active* pane and exits 0, so
/// every lookup "succeeds" and every task looks alive. Match against the real pane
/// list instead.
pub fn locate_task_pane<'a>(
    panes: &'a [PaneInfo],
    pane_id: Option<&str>,
    window: Option<&str>,
    path: &Path,
) -> Option<&'a PaneInfo> {
    // 1. Stable pane id: survives window renames.
    if let Some(id) = pane_id {
        if let Some(p) = panes.iter().find(|p| p.pane_id == id) {
            return Some(p);
        }
    }
    // 2. Recorded `session:window` target: survives the pane being recreated.
    if let Some((session, name)) = window.and_then(split_window_target) {
        if let Some(p) = panes
            .iter()
            .find(|p| p.session_name == session && p.window_name == name)
        {
            return Some(p);
        }
    }
    // 3. Worktree path: last resort when a pane was recreated *and* its window was
    //    renamed. `Path::starts_with` compares whole components, so the sibling
    //    task `tasks/review` never claims `tasks/review-gate`'s pane.
    panes.iter().find(|p| p.current_path.starts_with(path))
}

/// Split a recorded `session:window` target. Sessions cannot contain `:`, so the
/// first separator is the boundary.
fn split_window_target(target: &str) -> Option<(&str, &str)> {
    let (session, name) = target.split_once(':')?;
    (!session.is_empty() && !name.is_empty()).then_some((session, name))
}

/// List all panes across all tmux sessions.
pub fn list_all_panes(verbose: bool) -> Result<Vec<PaneInfo>, GroveError> {
    // NOTE: use #{window_activity}, not #{pane_activity}. tmux does not expose a
    // per-pane activity timestamp (empty as of tmux 3.6) — activity is tracked at
    // the window level. An empty value would parse to 0 and silently kill the
    // activity-based tree sort.
    let format_str = "#{pane_id}\t#{session_name}\t#{window_index}\t#{window_name}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_start_command}\t#{pane_pid}\t#{window_activity}";
    let output = run_tmux(&["list-panes", "-a", "-F", format_str], verbose)?;

    let mut panes = Vec::new();
    for line in output.lines() {
        if let Some(pane) = parse_pane_info_line(line) {
            panes.push(pane);
        }
    }
    Ok(panes)
}

fn parse_pane_info_line(line: &str) -> Option<PaneInfo> {
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 8 {
        return None;
    }
    Some(PaneInfo {
        pane_id: parts[0].to_string(),
        session_name: parts[1].to_string(),
        window_index: parts[2].parse().ok()?,
        window_name: parts[3].to_string(),
        current_path: PathBuf::from(parts[4]),
        current_command: parts[5].to_string(),
        start_command: parts[6].to_string(),
        pid: parts[7].parse().ok()?,
        activity: parts.get(8).and_then(|s| s.parse().ok()).unwrap_or(0),
    })
}

/// Capture only the last N lines of a tmux pane (for agent detection, not preview).
#[allow(dead_code)]
pub fn capture_pane_tail(
    pane_id: &str,
    n_lines: usize,
    verbose: bool,
) -> Result<String, GroveError> {
    let start = format!("-{}", n_lines);
    run_tmux(
        &["capture-pane", "-t", pane_id, "-p", "-S", &start, "-E", "-"],
        verbose,
    )
}

/// Capture pane content using a caller-built argument vector (e.g. a bounded
/// `-S -N` window). Keeps the bounding policy in one testable seam.
pub fn capture_with_args(args: &[String], verbose: bool) -> Result<String, GroveError> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_tmux(&refs, verbose)
}

/// Switch the tmux client to a specific pane.
pub fn switch_to_pane(pane_id: &str, verbose: bool) -> Result<(), GroveError> {
    run_tmux(&["switch-client", "-t", pane_id], verbose)?;
    Ok(())
}

/// Kill a tmux pane.
pub fn kill_pane(pane_id: &str, verbose: bool) -> Result<(), GroveError> {
    run_tmux(&["kill-pane", "-t", pane_id], verbose)?;
    Ok(())
}

/// Create a vertical split next to a target pane.
/// If `cmd` is Some, runs that command; otherwise spawns the default shell.
/// Equalizes pane sizes after splitting. Returns the new pane's ID.
pub fn split_window(
    target_pane: &str,
    cwd: &str,
    cmd: Option<&str>,
    verbose: bool,
) -> Result<String, GroveError> {
    let mut args = vec![
        "split-window",
        "-h",
        "-t",
        target_pane,
        "-c",
        cwd,
        "-P",
        "-F",
        "#{pane_id}",
    ];
    if let Some(c) = cmd {
        args.push(c);
    }
    let new_pane_id = run_tmux(&args, verbose)?;
    let _ = run_tmux(
        &["select-layout", "-t", target_pane, "even-horizontal"],
        verbose,
    );
    Ok(new_pane_id)
}

/// Create a new window in the current session.
/// If `cmd` is Some, runs that command; otherwise spawns the default shell.
/// Returns the new pane's ID.
pub fn new_window(cwd: &str, cmd: Option<&str>, verbose: bool) -> Result<String, GroveError> {
    let mut args = vec!["new-window", "-c", cwd, "-P", "-F", "#{pane_id}"];
    if let Some(c) = cmd {
        args.push(c);
    }
    run_tmux(&args, verbose)
}

/// Register tmux hooks to record new windows/panes as grove projects.
///
/// Idempotent per process: skips the redundant tmux round-trips when already
/// run. When it does register, it always overwrites via `set-hook -g` with the
/// freshly-built safe command, never preserving a possibly-stale hook.
pub fn register_project_hooks(verbose: bool) {
    static REGISTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if REGISTERED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    let grove_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "grove".to_string());

    for arg_vec in project_hook_set_args(&grove_bin) {
        let args: Vec<&str> = arg_vec.iter().map(String::as_str).collect();
        let _ = run_tmux(&args, verbose);
    }
}

/// The events whose hooks record new windows/panes as grove projects.
const PROJECT_HOOK_EVENTS: [&str; 3] = [
    "after-new-window",
    "after-split-window",
    "after-new-session",
];

/// Pure seam: the (event, value) pairs the hook registration overwrites.
/// Every value is the safe `project_hook_command`, enabling tmux-free testing.
fn project_hook_set_commands(grove_bin: &str) -> Vec<(&'static str, String)> {
    let cmd = project_hook_command(grove_bin);
    PROJECT_HOOK_EVENTS
        .iter()
        .map(|event| (*event, cmd.clone()))
        .collect()
}

/// Pure seam: the full `set-hook -g <event> <value>` arg vectors to execute.
/// Always the overwriting `-g` form, never the appending `-a` form.
fn project_hook_set_args(grove_bin: &str) -> Vec<Vec<String>> {
    project_hook_set_commands(grove_bin)
        .into_iter()
        .map(|(event, value)| {
            vec![
                "set-hook".to_string(),
                "-g".to_string(),
                event.to_string(),
                value,
            ]
        })
        .collect()
}

/// Build the per-event run-shell command for the project hook.
/// Uses tmux's `q:` shell-quote modifier and double-quotes both the binary
/// path and the pane path to prevent shell injection via the current path.
fn project_hook_command(grove_bin: &str) -> String {
    format!("run-shell -b '\"{grove_bin}\" project-touch \"#{{q:pane_current_path}}\"'")
}

/// Send raw keys to a tmux target (no -l flag, for keys like Enter).
pub fn send_raw_keys(target: &str, keys: &[&str], verbose: bool) -> Result<(), GroveError> {
    let mut args = vec!["send-keys", "-t", target];
    args.extend_from_slice(keys);
    run_tmux(&args, verbose)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_pane(pane_id: &str, session: &str, window_name: &str, path: &str) -> PaneInfo {
        PaneInfo {
            pane_id: pane_id.to_string(),
            session_name: session.to_string(),
            window_index: 1,
            window_name: window_name.to_string(),
            current_path: PathBuf::from(path),
            current_command: "zsh".to_string(),
            start_command: "zsh".to_string(),
            pid: 1234,
            activity: 0,
        }
    }

    /// The rename drift at its source: tmux rewrites a window's name whenever a
    /// program in the pane emits a title escape, so a window grove created as
    /// `grove-review-gate` silently becomes e.g. `2.1.206` and every recorded
    /// `session:name` target stops matching. Creation must pin the name off.
    #[test]
    fn new_named_window_pins_the_name_against_tmux_renaming() {
        let mut issued: Vec<Vec<String>> = Vec::new();

        let pane_id = new_named_window_with(
            "0",
            "grove-review-gate",
            Path::new("/home/user/tasks/review-gate"),
            |args| {
                issued.push(args.iter().map(|s| s.to_string()).collect());
                Ok("@7\t%183".to_string())
            },
        )
        .expect("window creation should succeed");

        assert_eq!(pane_id, "%183", "pane id comes from the created window");

        let joined: Vec<String> = issued.iter().map(|a| a.join(" ")).collect();
        assert!(
            joined[0].contains("new-window") && joined[0].contains("-n grove-review-gate"),
            "first command creates the named window, got: {:?}",
            joined[0]
        );
        assert!(
            joined
                .iter()
                .any(|c| c.contains("automatic-rename off") && c.contains("@7")),
            "automatic-rename must be pinned off on the new window, got: {joined:?}"
        );
        assert!(
            joined
                .iter()
                .any(|c| c.contains("allow-rename off") && c.contains("@7")),
            "allow-rename must be pinned off on the new window, got: {joined:?}"
        );
    }

    /// A task whose window was killed has no live pane. `tmux display-message -t`
    /// answers with the *active* pane for a target that does not exist (exit 0),
    /// so liveness must be decided against the real pane list, never that call.
    #[test]
    fn locate_task_pane_none_when_window_gone() {
        let panes = [make_pane("%1", "0", "other-window", "/home/user/elsewhere")];

        let found = locate_task_pane(
            &panes,
            Some("%171"),
            Some("0:grove-review-gate"),
            Path::new("/home/user/tasks/review-gate"),
        );

        assert!(found.is_none());
    }

    /// tmux renames windows out from under grove (a program in the pane emits a
    /// title escape and `automatic-rename` rewrites the name). The recorded
    /// pane_id is stable across that, so it is the primary anchor.
    #[test]
    fn locate_task_pane_by_pane_id_despite_window_rename() {
        let panes = [make_pane(
            "%169",
            "0",
            "2.1.208",
            "/home/user/tasks/glance-ship",
        )];

        let found = locate_task_pane(
            &panes,
            Some("%169"),
            Some("0:grove-glance-ship"), // name no longer matches anything
            Path::new("/home/user/tasks/glance-ship"),
        )
        .expect("pane_id anchor should find the renamed window's pane");

        assert_eq!(found.pane_id, "%169");
    }

    /// A pane recreated inside the same window gets a fresh id, so a stale
    /// recorded pane_id must fall through to the `session:window` target.
    #[test]
    fn locate_task_pane_by_window_when_pane_id_stale() {
        let panes = [
            make_pane("%9", "0", "unrelated", "/home/user"),
            make_pane(
                "%172",
                "0",
                "grove-review-gate",
                "/home/user/tasks/review-gate",
            ),
        ];

        let found = locate_task_pane(
            &panes,
            Some("%171"), // stale: pane was recreated as %172
            Some("0:grove-review-gate"),
            Path::new("/home/user/tasks/review-gate"),
        )
        .expect("window target should find the pane when pane_id is stale");

        assert_eq!(found.pane_id, "%172");
    }

    /// Both anchors can rot at once: the pane was recreated (new id) *and* the
    /// window was auto-renamed. The worktree path still identifies the task, and
    /// a subdirectory of it counts — an agent may `cd` deeper while working.
    #[test]
    fn locate_task_pane_by_path_when_pane_id_and_window_stale() {
        let panes = [
            make_pane("%9", "0", "zsh", "/home/user"),
            make_pane(
                "%180",
                "0",
                "2.1.208",
                "/home/user/tasks/glance-ship/artifacts",
            ),
        ];

        let found = locate_task_pane(
            &panes,
            Some("%168"),                // stale
            Some("0:grove-glance-ship"), // drifted away
            Path::new("/home/user/tasks/glance-ship"),
        )
        .expect("worktree path should identify the task's pane when both anchors rot");

        assert_eq!(found.pane_id, "%180");
    }

    /// The path fallback must not claim a *different* task's pane: `tasks/review`
    /// is not a parent of `tasks/review-gate`, despite the string prefix.
    #[test]
    fn locate_task_pane_path_does_not_match_sibling_task() {
        let panes = [make_pane(
            "%5",
            "0",
            "2.1.206",
            "/home/user/tasks/review-gate",
        )];

        let found = locate_task_pane(&panes, None, None, Path::new("/home/user/tasks/review"));

        assert!(found.is_none());
    }

    /// SESSION-cached-once (S16): the underlying resolver runs only once across
    /// multiple `current_session_once` calls against the same cell.
    #[test]
    fn session_cached_once() {
        let cache = OnceLock::new();
        let calls = AtomicUsize::new(0);
        let resolve = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Some("sess".to_string())
        };
        assert_eq!(
            current_session_once(&cache, resolve),
            Some("sess".to_string())
        );
        assert_eq!(
            current_session_once(&cache, resolve),
            Some("sess".to_string())
        );
        assert_eq!(
            current_session_once(&cache, resolve),
            Some("sess".to_string())
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_hook_uses_q_modifier() {
        let cmd = project_hook_command("/usr/local/bin/grove");
        assert!(
            cmd.contains("#{q:pane_current_path}"),
            "hook must use the q: shell-quote modifier: {cmd}"
        );
        assert!(
            !cmd.contains("#{pane_current_path}"),
            "hook must not splice the raw pane path: {cmd}"
        );
    }

    #[test]
    fn test_hook_register_overwrites_not_skips() {
        let bin = "/usr/local/bin/grove";
        let expected = project_hook_command(bin);
        let cmds = project_hook_set_commands(bin);

        assert_eq!(cmds.len(), 3, "must cover all 3 events: {cmds:?}");
        for (_event, value) in &cmds {
            assert_eq!(
                *value, expected,
                "every value must be the safe hook command"
            );
            assert!(
                value.contains("#{q:pane_current_path}"),
                "must use q: modifier: {value}"
            );
            assert!(
                !value.contains("#{pane_current_path}"),
                "must not splice raw pane path: {value}"
            );
        }

        let args = project_hook_set_args(bin);
        for arg_vec in &args {
            assert!(
                arg_vec.iter().any(|a| a == "set-hook"),
                "must use set-hook: {arg_vec:?}"
            );
            assert!(
                arg_vec.iter().any(|a| a == "-g"),
                "must overwrite globally with -g: {arg_vec:?}"
            );
            assert!(
                !arg_vec.iter().any(|a| a == "-a"),
                "must not append with -a: {arg_vec:?}"
            );
        }
    }

    #[test]
    fn test_is_inside_tmux_with_var() {
        // Save and restore the env var
        let original = std::env::var("TMUX").ok();

        // SAFETY: test is single-threaded, env var manipulation is safe here
        unsafe {
            std::env::set_var("TMUX", "/tmp/tmux-1000/default,12345,0");
        }
        assert!(is_inside_tmux());

        unsafe {
            std::env::set_var("TMUX", "");
        }
        assert!(!is_inside_tmux());

        unsafe {
            std::env::remove_var("TMUX");
        }
        assert!(!is_inside_tmux());

        // Restore
        if let Some(val) = original {
            unsafe {
                std::env::set_var("TMUX", val);
            }
        }
    }

    #[test]
    fn test_parse_pane_info_line_valid() {
        let line =
            "%42\tmain\t1\tgrove-task-1\t/home/user/src/grove\tclaude\tclaude\t12345\t1700000000";
        let pane = parse_pane_info_line(line).expect("should parse valid line");
        assert_eq!(pane.pane_id, "%42");
        assert_eq!(pane.session_name, "main");
        assert_eq!(pane.window_index, 1);
        assert_eq!(pane.window_name, "grove-task-1");
        assert_eq!(pane.current_path, PathBuf::from("/home/user/src/grove"));
        assert_eq!(pane.current_command, "claude");
        assert_eq!(pane.start_command, "claude");
        assert_eq!(pane.pid, 12345);
        assert_eq!(pane.activity, 1700000000);
    }

    #[test]
    fn test_parse_pane_info_line_too_few_fields() {
        let line = "%42\tmain\t1";
        assert!(parse_pane_info_line(line).is_none());
    }

    #[test]
    fn test_parse_pane_info_line_invalid_window_index() {
        let line = "%42\tmain\tnotanumber\twindow\t/path\tzsh\tzsh\t999";
        assert!(parse_pane_info_line(line).is_none());
    }

    #[test]
    fn test_parse_pane_info_line_invalid_pid() {
        let line = "%42\tmain\t1\twindow\t/path\tzsh\tzsh\tnotapid";
        assert!(parse_pane_info_line(line).is_none());
    }
}
