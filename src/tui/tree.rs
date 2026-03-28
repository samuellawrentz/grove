use std::collections::HashMap;
use std::path::PathBuf;

use crate::agent::{AgentFilter, AgentInfo, AgentState, detect_agent_in_pane};
use crate::tmux::PaneInfo;

/// A group of panes sharing the same working directory basename.
pub(crate) struct TreeGroup {
    pub name: String,
    #[allow(dead_code)]
    pub path: PathBuf, // kept for v2 basename disambiguation
    pub expanded: bool,
    pub panes: Vec<TreePane>,
}

/// A single pane entry within a group.
pub(crate) struct TreePane {
    pub pane_info: PaneInfo,
    pub agent: Option<AgentInfo>,
}

/// State for the tree view: groups, cursor, and scroll.
pub(crate) struct TreeState {
    pub groups: Vec<TreeGroup>,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub search_filter: Option<String>,
    pub agent_filter: AgentFilter,
}

impl TreeState {
    /// Build a new tree from pane data and claude states.
    /// Excludes the TUI's own pane via `exclude_pane_id`.
    #[cfg(test)]
    pub fn build(
        panes: &[PaneInfo],
        agent_states: &HashMap<String, AgentState>,
        exclude_pane_id: &str,
    ) -> Self {
        let groups = build_groups(panes, agent_states, exclude_pane_id, &[]);
        TreeState {
            groups,
            cursor: 0,
            scroll_offset: 0,
            search_filter: None,
            agent_filter: AgentFilter::All,
        }
    }

    /// Rebuild the tree preserving expanded state from the current groups.
    pub fn rebuild(
        &mut self,
        panes: &[PaneInfo],
        agent_states: &HashMap<String, AgentState>,
        exclude_pane_id: &str,
    ) {
        let old_expanded: Vec<(String, bool)> = self
            .groups
            .iter()
            .map(|g| (g.name.clone(), g.expanded))
            .collect();
        self.groups = build_groups(panes, agent_states, exclude_pane_id, &old_expanded);
        // Clamp cursor to valid range
        let count = self.visible_count();
        if count == 0 {
            self.cursor = 0;
        } else if self.cursor >= count {
            self.cursor = count - 1;
        }
    }

    /// Get the pane under the cursor, if the cursor is on a pane row (not a group header).
    pub fn selected_pane(&self) -> Option<&TreePane> {
        let mut pos = 0;
        for group in &self.groups {
            if pos == self.cursor {
                // Cursor is on the group header
                return None;
            }
            pos += 1;
            if group.expanded {
                for pane in &group.panes {
                    if pos == self.cursor {
                        return Some(pane);
                    }
                    pos += 1;
                }
            }
        }
        None
    }

    /// Convenience: get the pane_id of the selected pane.
    pub fn selected_pane_id(&self) -> Option<&str> {
        self.selected_pane().map(|p| p.pane_info.pane_id.as_str())
    }

    /// Move cursor by delta, skipping collapsed children.
    #[cfg(test)]
    pub fn move_cursor(&mut self, delta: i32) {
        let count = self.visible_count();
        if count == 0 {
            return;
        }
        let new = self.cursor as i32 + delta;
        if new < 0 {
            self.cursor = 0;
        } else if new >= count as i32 {
            self.cursor = count - 1;
        } else {
            self.cursor = new as usize;
        }
    }

    /// Toggle expand/collapse for the group under the cursor.
    #[cfg(test)]
    pub fn toggle_expand(&mut self) {
        let mut pos = 0;
        for group in &mut self.groups {
            if pos == self.cursor {
                group.expanded = !group.expanded;
                return;
            }
            pos += 1;
            if group.expanded {
                pos += group.panes.len();
            }
        }
    }

    /// Total number of visible rows (group headers + expanded pane rows).
    pub fn visible_count(&self) -> usize {
        let mut count = 0;
        for group in &self.groups {
            count += 1; // group header
            if group.expanded {
                count += group.panes.len();
            }
        }
        count
    }

