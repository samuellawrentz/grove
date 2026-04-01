use crate::error::GroveError;
use crate::tmux;

/// Find the pane above the current one (the claude pane).
fn detect_target_pane() -> Result<String, GroveError> {
    let my_pane = tmux::get_pane_id("", false)?;
    let output = tmux::run_tmux(&["list-panes", "-F", "#{pane_id} #{pane_top}"], false)?;

    let mut panes: Vec<(&str, i32)> = output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let id = parts.next()?;
            let top: i32 = parts.next()?.parse().ok()?;
            Some((id, top))
        })
        .collect();

    panes.sort_by_key(|(_, top)| *top);

    let my_idx = panes.iter().position(|(id, _)| *id == my_pane);
    if let Some(idx) = my_idx {
        if idx > 0 {
            return Ok(panes[idx - 1].0.to_string());
        }
    }

    panes
        .iter()
        .find(|(id, _)| *id != my_pane)
        .map(|(id, _)| id.to_string())
        .ok_or_else(|| GroveError::General("no target pane found".to_string()))
}

pub fn run(target: Option<&str>) -> Result<(), GroveError> {
    let target_pane = match target {
        Some(t) => t.to_string(),
        None => detect_target_pane()?,
    };

    let tmp_dir = std::env::temp_dir();
    let tmp_file = tmp_dir.join(format!("grove-compose-{}.md", std::process::id()));
    std::fs::write(&tmp_file, "")
        .map_err(|e| GroveError::General(format!("cannot write temp file: {e}")))?;

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
    let status = std::process::Command::new(&editor)
        .arg(&tmp_file)
        .status()
        .map_err(|e| GroveError::General(format!("failed to launch {editor}: {e}")))?;

    if !status.success() {
        let _ = std::fs::remove_file(&tmp_file);
        return Ok(());
    }

    let edited = std::fs::read_to_string(&tmp_file)
        .map_err(|e| GroveError::General(format!("cannot read temp file: {e}")))?;
    let _ = std::fs::remove_file(&tmp_file);

    if !edited.trim().is_empty() {
        // Load content into tmux paste buffer, then paste to target pane
        let mut child = std::process::Command::new("tmux")
            .args(["load-buffer", "-"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| GroveError::General(format!("tmux load-buffer failed: {e}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(edited.as_bytes())
                .map_err(|e| GroveError::General(format!("failed to write to tmux: {e}")))?;
        } // stdin dropped here, closes pipe

        let status = child
            .wait()
            .map_err(|e| GroveError::General(format!("tmux load-buffer failed: {e}")))?;

        if status.success() {
            tmux::run_tmux(&["paste-buffer", "-t", &target_pane], false)?;
        }
    }

    Ok(())
}
