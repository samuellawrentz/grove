use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::GroveError;

/// Information about a single tmux pane.
#[derive(Clone)]
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

/// Create a named window in a specific session with a working directory.
pub fn new_named_window(
    session: &str,
    window_name: &str,
    cwd: &Path,
    verbose: bool,
) -> Result<(), GroveError> {
    let cwd_str = cwd
        .to_str()
        .ok_or_else(|| GroveError::General("invalid path for tmux window".to_string()))?;

    run_tmux(
        &[
            "new-window",
            "-t",
            session,
            "-n",
            window_name,
            "-c",
            cwd_str,
        ],
        verbose,
    )?;
    Ok(())
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
pub fn send_keys(target: &str, text: &str, verbose: bool) -> Result<(), GroveError> {
    run_tmux(&["send-keys", "-t", target, "-l", text], verbose)?;
    run_tmux(&["send-keys", "-t", target, "Enter"], verbose)?;
    Ok(())
}

/// Kill a tmux window.
pub fn kill_window(target: &str, verbose: bool) -> Result<(), GroveError> {
    run_tmux(&["kill-window", "-t", target], verbose)?;
    Ok(())
}

/// Get the pane ID for a window target (also used as refresh_pane_id).
pub fn get_pane_id(target: &str, verbose: bool) -> Result<String, GroveError> {
    run_tmux(
        &["display-message", "-t", target, "-p", "#{pane_id}"],
        verbose,
    )
}

/// List all panes across all tmux sessions.
pub fn list_all_panes(verbose: bool) -> Result<Vec<PaneInfo>, GroveError> {
    let format_str = "#{pane_id}\t#{session_name}\t#{window_index}\t#{window_name}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_start_command}\t#{pane_pid}\t#{pane_activity}";
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

/// Capture the visible content of a tmux pane (with ANSI color codes).
pub fn capture_pane(pane_id: &str, verbose: bool) -> Result<String, GroveError> {
    run_tmux(
        &[
            "capture-pane",
            "-t",
            pane_id,
            "-p",
            "-e",
            "-S",
            "-",
            "-E",
            "-",
        ],
        verbose,
    )
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

/// Register tmux hooks to record new windows/panes in grove recents.
pub fn register_recents_hooks(verbose: bool) {
    let grove_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "grove".to_string());

    let events = [
        "after-new-window",
        "after-split-window",
        "after-new-session",
    ];

    for event in &events {
        let cmd = format!("run-shell -b '{grove_bin} recents-add #{{pane_current_path}}'");
        let _ = run_tmux(&["set-hook", "-g", event, &cmd], verbose);
    }
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