    /// Check if a pane matches the current search and claude filters.
    /// When search is active, the claude filter is ignored so all panes are searchable.
    pub fn pane_matches(&self, pane: &TreePane, group_name: &str) -> bool {
        let searching = matches!(&self.search_filter, Some(q) if !q.is_empty());
        let search_ok = match &self.search_filter {
            Some(query) if !query.is_empty() => pane_matches_filter(pane, group_name, query),
            _ => true,
        };
        let agent_ok = if searching {
            true
        } else {
            match &self.agent_filter {
                AgentFilter::All => true,
                AgentFilter::AnyAgent => pane.agent.is_some(),
                AgentFilter::Specific(kind) => pane.agent.as_ref().is_some_and(|a| a.kind == *kind),
                AgentFilter::NonAgent => pane.agent.is_none(),
            }
        };
        search_ok && agent_ok
    }

    /// Get positions of all visible pane rows (not group headers), respecting search filter.
    fn pane_positions(&self) -> Vec<usize> {
        let mut positions = Vec::new();
        let mut pos = 0;
        for group in &self.groups {
            pos += 1; // group header
            if group.expanded {
                for pane in &group.panes {
                    if self.pane_matches(pane, &group.name) {
                        positions.push(pos);
                    }
                    pos += 1;
                }
            }
        }
        positions
    }

    /// Move cursor to the next or previous pane row, skipping group headers.
    pub fn move_cursor_to_pane(&mut self, forward: bool) {
        let positions = self.pane_positions();
        if positions.is_empty() {
            return;
        }
        if forward {
            if let Some(&next) = positions.iter().find(|&&p| p > self.cursor) {
                self.cursor = next;
            }
        } else {
            if let Some(&prev) = positions.iter().rev().find(|&&p| p < self.cursor) {
                self.cursor = prev;
            }
        }
    }

    /// Jump cursor to the first visible pane.
    pub fn jump_first_pane(&mut self) {
        let positions = self.pane_positions();
        if let Some(&first) = positions.first() {
            self.cursor = first;
        }
    }

    /// Jump cursor to the last visible pane.
    pub fn jump_last_pane(&mut self) {
        let positions = self.pane_positions();
        if let Some(&last) = positions.last() {
            self.cursor = last;
        }
    }

    /// Find the group index containing the cursor position.
    fn cursor_group_index(&self) -> Option<usize> {
        let mut pos = 0;
        for (i, group) in self.groups.iter().enumerate() {
            if pos == self.cursor {
                return Some(i);
            }
            pos += 1;
            if group.expanded {
                for _ in &group.panes {
                    if pos == self.cursor {
                        return Some(i);
                    }
                    pos += 1;
                }
            }
        }
        None
    }

    /// Collapse the group containing the cursor, moving cursor to group header.
    pub fn collapse_current_group(&mut self) {
        if let Some(group_idx) = self.cursor_group_index()
            && self.groups[group_idx].expanded
        {
            let mut header_pos = 0;
            for i in 0..group_idx {
                header_pos += 1;
                if self.groups[i].expanded {
                    header_pos += self.groups[i].panes.len();
                }
            }
            self.groups[group_idx].expanded = false;
            self.cursor = header_pos;
        }
    }

    /// Expand the group containing the cursor, moving cursor to its first pane.
    pub fn expand_current_group(&mut self) {
        if let Some(group_idx) = self.cursor_group_index()
            && !self.groups[group_idx].expanded
        {
            self.groups[group_idx].expanded = true;
            let mut first_pane_pos = 0;
            for i in 0..group_idx {
                first_pane_pos += 1;
                if self.groups[i].expanded {
                    first_pane_pos += self.groups[i].panes.len();
                }
            }
            first_pane_pos += 1; // skip this group's header
            if !self.groups[group_idx].panes.is_empty() {
                self.cursor = first_pane_pos;
            }
        }
    }

    /// Get the group under the cursor, if the cursor is on a group header.
    pub fn selected_group(&self) -> Option<&TreeGroup> {
        let mut pos = 0;
        for group in &self.groups {
            if pos == self.cursor {
                return Some(group);
            }
            pos += 1;
            if group.expanded {
                pos += group.panes.len();
            }
        }
        None
    }

    /// Get the group containing the cursor, whether on a header or a pane row.
    #[allow(dead_code)]
    pub fn cursor_group(&self) -> Option<&TreeGroup> {
        self.cursor_group_index().map(|i| &self.groups[i])
    }
}

