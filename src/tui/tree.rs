use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::flat_rows::FlatRows;
use crate::agent::{detect_agent_in_pane, AgentFilter, AgentInfo, AgentKind, AgentState};
use crate::tmux::PaneInfo;

/// Cap on cached project-root mappings; oldest insertions evicted past this.
const ROOT_CACHE_CAP: usize = 1024;

/// Bounded path→project-root cache with insertion-order eviction and a `gc`
/// that drops entries for paths no longer live.
#[derive(Default)]
struct ProjectRootCache {
    map: HashMap<PathBuf, PathBuf>,
    order: Vec<PathBuf>,
}

impl ProjectRootCache {
    fn get(&self, path: &Path) -> Option<&PathBuf> {
        self.map.get(path)
    }

    fn insert(&mut self, key: PathBuf, root: PathBuf) {
        if self.map.len() >= ROOT_CACHE_CAP && !self.map.contains_key(&key) {
            if let Some(oldest) = (!self.order.is_empty()).then(|| self.order.remove(0)) {
                self.map.remove(&oldest);
            }
        }
        if self.map.insert(key.clone(), root).is_none() {
            self.order.push(key);
        }
    }

    /// Drop entries whose key path is not in `live` and enforce capacity.
    #[allow(dead_code)]
    fn gc(&mut self, live: &HashSet<PathBuf>) {
        self.map.retain(|k, _| live.contains(k));
        self.order.retain(|k| self.map.contains_key(k));
        while self.map.len() > ROOT_CACHE_CAP {
            if self.order.is_empty() {
                break;
            }
            let oldest = self.order.remove(0);
            self.map.remove(&oldest);
        }
    }
}

static PROJECT_ROOT_CACHE: Mutex<Option<ProjectRootCache>> = Mutex::new(None);

/// The real git-toplevel lookup: shells out to `git rev-parse --show-toplevel`.
/// This is the default root-resolver injected into `build_groups` in production
/// (seam S-rootresolver); tests substitute a pure closure to drive grouping
/// without invoking git.
fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()))
}

fn resolve_project_root(path: &Path) -> PathBuf {
    resolve_project_root_with(path, git_toplevel)
}

/// Resolve a pane's working directory to its project-root group key, using an
/// injectable `git_root` lookup. Production passes `git_toplevel`; tests pass a
/// pure closure so workspace grouping can be exercised without real git.
fn resolve_project_root_with(path: &Path, git_root: impl Fn(&Path) -> Option<PathBuf>) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut cache = PROJECT_ROOT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(ProjectRootCache::default);
    if let Some(root) = cache.get(&canonical) {
        return root.clone();
    }
    let git_root = git_root(&canonical);

    let root = match git_root {
        Some(ref gr) => {
            // Check if git root's parent is a workspace (no .git, 2+ .git children)
            if let Some(parent) = gr.parent() {
                if is_workspace(parent) {
                    parent.to_path_buf()
                } else {
                    gr.clone()
                }
            } else {
                gr.clone()
            }
        }
        None => canonical.clone(),
    };
    cache.insert(canonical, root.clone());
    root
}

/// A workspace is a directory that has no .git itself but contains 2+ direct
/// child directories with .git. Only checks immediate children to avoid
/// false positives on broad directories like ~/src/.
fn is_workspace(dir: &Path) -> bool {
    if dir.join(".git").exists() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let git_child_count = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().join(".git").exists())
        .count();
    git_child_count >= 2
}

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
    /// User-asserted mark forcing this pane into the "others" tab.
    pub forced_other: bool,
}

/// A single visible (filter-aware, compacted) row in display space.
///
/// Display space is the *rendered* row list: group headers that have at least
/// one matching pane, each followed by their matching pane rows. It is the
/// space `ui::draw_tree` actually pushes lines into and scrolls. Each row also
/// carries its absolute `cursor`-space index so the cursor (which lives in the
/// full, unfiltered row space) can be located within the compacted display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayKind {
    Group(usize),
    Pane(usize, usize),
}

