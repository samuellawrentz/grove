use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::grove_dir;

const MAX_RECENTS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentEntry {
    pub path: PathBuf,
    pub timestamp: u64,
}

pub fn recents_path() -> PathBuf {
    grove_dir().join("recents.json")
}

pub fn load() -> Vec<RecentEntry> {
    std::fs::read_to_string(recents_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn add(dir: &str) {
    let mut entries = load();
    let path = PathBuf::from(dir);

    // Remove existing entry for this path
    entries.retain(|e| e.path != path);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    entries.insert(
        0,
        RecentEntry {
            path,
            timestamp: now,
        },
    );

    entries.truncate(MAX_RECENTS);

    if let Ok(json) = serde_json::to_string_pretty(&entries) {
        let _ = std::fs::write(recents_path(), json);
    }
}

pub fn remove(index: usize) {
    let mut entries = load();
    if index < entries.len() {
        entries.remove(index);
        if let Ok(json) = serde_json::to_string_pretty(&entries) {
            let _ = std::fs::write(recents_path(), json);
        }
    }
}

pub fn format_relative_time(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let delta = now.saturating_sub(timestamp);

    if delta < 60 {
        "now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}