fn fuzzy_match(query: &str, target: &str) -> bool {
    let mut target_chars = target.chars().flat_map(|c| c.to_lowercase());
    for qc in query.chars().flat_map(|c| c.to_lowercase()) {
        loop {
            match target_chars.next() {
                Some(tc) if tc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

fn pane_matches_filter(pane: &TreePane, group_name: &str, query: &str) -> bool {
    fuzzy_match(query, &pane.pane_info.session_name)
        || fuzzy_match(query, &pane.pane_info.current_command)
        || fuzzy_match(query, group_name)
        || fuzzy_match(query, &pane.pane_info.current_path.to_string_lossy())
}

/// Shorten a path fish-style: replace $HOME with ~, keep the last 2 components
/// full, and collapse earlier components to their first character.
/// e.g. `/home/user/src/personal/grove` → `~/s/personal/grove`
fn shorten_path(path: &std::path::Path) -> String {
    let path_str = path.to_string_lossy();
    // Replace $HOME with ~
    let home = dirs::home_dir().unwrap_or_default();
    let (prefix, rest) = if let Ok(stripped) = path.strip_prefix(&home) {
        ("~", stripped.to_path_buf())
    } else {
        ("", path.to_path_buf())
    };

    let components: Vec<&str> = rest
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();

    if components.is_empty() {
        return if prefix.is_empty() {
            path_str.to_string()
        } else {
            prefix.to_string()
        };
    }

    // Keep last 2 full, shorten the rest to first char
    let keep_full = 2;
    let mut parts: Vec<String> = Vec::new();
    for (i, comp) in components.iter().enumerate() {
        if i < components.len().saturating_sub(keep_full) {
            parts.push(comp.chars().next().unwrap_or('.').to_string());
        } else {
            parts.push(comp.to_string());
        }
    }
    if !prefix.is_empty() {
        format!("{prefix}/{}", parts.join("/"))
    } else {
        parts.join("/")
    }
}

fn build_groups(
    panes: &[PaneInfo],
    agent_states: &HashMap<String, AgentState>,
    exclude_pane_id: &str,
    old_expanded: &[(String, bool)],
) -> Vec<TreeGroup> {
    // Group panes by parent directory
    let mut group_map: HashMap<PathBuf, Vec<TreePane>> = HashMap::new();

    for pane in panes {
        if pane.pane_id == exclude_pane_id || pane.current_command == "grove" {
            continue;
        }

        // Skip panes whose working directory no longer exists (e.g. deleted worktrees)
        #[cfg(not(test))]
        if !pane.current_path.exists() {
            continue;
        }

        let agent = detect_agent_in_pane(pane, agent_states);

        let tree_pane = TreePane {
            pane_info: PaneInfo {
                pane_id: pane.pane_id.clone(),
                session_name: pane.session_name.clone(),
                window_index: pane.window_index,
                window_name: pane.window_name.clone(),
                current_path: pane.current_path.clone(),
                current_command: pane.current_command.clone(),
                start_command: pane.start_command.clone(),
                pid: pane.pid,
                activity: pane.activity,
            },
            agent,
        };

        let parent = pane
            .current_path
            .parent()
            .unwrap_or(&pane.current_path)
            .to_path_buf();

        group_map.entry(parent).or_default().push(tree_pane);
    }

    let mut groups: Vec<TreeGroup> = group_map
        .into_iter()
        .map(|(path, mut panes)| {
            let name = shorten_path(&path);
            // Sort panes by activity descending (most recent first), then session:window as tiebreaker
            panes.sort_by(|a, b| {
                b.pane_info
                    .activity
                    .cmp(&a.pane_info.activity)
                    .then(a.pane_info.session_name.cmp(&b.pane_info.session_name))
                    .then(a.pane_info.window_index.cmp(&b.pane_info.window_index))
            });

            // Preserve expanded state from previous build
            let expanded = old_expanded
                .iter()
                .find(|(n, _)| n == &name)
                .map(|(_, e)| *e)
                .unwrap_or(true); // Default to expanded

            TreeGroup {
                name,
                path,
                expanded,
                panes,
            }
        })
        .collect();

    // Sort groups by most recent pane activity (most active first), alphabetical tiebreaker
    groups.sort_by(|a, b| {
        let a_max = a
            .panes
            .iter()
            .map(|p| p.pane_info.activity)
            .max()
            .unwrap_or(0);
        let b_max = b
            .panes
            .iter()
            .map(|p| p.pane_info.activity)
            .max()
            .unwrap_or(0);
        b_max.cmp(&a_max).then(a.name.cmp(&b.name))
    });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentInfo, AgentKind, AgentState};
    use std::path::PathBuf;

    fn make_pane(id: &str, session: &str, win_idx: u32, path: &str, cmd: &str) -> PaneInfo {
        make_pane_win(id, session, win_idx, &format!("win-{win_idx}"), path, cmd)
    }

    fn make_pane_win(
        id: &str,
        session: &str,
        win_idx: u32,
        win_name: &str,
        path: &str,
        cmd: &str,
    ) -> PaneInfo {
        PaneInfo {
            pane_id: id.to_string(),
            session_name: session.to_string(),
            window_index: win_idx,
            window_name: win_name.to_string(),
            current_path: PathBuf::from(path),
            current_command: cmd.to_string(),
            start_command: String::new(),
            pid: 1000,
            activity: 0,
        }
    }

    #[test]
    fn test_groups_by_parent_directory() {
        // Panes in /opt/src/grove and /opt/src/other share parent /opt/src
        let panes = vec![
            make_pane("%1", "main", 0, "/opt/src/grove", "zsh"),
            make_pane("%2", "main", 1, "/opt/src/grove", "claude"),
            make_pane("%3", "work", 0, "/opt/src/other", "vim"),
            make_pane("%5", "dev", 0, "/tmp/third", "zsh"),
        ];
        let states = HashMap::new();
        let tree = TreeState::build(&panes, &states, "");

        // /opt/src siblings → 1 group, /tmp/third → another
        assert_eq!(tree.groups.len(), 2);
        let names: Vec<&str> = tree.groups.iter().map(|g| g.name.as_str()).collect();
        assert!(names.contains(&"opt/src"));
        assert!(names.contains(&"tmp"));
    }

    #[test]
    fn test_sibling_dirs_grouped() {
        // Two panes in different subdirs of the same parent
        let panes = vec![
            make_pane("%1", "main", 0, "/home/user/tasks/task-a/console", "zsh"),
            make_pane("%2", "work", 0, "/home/user/tasks/task-a/api", "zsh"),
        ];
        let states = HashMap::new();
        let tree = TreeState::build(&panes, &states, "");

        // Both share parent /home/user/tasks/task-a → 1 group
        assert_eq!(tree.groups.len(), 1);
        assert_eq!(tree.groups[0].panes.len(), 2);
    }

    #[test]
    fn test_shorten_path_fish_style() {
        assert_eq!(
            shorten_path(std::path::Path::new("/opt/src/grove")),
            "o/src/grove"
        );
        assert_eq!(
            shorten_path(std::path::Path::new("/a/b/c/d/e")),
            "a/b/c/d/e"
        );
        assert_eq!(shorten_path(std::path::Path::new("/tmp")), "tmp");
    }

    #[test]
    fn test_excludes_own_pane() {
        let panes = vec![
            make_pane("%1", "main", 0, "/opt/src/grove", "zsh"),
            make_pane("%2", "main", 1, "/opt/src/grove", "zsh"),
        ];
        let states = HashMap::new();
        let tree = TreeState::build(&panes, &states, "%1");

        assert_eq!(tree.groups.len(), 1);
        assert_eq!(tree.groups[0].panes.len(), 1);
        assert_eq!(tree.groups[0].panes[0].pane_info.pane_id, "%2");
    }

    #[test]
    fn test_root_path_group_name() {
        let panes = vec![make_pane("%1", "main", 0, "/tmp", "zsh")];
        let states = HashMap::new();
        let tree = TreeState::build(&panes, &states, "");

        // Parent of /tmp is / → shorten_path("/") returns "/"
        assert_eq!(tree.groups[0].name, "/");
    }

    #[test]
    fn test_agent_detection_from_state_file() {
        let panes = vec![make_pane("%1", "main", 0, "/home/user/src/grove", "zsh")];
        let mut states = HashMap::new();
        states.insert("%1".to_string(), AgentState::Waiting);
        let tree = TreeState::build(&panes, &states, "");

        let agent = tree.groups[0].panes[0].agent.as_ref().unwrap();
        assert_eq!(agent.kind, AgentKind::Claude);
        assert_eq!(agent.state, AgentState::Waiting);
    }

    #[test]
    fn test_agent_detection_from_command_fallback() {
        let panes = vec![make_pane("%1", "main", 0, "/home/user/src/grove", "claude")];
        let states = HashMap::new();
        let tree = TreeState::build(&panes, &states, "");

        let agent = tree.groups[0].panes[0].agent.as_ref().unwrap();
        assert_eq!(agent.kind, AgentKind::Claude);
        assert_eq!(agent.state, AgentState::Active);
    }

    #[test]
    fn test_non_agent_pane() {
        let panes = vec![make_pane("%1", "main", 0, "/home/user/src/grove", "vim")];
        let states = HashMap::new();
        let tree = TreeState::build(&panes, &states, "");

        assert!(tree.groups[0].panes[0].agent.is_none());
    }

    #[test]
    fn test_cursor_navigation() {
        let panes = vec![
            make_pane("%1", "main", 0, "/opt/a/x", "zsh"),
            make_pane("%2", "main", 1, "/opt/a/y", "zsh"),
            make_pane("%3", "work", 0, "/opt/b/z", "zsh"),
        ];
        let states = HashMap::new();
        let mut tree = TreeState::build(&panes, &states, "");

        // Groups: /opt/a (2 panes), /opt/b (1 pane) → headers + panes = 5
        assert_eq!(tree.visible_count(), 5);
        assert_eq!(tree.cursor, 0);

        // Move down to first pane
        tree.move_cursor(1);
        assert_eq!(tree.cursor, 1);
        assert!(tree.selected_pane().is_some());
        assert_eq!(tree.selected_pane_id(), Some("%1"));

        // Move to end
        tree.move_cursor(100);
        assert_eq!(tree.cursor, 4);
        assert_eq!(tree.selected_pane_id(), Some("%3"));

        // Move past beginning
        tree.move_cursor(-100);
        assert_eq!(tree.cursor, 0);
        // Cursor on group header
        assert!(tree.selected_pane().is_none());
    }

    #[test]
    fn test_collapsed_group_hides_children() {
        let panes = vec![
            make_pane("%1", "main", 0, "/opt/a/x", "zsh"),
            make_pane("%2", "main", 1, "/opt/a/y", "zsh"),
            make_pane("%3", "work", 0, "/opt/b/z", "zsh"),
        ];
        let states = HashMap::new();
        let mut tree = TreeState::build(&panes, &states, "");

        // Collapse first group (cursor is on group "a" at position 0)
        tree.toggle_expand();
        assert!(!tree.groups[0].expanded);

        // Visible: a(collapsed), b(header), %3
        assert_eq!(tree.visible_count(), 3);

        // Move to position 1 -- should be group "b" header
        tree.move_cursor(1);
        assert!(tree.selected_pane().is_none());

        // Move to position 2 -- should be pane %3
        tree.move_cursor(1);
        assert_eq!(tree.selected_pane_id(), Some("%3"));
    }

    #[test]
    fn test_rebuild_preserves_expanded_state() {
        let panes = vec![
            make_pane("%1", "main", 0, "/opt/a/x", "zsh"),
            make_pane("%2", "work", 0, "/opt/b/y", "zsh"),
        ];
        let states = HashMap::new();
        let mut tree = TreeState::build(&panes, &states, "");

        // Collapse group "a"
        tree.toggle_expand();
        assert!(!tree.groups[0].expanded);

        // Rebuild with same data
        tree.rebuild(&panes, &states, "");

        // "a" should still be collapsed
        assert!(!tree.groups[0].expanded);
        assert!(tree.groups[1].expanded);
    }

    #[test]
    fn test_empty_tree() {
        let panes: Vec<PaneInfo> = vec![];
        let states = HashMap::new();
        let mut tree = TreeState::build(&panes, &states, "");

        assert_eq!(tree.visible_count(), 0);
        assert!(tree.selected_pane().is_none());
        tree.move_cursor(1); // Should not panic
        assert_eq!(tree.cursor, 0);
    }

    #[test]
    fn test_panes_sorted_within_group() {
        let panes = vec![
            make_pane("%3", "work", 2, "/opt/src/c", "zsh"),
            make_pane("%1", "main", 0, "/opt/src/a", "zsh"),
            make_pane("%2", "main", 1, "/opt/src/b", "zsh"),
        ];
        let states = HashMap::new();
        let tree = TreeState::build(&panes, &states, "");

        let group = &tree.groups[0];
        assert_eq!(group.panes[0].pane_info.pane_id, "%1"); // main
        assert_eq!(group.panes[1].pane_info.pane_id, "%2"); // main
        assert_eq!(group.panes[2].pane_info.pane_id, "%3"); // work
    }
}