/// A compacted, filter-aware visible row plus its absolute cursor-space index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayRow {
    pub kind: DisplayKind,
    /// The absolute index in full `cursor` space this row corresponds to.
    pub cursor_index: usize,
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
        let recorded = HashMap::new();
        let marked = HashSet::new();
        let groups = build_groups(
            panes,
            agent_states,
            &recorded,
            &marked,
            None,
            exclude_pane_id,
            &[],
        );
        TreeState {
            groups,
            cursor: 0,
            scroll_offset: 0,
            search_filter: None,
            agent_filter: AgentFilter::AnyAgent,
        }
    }

    /// Rebuild the tree preserving expanded state from the current groups.
    pub fn rebuild(
        &mut self,
        panes: &[PaneInfo],
        agent_states: &HashMap<String, AgentState>,
        recorded_kinds: &HashMap<String, AgentKind>,
        marked_other: &HashSet<String>,
        current_session: Option<&str>,
        exclude_pane_id: &str,
    ) {
        let old_expanded: Vec<(String, bool)> = self
            .groups
            .iter()
            .map(|g| (g.name.clone(), g.expanded))
            .collect();
        // Capture the pane under the cursor so we can re-anchor onto it after
        // panes re-sort (e.g. a just-active pane floats up on a background tick).
        let selected_id = self.selected_pane_id().map(str::to_string);
        self.groups = build_groups(
            panes,
            agent_states,
            recorded_kinds,
            marked_other,
            current_session,
            exclude_pane_id,
            &old_expanded,
        );
        // Re-anchor the cursor onto the same pane if it still exists; otherwise
        // fall back to clamping the numeric cursor into range.
        if let Some(pos) = selected_id.and_then(|id| self.pane_cursor_position(&id)) {
            self.cursor = pos;
        } else {
            let count = self.visible_count();
            if count == 0 {
                self.cursor = 0;
            } else if self.cursor >= count {
                self.cursor = count - 1;
            }
        }
    }

    /// The single canonical walk: every visible row (group headers + expanded
    /// pane rows) in order, each tagged with its kind and absolute `cursor`-space
    /// index. Unfiltered — `display_rows()` derives the filtered view from this.
    /// All header/expand traversal routes through here (G3: no move math).
    pub(crate) fn rows(&self) -> Vec<DisplayRow> {
        let mut rows = Vec::new();
        let mut pos = 0;
        for (gi, group) in self.groups.iter().enumerate() {
            rows.push(DisplayRow {
                kind: DisplayKind::Group(gi),
                cursor_index: pos,
            });
            pos += 1;
            if group.expanded {
                for pi in 0..group.panes.len() {
                    rows.push(DisplayRow {
                        kind: DisplayKind::Pane(gi, pi),
                        cursor_index: pos,
                    });
                    pos += 1;
                }
            }
        }
        rows
    }

    /// The row under the cursor, if any (header or pane).
    fn row_at_cursor(&self) -> Option<DisplayRow> {
        self.rows()
            .into_iter()
            .find(|r| r.cursor_index == self.cursor)
    }

    /// Get the pane under the cursor, if the cursor is on a pane row (not a group header).
    #[must_use]
    pub fn selected_pane(&self) -> Option<&TreePane> {
        match self.row_at_cursor()?.kind {
            DisplayKind::Pane(gi, pi) => Some(&self.groups[gi].panes[pi]),
            DisplayKind::Group(_) => None,
        }
    }

    /// Convenience: get the pane_id of the selected pane.
    #[must_use]
    pub fn selected_pane_id(&self) -> Option<&str> {
        self.selected_pane().map(|p| p.pane_info.pane_id.as_str())
    }

    /// Move cursor by delta, skipping collapsed children. Cursor/overrun math is
    /// the shared `FlatRows` trait (one copy across tree + diff).
    #[cfg(test)]
    pub fn move_cursor(&mut self, delta: i32) {
        if self.visible_count() == 0 {
            return;
        }
        if delta >= 0 {
            FlatRows::move_down_by(self, delta as usize);
        } else {
            FlatRows::move_up_by(self, delta.unsigned_abs() as usize);
        }
    }

    /// Toggle expand/collapse for the group under the cursor.
    #[cfg(test)]
    pub fn toggle_expand(&mut self) {
        if let Some(DisplayKind::Group(gi)) = self.row_at_cursor().map(|r| r.kind) {
            self.groups[gi].expanded = !self.groups[gi].expanded;
        }
    }

    /// Total number of visible rows (group headers + expanded pane rows).
    #[must_use]
    pub fn visible_count(&self) -> usize {
        self.rows().len()
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
                AgentFilter::AnyAgent => pane.agent.is_some(),
                AgentFilter::Others => pane.agent.is_none(),
            }
        };
        search_ok && agent_ok
    }

    /// The compacted, filter-aware ordered list of visible rows: group headers
    /// that have matching panes followed by their matching pane rows. Each row
    /// carries its absolute `cursor`-space index. This is the single source of
    /// truth for what `ui::draw_tree` renders and scrolls.
    pub(crate) fn display_rows(&self) -> Vec<DisplayRow> {
        // Derive the filtered view from the canonical unfiltered walk: keep
        // matching pane rows, and headers only when their group has a match.
        let group_has_match = |gi: usize| {
            self.groups[gi]
                .panes
                .iter()
                .any(|p| self.pane_matches(p, &self.groups[gi].name))
        };
        self.rows()
            .into_iter()
            .filter(|r| match r.kind {
                DisplayKind::Group(gi) => group_has_match(gi),
                DisplayKind::Pane(gi, pi) => {
                    self.pane_matches(&self.groups[gi].panes[pi], &self.groups[gi].name)
                }
            })
            .collect()
    }

    /// The cursor's index within display space, if the cursor sits on a visible
    /// (matching) row. Hidden/filtered cursor positions yield `None`.
    pub(crate) fn cursor_display_index(&self) -> Option<usize> {
        self.display_rows()
            .iter()
            .position(|r| r.cursor_index == self.cursor)
    }

    /// Clamp `scroll_offset` against display space so the offset never exceeds
    /// the number of display rows and the cursor's display row stays inside the
    /// visible window `[scroll_offset, scroll_offset + height)`.
    pub(crate) fn clamp_scroll(&mut self, height: usize) {
        let rows = self.display_rows();
        let len = rows.len();
        if self.scroll_offset > len {
            self.scroll_offset = len;
        }
        if height == 0 {
            return;
        }
        if let Some(idx) = rows.iter().position(|r| r.cursor_index == self.cursor) {
            if idx < self.scroll_offset {
                self.scroll_offset = idx;
            } else if idx >= self.scroll_offset + height {
                self.scroll_offset = idx + 1 - height;
            }
        }
    }

    /// Find the absolute `cursor`-space position of a pane by its id.
    fn pane_cursor_position(&self, pane_id: &str) -> Option<usize> {
        self.rows().into_iter().find_map(|r| match r.kind {
            DisplayKind::Pane(gi, pi) if self.groups[gi].panes[pi].pane_info.pane_id == pane_id => {
                Some(r.cursor_index)
            }
            _ => None,
        })
    }

    /// Get positions of all visible pane rows (not group headers), respecting search filter.
    fn pane_positions(&self) -> Vec<usize> {
        self.display_rows()
            .into_iter()
            .filter(|r| matches!(r.kind, DisplayKind::Pane(..)))
            .map(|r| r.cursor_index)
            .collect()
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

    /// Jump cursor to the pane with the given id if it's visible. Returns
    /// whether a match was found (so callers can fall back).
    pub fn jump_to_pane(&mut self, pane_id: &str) -> bool {
        match self.pane_cursor_position(pane_id) {
            Some(pos) => {
                self.cursor = pos;
                true
            }
            None => false,
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
        self.row_at_cursor().map(|r| match r.kind {
            DisplayKind::Group(gi) | DisplayKind::Pane(gi, _) => gi,
        })
    }

    /// Absolute `cursor`-space position of a group's header row.
    fn group_header_position(&self, group_idx: usize) -> usize {
        self.rows()
            .into_iter()
            .find_map(|r| match r.kind {
                DisplayKind::Group(gi) if gi == group_idx => Some(r.cursor_index),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// Collapse the group containing the cursor, moving cursor to group header.
    pub fn collapse_current_group(&mut self) {
        if let Some(group_idx) = self.cursor_group_index() {
            if self.groups[group_idx].expanded {
                let header_pos = self.group_header_position(group_idx);
                self.groups[group_idx].expanded = false;
                self.cursor = header_pos;
            }
        }
    }

    /// Expand the group containing the cursor, moving cursor to its first pane.
    pub fn expand_current_group(&mut self) {
        if let Some(group_idx) = self.cursor_group_index() {
            if !self.groups[group_idx].expanded {
                let header_pos = self.group_header_position(group_idx);
                self.groups[group_idx].expanded = true;
                if !self.groups[group_idx].panes.is_empty() {
                    self.cursor = header_pos + 1; // first pane follows the header
                }
            }
        }
    }

    /// Get the group under the cursor, if the cursor is on a group header.
    pub fn selected_group(&self) -> Option<&TreeGroup> {
        match self.row_at_cursor()?.kind {
            DisplayKind::Group(gi) => Some(&self.groups[gi]),
            DisplayKind::Pane(..) => None,
        }
    }

    /// Get the group containing the cursor, whether on a header or a pane row.
    #[allow(dead_code)]
    pub fn cursor_group(&self) -> Option<&TreeGroup> {
        self.cursor_group_index().map(|i| &self.groups[i])
    }
}

impl FlatRows for TreeState {
    fn total(&self) -> usize {
        self.visible_count()
    }
    fn cursor(&self) -> usize {
        self.cursor
    }
    fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor;
    }
    // No per-step side effect: tree expand/collapse is driven by explicit keys,
    // not by cursor movement (unlike diff auto-expand).
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
pub(crate) fn shorten_path(path: &std::path::Path) -> String {
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
    let mut parts: Vec<String> = Vec::with_capacity(components.len());
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

/// A pane needs the user's attention when its agent has finished a turn
/// (`Idle`, i.e. the Stop hook fired) or is blocked on an approval prompt
/// (`Waiting`). These float to the top, newest attention event first.
fn needs_attention(p: &TreePane) -> bool {
    p.agent
        .as_ref()
        .map(|a| matches!(a.state, AgentState::Waiting | AgentState::Idle))
        .unwrap_or(false)
}

fn build_groups(
    panes: &[PaneInfo],
    agent_states: &HashMap<String, AgentState>,
    recorded_kinds: &HashMap<String, AgentKind>,
    marked_other: &HashSet<String>,
    current_session: Option<&str>,
    exclude_pane_id: &str,
    old_expanded: &[(String, bool)],
) -> Vec<TreeGroup> {
    build_groups_with_resolver(
        panes,
        agent_states,
        recorded_kinds,
        marked_other,
        current_session,
        exclude_pane_id,
        old_expanded,
        resolve_project_root,
    )
}

/// `build_groups` with an injectable project-root resolver (seam S-rootresolver).
/// Production delegates here with `resolve_project_root` (which shells out to
/// real git); tests pass a pure resolver to drive workspace grouping without git.
#[allow(clippy::too_many_arguments)]
fn build_groups_with_resolver(
    panes: &[PaneInfo],
    agent_states: &HashMap<String, AgentState>,
    recorded_kinds: &HashMap<String, AgentKind>,
    marked_other: &HashSet<String>,
    current_session: Option<&str>,
    exclude_pane_id: &str,
    old_expanded: &[(String, bool)],
    resolve_root: impl Fn(&Path) -> PathBuf,
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

        let forced_other = marked_other.contains(&pane.pane_id);
        // A user-marked pane is forced into the "others" tab by dropping its
        // detected agent.
        let agent = if forced_other {
            None
        } else {
            detect_agent_in_pane(pane, agent_states, recorded_kinds)
        };

        let tree_pane = TreePane {
            pane_info: pane.clone(),
            agent,
            forced_other,
        };

        let project_root = resolve_root(&pane.current_path);
        group_map.entry(project_root).or_default().push(tree_pane);
    }

    let mut groups: Vec<TreeGroup> = group_map
        .into_iter()
        .map(|(path, mut panes)| {
            let name = shorten_path(&path);
            // Tiered sort: attention-needing panes first, ordered among
            // themselves by most recent activity (≈ when the Stop hook fired);
            // then panes in current tmux session, then by activity desc;
            // session:window as final tiebreaker.
            panes.sort_by(|a, b| {
                let a_att = needs_attention(a);
                let b_att = needs_attention(b);
                let a_cur = current_session.is_some_and(|s| a.pane_info.session_name == s);
                let b_cur = current_session.is_some_and(|s| b.pane_info.session_name == s);
                b_att
                    .cmp(&a_att)
                    .then_with(|| {
                        if a_att && b_att {
                            b.pane_info.activity.cmp(&a.pane_info.activity)
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    })
                    .then(b_cur.cmp(&a_cur))
                    .then(b.pane_info.activity.cmp(&a.pane_info.activity))
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

    // Tiered group sort: groups with an attention-needing pane first, then
    // groups containing a pane in the current tmux session, then by most recent
    // pane activity, alphabetical tiebreaker.
    groups.sort_by(|a, b| {
        let a_att = a.panes.iter().any(needs_attention);
        let b_att = b.panes.iter().any(needs_attention);
        let a_cur =
            current_session.is_some_and(|s| a.panes.iter().any(|p| p.pane_info.session_name == s));
        let b_cur =
            current_session.is_some_and(|s| b.panes.iter().any(|p| p.pane_info.session_name == s));
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
        b_att
            .cmp(&a_att)
            .then(b_cur.cmp(&a_cur))
            .then(b_max.cmp(&a_max))
            .then(a.name.cmp(&b.name))
    });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentKind, AgentState};
    use std::path::PathBuf;

    fn make_pane(id: &str, session: &str, win_idx: u32, path: &str, cmd: &str) -> PaneInfo {
        make_pane_win(id, session, win_idx, &format!("win-{win_idx}"), path, cmd)
    }

    #[test]
    fn cache_evicts_dead_and_bounds() {
        let mut cache = ProjectRootCache::default();
        for i in 0..(ROOT_CACHE_CAP + 10) {
            cache.insert(PathBuf::from(format!("/p/{i}")), PathBuf::from("/root"));
        }
        assert!(cache.map.len() <= ROOT_CACHE_CAP);

        let keep = ROOT_CACHE_CAP + 5; // a surviving (recent) entry
        let live: HashSet<PathBuf> = [PathBuf::from(format!("/p/{keep}"))].into_iter().collect();
        cache.gc(&live);
        assert!(cache.map.contains_key(&PathBuf::from(format!("/p/{keep}"))));
        assert!(!cache.map.contains_key(Path::new("/p/0")));
        assert!(cache.map.len() <= live.len());
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
    fn test_groups_by_git_root() {
        // Panes in different dirs with no git root — each becomes its own group
        let panes = vec![
            make_pane("%1", "main", 0, "/opt/src/grove", "zsh"),
            make_pane("%2", "main", 1, "/opt/src/grove", "claude"),
            make_pane("%3", "work", 0, "/opt/src/other", "vim"),
            make_pane("%5", "dev", 0, "/tmp/third", "zsh"),
        ];
        let states = HashMap::new();
        let tree = TreeState::build(&panes, &states, "");

        // /opt/src/grove (2 panes) → 1 group, /opt/src/other → 1 group, /tmp/third → 1 group
        assert_eq!(tree.groups.len(), 3);
        let names: Vec<&str> = tree.groups.iter().map(|g| g.name.as_str()).collect();
        assert!(names.contains(&"o/src/grove"));
        assert!(names.contains(&"o/src/other"));
        assert!(names.contains(&"tmp/third"));
    }

    #[test]
    fn test_same_dir_grouped() {
        // Two panes in the same directory are grouped together
        let panes = vec![
            make_pane("%1", "main", 0, "/home/user/tasks/task-a", "zsh"),
            make_pane("%2", "work", 0, "/home/user/tasks/task-a", "zsh"),
        ];
        let states = HashMap::new();
        let tree = TreeState::build(&panes, &states, "");

        // Same path → 1 group
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
        // Use a non-existent single-component path so canonicalize() falls back to
        // the literal path (avoids platform symlink resolution like /tmp → /private/tmp).
        let panes = vec![make_pane("%1", "main", 0, "/grove-nogit-xyz", "zsh")];
        let states = HashMap::new();
        let tree = TreeState::build(&panes, &states, "");

        // No git root → groups by the path itself → shorten_path = last component.
        assert_eq!(tree.groups[0].name, "grove-nogit-xyz");
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
        // Kind is known from the command, but there is no live state signal, so
        // it must be Unknown — never a stuck Active.
        assert_eq!(agent.state, AgentState::Unknown);
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
            make_pane("%1", "main", 0, "/opt/a", "zsh"),
            make_pane("%2", "main", 1, "/opt/a", "zsh"),
            make_pane("%3", "work", 0, "/opt/b", "zsh"),
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
            make_pane("%1", "main", 0, "/opt/a", "zsh"),
            make_pane("%2", "main", 1, "/opt/a", "zsh"),
            make_pane("%3", "work", 0, "/opt/b", "zsh"),
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
            make_pane("%1", "main", 0, "/opt/a", "zsh"),
            make_pane("%2", "work", 0, "/opt/b", "zsh"),
        ];
        let states = HashMap::new();
        let mut tree = TreeState::build(&panes, &states, "");

        // Collapse first group
        tree.toggle_expand();
        assert!(!tree.groups[0].expanded);

        // Rebuild with same data
        tree.rebuild(&panes, &states, &HashMap::new(), &HashSet::new(), None, "");

        // First group should still be collapsed
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
        // All panes in the same directory → one group, sorted by session/window
        let panes = vec![
            make_pane("%3", "work", 2, "/opt/src", "zsh"),
            make_pane("%1", "main", 0, "/opt/src", "zsh"),
            make_pane("%2", "main", 1, "/opt/src", "zsh"),
        ];
        let states = HashMap::new();
        let tree = TreeState::build(&panes, &states, "");

        assert_eq!(tree.groups.len(), 1);
        let group = &tree.groups[0];
        assert_eq!(group.panes[0].pane_info.pane_id, "%1"); // main win 0
        assert_eq!(group.panes[1].pane_info.pane_id, "%2"); // main win 1
        assert_eq!(group.panes[2].pane_info.pane_id, "%3"); // work win 2
    }

    fn pane_with_activity(
        id: &str,
        session: &str,
        path: &str,
        cmd: &str,
        activity: u64,
    ) -> PaneInfo {
        let mut p = make_pane(id, session, 0, path, cmd);
        p.activity = activity;
        p
    }

    #[test]
    fn test_panes_idle_ordered_by_latest_stop() {
        // Three claude panes in one group: two idle (Stop hook fired), one
        // active. Idle panes float above the active one, newest stop (highest
        // activity) first — even though the active pane has higher activity.
        let panes = vec![
            pane_with_activity("%active", "main", "/opt/x", "claude", 9999),
            pane_with_activity("%old", "main", "/opt/x", "claude", 1000),
            pane_with_activity("%new", "main", "/opt/x", "claude", 2000),
        ];
        let mut states = HashMap::new();
        states.insert("%active".to_string(), AgentState::Active);
        states.insert("%old".to_string(), AgentState::Idle);
        states.insert("%new".to_string(), AgentState::Idle);

        let recorded = HashMap::new();
        let groups = build_groups(
            &panes,
            &states,
            &recorded,
            &HashSet::new(),
            Some("main"),
            "",
            &[],
        );

        let g = &groups[0];
        assert_eq!(g.panes[0].pane_info.pane_id, "%new"); // newest stop
        assert_eq!(g.panes[1].pane_info.pane_id, "%old"); // older stop
        assert_eq!(g.panes[2].pane_info.pane_id, "%active"); // still working
    }

    #[test]
    fn test_groups_waiting_first() {
        // Group A: claude with no waiting state, high activity
        // Group B: claude in waiting state, lower activity
        // Waiting should win despite lower activity.
        let panes = vec![
            pane_with_activity("%1", "main", "/opt/a", "claude", 1000),
            pane_with_activity("%2", "main", "/opt/b", "claude", 10),
        ];
        let mut states = HashMap::new();
        states.insert("%2".to_string(), AgentState::Waiting);

        let recorded = HashMap::new();
        let groups = build_groups(&panes, &states, &recorded, &HashSet::new(), None, "", &[]);

        assert_eq!(groups[0].panes[0].pane_info.pane_id, "%2");
        assert_eq!(groups[1].panes[0].pane_info.pane_id, "%1");
    }

    #[test]
    fn test_groups_current_session_beats_activity() {
        // Two groups, no waiting. Group with pane in "current" session wins
        // even if the other group has higher activity.
        let panes = vec![
            pane_with_activity("%1", "other", "/opt/a", "zsh", 9999),
            pane_with_activity("%2", "main", "/opt/b", "zsh", 1),
        ];
        let states = HashMap::new();
        let recorded = HashMap::new();
        let groups = build_groups(
            &panes,
            &states,
            &recorded,
            &HashSet::new(),
            Some("main"),
            "",
            &[],
        );

        assert_eq!(groups[0].panes[0].pane_info.pane_id, "%2");
        assert_eq!(groups[1].panes[0].pane_info.pane_id, "%1");
    }

    #[test]
    fn test_groups_waiting_beats_current_session() {
        // Waiting in non-current session still outranks current-session group.
        let panes = vec![
            pane_with_activity("%1", "main", "/opt/a", "claude", 5000),
            pane_with_activity("%2", "other", "/opt/b", "claude", 1),
        ];
        let mut states = HashMap::new();
        states.insert("%2".to_string(), AgentState::Waiting);

        let recorded = HashMap::new();
        let groups = build_groups(
            &panes,
            &states,
            &recorded,
            &HashSet::new(),
            Some("main"),
            "",
            &[],
        );

        assert_eq!(groups[0].panes[0].pane_info.pane_id, "%2"); // waiting
        assert_eq!(groups[1].panes[0].pane_info.pane_id, "%1"); // current session
    }

    #[test]
    fn test_panes_within_group_waiting_first() {
        // All panes in same group: waiting pane should float to top regardless
        // of activity or current-session membership.
        let panes = vec![
            pane_with_activity("%1", "main", "/opt/x", "claude", 5000),
            pane_with_activity("%2", "main", "/opt/x", "claude", 10),
            pane_with_activity("%3", "main", "/opt/x", "claude", 100),
        ];
        let mut states = HashMap::new();
        states.insert("%2".to_string(), AgentState::Waiting);

        let recorded = HashMap::new();
        let groups = build_groups(
            &panes,
            &states,
            &recorded,
            &HashSet::new(),
            Some("main"),
            "",
            &[],
        );

        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.panes[0].pane_info.pane_id, "%2"); // waiting
        assert_eq!(g.panes[1].pane_info.pane_id, "%1"); // higher activity
        assert_eq!(g.panes[2].pane_info.pane_id, "%3"); // lower activity
    }

    #[test]
    fn test_panes_within_group_current_session_beats_activity() {
        // Within one group, panes in the current session beat higher-activity
        // panes in other sessions.
        let panes = vec![
            pane_with_activity("%1", "other", "/opt/x", "zsh", 9999),
            pane_with_activity("%2", "main", "/opt/x", "zsh", 1),
        ];
        let states = HashMap::new();
        let recorded = HashMap::new();
        let groups = build_groups(
            &panes,
            &states,
            &recorded,
            &HashSet::new(),
            Some("main"),
            "",
            &[],
        );

        assert_eq!(groups[0].panes[0].pane_info.pane_id, "%2");
        assert_eq!(groups[0].panes[1].pane_info.pane_id, "%1");
    }

    // ── Step 10: shared FlatRows equivalence pins ───────────────────────────

    /// Independent reference walk: the flat list of (is_header, pane_id) rows in
    /// the same order the real walks visit them. The oracle for the equivalence
    /// properties below.
    fn reference_rows(tree: &TreeState) -> Vec<Option<String>> {
        let mut rows = Vec::new();
        for group in &tree.groups {
            rows.push(None); // group header
            if group.expanded {
                for pane in &group.panes {
                    rows.push(Some(pane.pane_info.pane_id.clone()));
                }
            }
        }
        rows
    }

    fn three_group_tree() -> TreeState {
        let panes = vec![
            make_pane("%1", "main", 0, "/opt/a", "zsh"),
            make_pane("%2", "main", 1, "/opt/a", "zsh"),
            make_pane("%3", "work", 0, "/opt/b", "zsh"),
            make_pane("%4", "work", 1, "/opt/b", "zsh"),
            make_pane("%5", "dev", 0, "/opt/c", "zsh"),
        ];
        let states = HashMap::new();
        TreeState::build(&panes, &states, "")
    }

    /// TREE-visiblecount-eq-rows-property (P1 / Step 10): visible_count() must
    /// equal the reference row walk across every expand/collapse combination.
    #[test]
    fn tree_visiblecount_eq_rows_property() {
        let mut tree = three_group_tree();
        let n = tree.groups.len();
        // Drive every combination of group expand/collapse.
        for mask in 0..(1u32 << n) {
            for (i, group) in tree.groups.iter_mut().enumerate() {
                group.expanded = (mask >> i) & 1 == 1;
            }
            assert_eq!(
                tree.visible_count(),
                reference_rows(&tree).len(),
                "visible_count disagrees with reference walk for mask {mask}"
            );
        }
    }

    /// TREE-selected-pane-agrees-every-cursor (P1 / Step 10): selected_pane_id()
    /// must match the reference walk (header → None, pane → its id) for every
    /// cursor and every expand configuration.
    #[test]
    fn tree_selected_pane_agrees_every_cursor() {
        let mut tree = three_group_tree();
        let n = tree.groups.len();
        for mask in 0..(1u32 << n) {
            for (i, group) in tree.groups.iter_mut().enumerate() {
                group.expanded = (mask >> i) & 1 == 1;
            }
            let oracle = reference_rows(&tree);
            for (c, expected) in oracle.iter().enumerate() {
                tree.cursor = c;
                let got = tree.selected_pane_id().map(|s| s.to_string());
                assert_eq!(
                    &got, expected,
                    "selected_pane_id mismatch at cursor {c}, mask {mask}"
                );
            }
        }
    }

    // ── S7: scroll/cursor desync under search (seam S-displayrows) ──────────

    /// SCROLL-deep-cursor-filtered-not-blank: group A with many panes + group B
    /// with a matching pane at a deep absolute row. Search matches only the B
    /// pane; cursor on that match. After clamp_scroll the match's display index
    /// must be inside the visible window and scroll <= display_rows().len().
    #[test]
    fn scroll_deep_cursor_filtered_not_blank() {
        let mut panes = Vec::new();
        for i in 0..30 {
            panes.push(make_pane(&format!("%a{i}"), "main", i, "/opt/aaa", "zsh"));
        }
        // Distinctive command only on the B pane so the filter isolates it.
        panes.push(make_pane("%bmatch", "work", 0, "/opt/bbb", "neovimxyz"));
        let states = HashMap::new();
        let mut tree = TreeState::build(&panes, &states, "");
        tree.search_filter = Some("neovimxyz".to_string());

        // Put the cursor on the matching pane via its absolute position.
        let match_cursor = tree.pane_cursor_position("%bmatch").unwrap();
        tree.cursor = match_cursor;
        // Adversarial: a stale deep offset (cursor-space) that a no-op clamp
        // would leave blank past the compacted display.
        tree.scroll_offset = match_cursor;

        let height = 10;
        tree.clamp_scroll(height);

        let rows = tree.display_rows();
        let match_idx = tree.cursor_display_index().expect("cursor maps to a row");
        let start = tree.scroll_offset;
        assert!(
            start <= rows.len(),
            "scroll {start} past rows {}",
            rows.len()
        );
        assert!(
            start <= match_idx && match_idx < start + height,
            "match display idx {match_idx} not in window [{start}, {})",
            start + height
        );
    }

    /// SCROLL-invariant-pin (property): for several (filter, expand mask, cursor)
    /// combinations, after clamp_scroll the offset never exceeds display rows and
    /// the cursor maps to a real display row (or there are zero display rows).
    /// Expressed purely via the public seam so it survives the S17 refactor.
    #[test]
    fn scroll_invariant_pin() {
        let filters = [None, Some("zsh".to_string()), Some("nomatchqz".to_string())];
        for filter in filters {
            let mut tree = three_group_tree();
            tree.search_filter = filter.clone();
            let n = tree.groups.len();
            for mask in 0..(1u32 << n) {
                for (i, group) in tree.groups.iter_mut().enumerate() {
                    group.expanded = (mask >> i) & 1 == 1;
                }
                let total = tree.visible_count();
                for cursor in 0..=total {
                    tree.cursor = cursor;
                    tree.scroll_offset = cursor; // adversarial starting offset
                    for height in [0usize, 1, 5] {
                        tree.clamp_scroll(height);
                        let rows = tree.display_rows();
                        assert!(
                            tree.scroll_offset <= rows.len(),
                            "offset {} past rows {} (filter {:?}, mask {mask}, cursor {cursor}, h {height})",
                            tree.scroll_offset,
                            rows.len(),
                            filter
                        );
                        if let Some(idx) = tree.cursor_display_index() {
                            assert!(idx < rows.len(), "cursor display idx out of range");
                        } else {
                            // cursor not on a visible row is acceptable; nothing to pin.
                        }
                    }
                }
            }
        }
    }

    // ── S17: one canonical row iterator equivalence ─────────────────────────

    /// ROWS-iterator-equiv-old-walks (P1 / S17): the rows()-based walks
    /// (visible_count, selected_pane_id, selected_group, pane_positions) must
    /// agree with an independent inline oracle for every cursor across every
    /// expand mask, both unfiltered and under a search filter. Pins that rows()
    /// is the single source of traversal truth.
    #[test]
    fn rows_iterator_equiv_old_walks() {
        let filters = [None, Some("zsh".to_string()), Some("nomatchqz".to_string())];
        for filter in filters {
            let mut tree = three_group_tree();
            tree.search_filter = filter.clone();
            let n = tree.groups.len();
            for mask in 0..(1u32 << n) {
                for (i, group) in tree.groups.iter_mut().enumerate() {
                    group.expanded = (mask >> i) & 1 == 1;
                }

                // Oracle: rebuild the unfiltered flat walk inline. Each entry:
                // (group_idx, Option<pane_idx>) — None pane_idx means header.
                let mut oracle: Vec<(usize, Option<usize>)> = Vec::new();
                for (gi, group) in tree.groups.iter().enumerate() {
                    oracle.push((gi, None));
                    if group.expanded {
                        for pi in 0..group.panes.len() {
                            oracle.push((gi, Some(pi)));
                        }
                    }
                }

                // visible_count == unfiltered row count.
                assert_eq!(
                    tree.visible_count(),
                    oracle.len(),
                    "visible_count, mask {mask}"
                );

                // pane_positions: absolute indices of matching pane rows.
                let expected_positions: Vec<usize> = oracle
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, (gi, pi))| {
                        let pi = (*pi)?;
                        let g = &tree.groups[*gi];
                        tree.pane_matches(&g.panes[pi], &g.name).then_some(idx)
                    })
                    .collect();
                assert_eq!(
                    tree.pane_positions(),
                    expected_positions,
                    "pane_positions, filter {filter:?}, mask {mask}"
                );

                // Per-cursor: selected_pane_id and selected_group.
                for (c, (gi, pi)) in oracle.iter().enumerate() {
                    tree.cursor = c;
                    let exp_pane =
                        pi.map(|pi| tree.groups[*gi].panes[pi].pane_info.pane_id.clone());
                    assert_eq!(
                        tree.selected_pane_id().map(str::to_string),
                        exp_pane,
                        "selected_pane_id cursor {c}, mask {mask}"
                    );
                    let exp_group = pi.is_none().then(|| tree.groups[*gi].name.clone());
                    assert_eq!(
                        tree.selected_group().map(|g| g.name.clone()),
                        exp_group,
                        "selected_group cursor {c}, mask {mask}"
                    );
                }
            }
        }
    }

    // ── S8: selection preserved across rebuild reorder ──────────────────────

    /// REBUILD-keeps-selected-across-reorder: select pane X, then rebuild with an
    /// agent_states change that floats another pane above X. The selection must
    /// stay on X even though its absolute row moved.
    #[test]
    fn rebuild_keeps_selected_across_reorder() {
        let panes = vec![
            pane_with_activity("%x", "main", "/opt/x", "claude", 100),
            pane_with_activity("%y", "main", "/opt/x", "claude", 50),
        ];
        let mut states = HashMap::new();
        states.insert("%x".to_string(), AgentState::Active);
        states.insert("%y".to_string(), AgentState::Active);
        let mut tree = TreeState::build(&panes, &states, "");

        // Select pane %x (initially first pane: header at 0, %x at 1).
        tree.cursor = tree.pane_cursor_position("%x").unwrap();
        assert_eq!(tree.selected_pane_id(), Some("%x"));

        // Rebuild: %y now needs attention (Idle) so it floats above %x.
        states.insert("%y".to_string(), AgentState::Idle);
        tree.rebuild(
            &panes,
            &states,
            &HashMap::new(),
            &HashSet::new(),
            Some("main"),
            "",
        );

        // Order changed (%y floated up) but selection must still be %x.
        assert_eq!(tree.groups[0].panes[0].pane_info.pane_id, "%y");
        assert_eq!(tree.selected_pane_id(), Some("%x"));
    }

    // ── S22 / seam S-rootresolver: workspace detection + grouping ───────────

    /// Make `dir/<name>/.git` so `name` looks like a git repo child.
    fn make_git_child(dir: &Path, name: &str) {
        let child = dir.join(name);
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join(".git"), "gitdir: x").unwrap();
    }

    /// WORKSPACE-2plus-children-true: 2+ child dirs each with a `.git` => true.
    #[test]
    fn workspace_2plus_children_true() {
        let tmp = tempfile::tempdir().unwrap();
        make_git_child(tmp.path(), "repo-a");
        make_git_child(tmp.path(), "repo-b");
        assert!(is_workspace(tmp.path()));
    }

    /// WORKSPACE-one-child-false: only 1 child with .git => false.
    #[test]
    fn workspace_one_child_false() {
        let tmp = tempfile::tempdir().unwrap();
        make_git_child(tmp.path(), "repo-a");
        assert!(!is_workspace(tmp.path()));
    }

    /// WORKSPACE-own-git-false: dir that itself has .git => false even with
    /// git children.
    #[test]
    fn workspace_own_git_false() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".git"), "gitdir: x").unwrap();
        make_git_child(tmp.path(), "repo-a");
        make_git_child(tmp.path(), "repo-b");
        assert!(!is_workspace(tmp.path()));
    }

    /// WORKSPACE-unreadable-false: nonexistent dir => false.
    #[test]
    fn workspace_unreadable_false() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        assert!(!is_workspace(&missing));
    }

    /// WORKSPACE-siblings-collapse-one-group: two panes whose paths resolve (via
    /// the injected resolver) to sibling repos under a common workspace parent
    /// collapse into ONE group rooted at the workspace. Drives the seam without
    /// invoking real git.
    #[test]
    fn workspace_siblings_collapse_one_group() {
        // Real tempdir workspace with two git-child repos so is_workspace(parent)
        // returns true; the injected resolver maps each pane to its sibling repo.
        let tmp = tempfile::tempdir().unwrap();
        make_git_child(tmp.path(), "repo-a");
        make_git_child(tmp.path(), "repo-b");
        let ws = std::fs::canonicalize(tmp.path()).unwrap();
        let repo_a = ws.join("repo-a");
        let repo_b = ws.join("repo-b");

        let panes = vec![
            make_pane("%1", "main", 0, repo_a.to_str().unwrap(), "claude"),
            make_pane("%2", "work", 0, repo_b.to_str().unwrap(), "claude"),
        ];
        let states = HashMap::new();
        let recorded = HashMap::new();

        // Injected resolver: mimic resolve_project_root's climb — each repo's
        // git root's parent is the workspace, so both collapse to `ws`.
        let resolver = |p: &Path| {
            let git_root = p.to_path_buf();
            match git_root.parent() {
                Some(parent) if is_workspace(parent) => parent.to_path_buf(),
                _ => git_root,
            }
        };

        let groups = build_groups_with_resolver(
            &panes,
            &states,
            &recorded,
            &HashSet::new(),
            None,
            "",
            &[],
            resolver,
        );

        assert_eq!(groups.len(), 1, "siblings must collapse into one group");
        assert_eq!(groups[0].path, ws, "group rooted at the workspace parent");
        assert_eq!(groups[0].panes.len(), 2);
    }
}
