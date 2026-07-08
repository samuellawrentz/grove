use std::time::{Duration, Instant};

use edtui::{EditorState, Lines};

use crate::agent::AgentFilter;
use crate::config::GroveConfig;
use crate::db::{Db, Project};
use crate::error::GroveError;
use crate::tmux;

use super::source::{self, DiffState};
use super::tree::TreeState;

const TREE_POLL: Duration = Duration::from_secs(5);

/// Number of trailing scrollback lines to capture for the preview viewport.
/// Bounds the per-tick capture rather than dumping full history.
const PREVIEW_LINES: usize = 200;

/// Whether the selected pane's preview should be re-captured. True on a fresh
/// selection (`last` is None) or when its activity signal changed since the last
/// capture; false when activity is unchanged (idle pane → skip the tick capture).
pub(crate) fn should_recapture_preview(sel_activity: Option<u64>, last: Option<u64>) -> bool {
    match (sel_activity, last) {
        (Some(cur), Some(prev)) => cur != prev,
        _ => true,
    }
}

/// Build a bounded `capture-pane` argument vector: `-S -N` (last N lines) rather
/// than `-S -` (full history), keeping the per-tick capture cheap.
pub(crate) fn capture_args(pane_id: &str, n: usize) -> Vec<String> {
    vec![
        "capture-pane".into(),
        "-t".into(),
        pane_id.into(),
        "-p".into(),
        "-e".into(),
        "-S".into(),
        format!("-{n}"),
        "-E".into(),
        "-".into(),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarFocus {
    Tree,
    Projects,
}

/// Which panel has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Sidebar,
    Notepad,
}

/// Current input-capture overlay. Mutually exclusive by construction.
pub(crate) enum Overlay {
    None,
    Search { query: String, target: SidebarFocus },
    Prompt(String),
    OpenChoice { dir: String },
}

/// A one-shot deferred shell side-effect to run after the next draw.
pub(crate) enum PendingShell {
    None,
    Popup(String),
    FzfPicker,
}

/// Preview-pane state: the captured content plus the dedup tokens that gate
/// per-tick re-capture (S14/S15) and diff-mode rendering.
pub(crate) struct PreviewState {
    pub content: String,
    pub scroll_up: u16,
    pub diff_mode: bool,
    pub diff_state: Option<DiffState>,
    /// (pane_id, activity) last captured into `content`. Used to skip
    /// re-capturing an idle pane every tick; the pane_id guards against an
    /// activity collision when the selection moves to a different pane.
    pub last_activity: Option<u64>,
    pub last_pane: Option<String>,
    /// Last diff dirty-token (per target dir); skips the per-tick `git diff`
    /// reparse when HEAD+index are unchanged since the last fetch.
    pub last_diff_token: Option<String>,
}

/// Projects-list state: the loaded list, its cursor/search, and the
/// DB-error marker that distinguishes a load failure from an empty list.
pub(crate) struct ProjectsState {
    pub list: Vec<Project>,
    pub cursor: usize,
    pub search_filter: Option<String>,
    /// Set when `list_projects` errors, distinguishing a DB failure from a
    /// genuinely empty project list.
    pub error: bool,
}

/// UI-chrome / focus / transient-status state.
pub(crate) struct UiState {
    pub overlay: Overlay,
    pub status_message: Option<String>,
    pub focus: Focus,
    pub sidebar_focus: SidebarFocus,
    pub show_notepad: bool,
    pub last_interaction: Instant,
    /// Sticky: set when a tree refresh fails, cleared only by a successful
    /// refresh. Unlike `status_message`, a keypress does not clear it, so the
    /// "data is stale" signal survives until data is genuinely refreshed.
    pub data_stale: bool,
}

/// Main application state for the TUI.
pub(crate) struct App {
    pub tree: TreeState,
    pub preview: PreviewState,
    pub projects: ProjectsState,
    pub ui: UiState,
    pub should_quit: bool,
    pub verbose: bool,
    pub my_pane_id: String,
    pub pending_shell: PendingShell,
    pub default_agent_command: String,
    pub config: GroveConfig,
    pub db: Db,
    /// When true, quit after launching a pane (popup mode).
    pub popup: bool,
    pub notepad: NoteState,
}

pub(crate) struct NoteState {
    pub editor: EditorState,
    pub project: String,
}

impl App {
    /// Resolve the TUI's own pane ID from the environment, falling back to a
    /// tmux query. Kept separate from `construct` so it's the only live-tmux
    /// touch before the first frame.
    pub(crate) fn resolve_my_pane_id(verbose: bool) -> String {
        let my_pane_id = std::env::var("TMUX_PANE").unwrap_or_default();
        if my_pane_id.is_empty() {
            tmux::get_pane_id("", verbose).unwrap_or_default()
        } else {
            my_pane_id
        }
    }

    /// Cheap construction: build from parts and load the projects list, with NO
    /// tmux/preview fetch. The run loop paints one frame from this state before
    /// the blocking `initial_refresh`, so startup is not perceived as a hang.
    pub(crate) fn construct(
        config: GroveConfig,
        db: Db,
        my_pane_id: String,
        verbose: bool,
        popup: bool,
    ) -> Self {
        let mut app = Self::with_parts(config, db, my_pane_id, verbose, popup);
        app.load_projects();
        app
    }

    /// The first blocking refresh, run after the initial frame is drawn.
    pub(crate) fn initial_refresh(&mut self) {
        self.refresh_tree();
        // In a popup, my_pane_id is the originating pane — a real pane the user
        // launched from; default the cursor onto it. Otherwise fall back to the
        // first pane (and in non-popup mode it's excluded entirely anyway).
        let my = self.my_pane_id.clone();
        if !(self.popup && self.tree.jump_to_pane(&my)) {
            self.tree.jump_first_pane();
        }
        self.refresh_preview();
        self.sync_note_to_group();
    }

    /// Construct an App from explicit parts without touching tmux or the
    /// network. The headless seam used by tests (and by `new`, which then
    /// performs the live refresh).
    pub(crate) fn with_parts(
        config: GroveConfig,
        db: Db,
        my_pane_id: String,
        verbose: bool,
        popup: bool,
    ) -> Self {
        let default_agent_command = config.claude_command.clone();
        App {
            tree: TreeState {
                groups: Vec::new(),
                cursor: 0,
                scroll_offset: 0,
                search_filter: None,
                agent_filter: AgentFilter::AnyAgent,
            },
            preview: PreviewState {
                content: String::new(),
                scroll_up: 0,
                diff_mode: false,
                diff_state: None,
                last_activity: None,
                last_pane: None,
                last_diff_token: None,
            },
            projects: ProjectsState {
                list: Vec::new(),
                cursor: 0,
                search_filter: None,
                error: false,
            },
            ui: UiState {
                overlay: Overlay::None,
                status_message: None,
                focus: Focus::Sidebar,
                sidebar_focus: SidebarFocus::Tree,
                show_notepad: false,
                last_interaction: Instant::now(),
                data_stale: false,
            },
            should_quit: false,
            verbose,
            my_pane_id,
            pending_shell: PendingShell::None,
            default_agent_command,
            config,
            db,
            popup,
            notepad: NoteState {
                editor: EditorState::default(),
                project: String::new(),
            },
        }
    }

    /// Whether an input-capture overlay is active. Background refresh
    /// (`on_tick`/SIGUSR1) is gated on this so a tmux hook firing mid-overlay
    /// can't rebuild the tree and yank the cursor out from under the user.
    pub(crate) fn overlay_active(&self) -> bool {
        !matches!(self.ui.overlay, Overlay::None)
    }

    /// Whether a tmux-hook/tick driven refresh may run now. The notepad is a
    /// `Focus` rather than an `Overlay`, so it also captures input and must
    /// suppress background rebuilds that would yank focus mid-typing.
    pub(crate) fn should_background_refresh(&self) -> bool {
        !self.overlay_active() && self.ui.focus != Focus::Notepad
    }

    /// Refresh tree data from tmux and claude state.
    pub fn refresh_tree(&mut self) {
        let result = match (
            source::fetch_panes(self.verbose),
            source::fetch_agent_states(),
        ) {
            (Ok(panes), Ok(states)) => Ok((panes, states)),
            (Err(e), _) | (_, Err(e)) => Err(e),
        };
        self.apply_refresh(result);
    }

    /// Apply a tree-refresh result. On success, rebuild the tree and clear the
    /// stale marker; on failure, set the (sticky) stale marker and a status
    /// message. Split from `refresh_tree` so the flag logic is unit-testable.
    pub(crate) fn apply_refresh(
        &mut self,
        result: Result<
            (
                Vec<crate::tmux::PaneInfo>,
                std::collections::HashMap<String, crate::agent::AgentState>,
            ),
            GroveError,
        >,
    ) {
        match result {
            Ok((panes, states)) => {
                let live: std::collections::HashSet<String> =
                    panes.iter().map(|p| p.pane_id.clone()).collect();
                let recorded = self.gc_and_read_kinds(&live);
                let marked = self.db.list_pane_overrides().unwrap_or_else(|e| {
                    if self.verbose {
                        eprintln!("Warning: list_pane_overrides failed: {e}");
                    }
                    std::collections::HashSet::new()
                });
                let current_session = crate::tmux::current_session_cached(self.verbose);
                let old_group_count = self.tree.groups.len();
                // Exclude the TUI's own pane so it never lists itself (D7). In a
                // popup the TUI lives in its own ephemeral pane, so my_pane_id is
                // the originating pane — keep it listed (and selected by default).
                let my_pane_id = if self.popup {
                    String::new()
                } else {
                    self.my_pane_id.clone()
                };
                self.tree.rebuild(
                    &panes,
                    &states,
                    &recorded,
                    &marked,
                    current_session.as_deref(),
                    &my_pane_id,
                );
                self.ui.status_message = None;
                self.ui.data_stale = false;
                self.upsert_projects_if_groups_changed(old_group_count);
            }
            Err(e) => {
                self.ui.status_message = Some(format!("Refresh error: {e}"));
                self.ui.data_stale = true;
            }
        }
    }

    /// GC the pane_agents rows + process-tree cache against the live pane set,
    /// then read back the surviving recorded kinds, merging in any kinds the
    /// agents declared in the shared state file. DB-recorded kinds (set at
    /// launch) are authoritative; state-file kinds are the fallback.
    fn gc_and_read_kinds(
        &self,
        live: &std::collections::HashSet<String>,
    ) -> std::collections::HashMap<String, crate::agent::AgentKind> {
        // GC dead rows and read back the survivors in a single SELECT, so dead
        // rows can't resurrect a stale agent.
        let mut recorded = crate::agent::PaneAgentStore::new(&self.db)
            .gc_returning(live)
            .unwrap_or_else(|e| {
                if self.verbose {
                    eprintln!("Warning: pane_agents GC failed: {e}");
                }
                std::collections::HashMap::new()
            });
        // GC the bounded process-tree cache against the same live set.
        crate::agent::gc_process_tree_cache(live);
        for (id, kind) in source::fetch_state_kinds() {
            recorded.entry(id).or_insert(kind);
        }
        recorded
    }

    /// Upsert each group's path as a project, but only when the group count
    /// changed since the last rebuild (avoids a DB write every 5s tick).
    fn upsert_projects_if_groups_changed(&self, old_group_count: usize) {
        if self.tree.groups.len() != old_group_count {
            for group in &self.tree.groups {
                let _ = self.db.upsert_project(&group.path.to_string_lossy());
            }
        }
    }

    /// Refresh preview content for the selected pane.
    pub fn refresh_preview(&mut self) {
        if self.preview.diff_mode {
            let dir = self
                .tree
                .selected_group()
                .map(|g| g.path.clone())
                .or_else(|| {
                    self.tree
                        .selected_pane()
                        .map(|p| p.pane_info.current_path.clone())
                });
            if let Some(path) = dir {
                // Skip the git diff + reparse when HEAD+index are unchanged since
                // the last fetch for this target (and we already have state).
                let token = source::diff_dirty(&path);
                if self.preview.diff_state.is_some()
                    && !source::diff_token_changed(
                        token.as_ref(),
                        self.preview.last_diff_token.as_ref(),
                    )
                {
                    return;
                }
                match source::fetch_git_diffs(&path) {
                    Ok(repos) => {
                        if let Some(ref mut ds) = self.preview.diff_state {
                            ds.update(repos);
                        } else {
                            self.preview.diff_state = Some(DiffState::new(repos));
                        }
                        self.preview.last_diff_token = token;
                    }
                    Err(e) => self.ui.status_message = Some(format!("Git diff error: {e}")),
                }
            }
            return;
        }
        if let Some((pane_id, activity)) = self
            .tree
            .selected_pane()
            .map(|p| (p.pane_info.pane_id.clone(), p.pane_info.activity))
        {
            // Skip the re-capture when the same pane's activity is unchanged
            // since the last capture (idle pane → no tmux call). A selection
            // change to a different pane always re-captures.
            let same_pane = self.preview.last_pane.as_deref() == Some(pane_id.as_str());
            let prev = same_pane.then_some(self.preview.last_activity).flatten();
            if same_pane && !should_recapture_preview(Some(activity), prev) {
                return;
            }
            match source::fetch_preview(&pane_id, PREVIEW_LINES, self.verbose) {
                Ok(content) => {
                    self.preview.content = content;
                    self.preview.last_activity = Some(activity);
                    self.preview.last_pane = Some(pane_id);
                }
                Err(e) => {
                    self.ui.status_message = Some(format!("Preview error: {e}"));
                }
            }
        } else if let Some(group) = self.tree.selected_group() {
            self.preview.last_activity = None;
            let path = group.path.clone();
            match source::fetch_directory_listing(&path) {
                Ok(listing) => {
                    self.preview.content = listing;
                }
                Err(e) => {
                    self.ui.status_message = Some(format!("Directory error: {e}"));
                }
            }
        }
    }

    /// Get the poll timeout.
    pub fn poll_timeout(&self) -> Duration {
        TREE_POLL
    }

    /// Refresh the projects list from the database.
    pub fn refresh_projects(&mut self) {
        self.load_projects();
        if self.projects.cursor >= self.projects.list.len() {
            self.projects.cursor = self.projects.list.len().saturating_sub(1);
        }
    }

    /// Load projects from the DB, recording a marker on failure so a DB error
    /// is distinguishable from a genuinely empty project list.
    pub(crate) fn load_projects(&mut self) {
        let result = self.db.list_projects();
        self.apply_projects(result);
    }

    /// Apply a `list_projects` result: on Ok replace the list and clear the
    /// error marker; on Err set `projects_error` and leave the list untouched.
    /// Split out so the empty-vs-error logic is unit-testable.
    pub(crate) fn apply_projects(&mut self, result: Result<Vec<Project>, GroveError>) {
        match result {
            Ok(projects) => {
                self.projects.list = projects;
                self.projects.error = false;
            }
            Err(_) => self.projects.error = true,
        }
    }

    /// Called on each tick (timeout expiry) to refresh data.
    pub fn on_tick(&mut self) {
        self.refresh_tree();
        self.refresh_projects();
        self.refresh_preview();
    }

    pub fn sync_note_to_group(&mut self) {
        let current_path = self
            .tree
            .cursor_group()
            .map(|g| g.path.to_string_lossy().to_string());
        let Some(path) = current_path else {
            return;
        };
        if path != self.notepad.project {
            self.save_note();
            self.notepad.editor = self.load_note(&path);
            self.notepad.project = path;
        }
    }

    fn load_note(&self, path: &str) -> EditorState {
        let content = match self.db.get_note(path) {
            Ok(Some(c)) => c,
            Ok(None) => String::new(),
            Err(e) => {
                eprintln!("Note load error: {e}");
                String::new()
            }
        };
        EditorState::new(Lines::from(content.as_str()))
    }

    /// Get filtered projects list indices matching the current search filter.
    pub fn filtered_project_indices(&self) -> Vec<usize> {
        match &self.projects.search_filter {
            Some(query) if !query.is_empty() => self
                .projects
                .list
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    let q = query.to_lowercase();
                    p.name.to_lowercase().contains(&q)
                        || p.path.to_string_lossy().to_lowercase().contains(&q)
                })
                .map(|(i, _)| i)
                .collect(),
            _ => (0..self.projects.list.len()).collect(),
        }
    }

    pub fn save_note(&mut self) {
        if self.notepad.project.is_empty() {
            return;
        }
        let content = self.notepad.editor.lines.to_string();
        if let Err(e) = self.db.save_note(&self.notepad.project, &content) {
            self.ui.status_message = Some(format!("Failed to save note: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> Db {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_path_buf();
        std::mem::forget(f);
        Db::open_path(&path).unwrap()
    }

    /// TUI-config-flag-honored (P0 / A2): the App carries the config it was
    /// constructed with (which `main` resolved from `--config`), not a reloaded
    /// default. Pre-refactor `App::new` called `GroveConfig::load(None,..)`,
    /// silently discarding `--config`.
    #[test]
    fn config_flag_honored() {
        let config = GroveConfig {
            claude_command: "claude --distinctive-flag".to_string(),
            ..Default::default()
        };
        let app = App::with_parts(config, temp_db(), String::new(), false, false);
        assert_eq!(app.config.claude_command, "claude --distinctive-flag");
        assert_eq!(app.default_agent_command, "claude --distinctive-flag");
    }

    /// TUI-refresh-gated-under-modal (P1 / sigusr1-tick-modal): the event loop
    /// gates `on_tick`/SIGUSR1 `refresh_tree` on `!overlay_active()`, so a tmux
    /// hook firing mid-search can't rebuild the tree and reset the cursor under
    /// the user's typing. Pin the predicate the gate relies on.
    #[test]
    fn refresh_gated_under_modal() {
        let mut app = App::with_parts(
            GroveConfig::default(),
            temp_db(),
            String::new(),
            false,
            false,
        );

        // No overlay: background refresh is allowed.
        assert!(!app.overlay_active());

        // Entering search captures input → background refresh is suppressed.
        app.ui.overlay = Overlay::Search {
            query: "x".to_string(),
            target: SidebarFocus::Tree,
        };
        assert!(app.overlay_active());

        // Closing the overlay re-enables background refresh.
        app.ui.overlay = Overlay::None;
        assert!(!app.overlay_active());
    }

    /// GATE-suppress-under-notepad (S9 / S-gatefn): the notepad is a `Focus`,
    /// not an `Overlay`, so `overlay_active()` alone won't suppress a background
    /// refresh while the user types in it. `should_background_refresh()` must
    /// additionally gate on `Focus::Notepad`, while still honoring the overlay.
    #[test]
    fn refresh_suppressed_under_notepad() {
        let mut app = test_app();

        // Notepad focus, no overlay: suppressed.
        app.ui.focus = Focus::Notepad;
        assert!(!app.should_background_refresh());

        // Sidebar focus, no overlay: allowed.
        app.ui.focus = Focus::Sidebar;
        assert!(app.should_background_refresh());

        // Overlay still gates even with sidebar focus.
        app.ui.overlay = Overlay::Prompt(String::new());
        assert!(!app.should_background_refresh());
    }

    fn test_app() -> App {
        App::with_parts(
            GroveConfig::default(),
            temp_db(),
            String::new(),
            false,
            false,
        )
    }

    /// REFRESH-failure-sets-sticky-stale: a failed refresh marks data stale;
    /// only a subsequent successful refresh clears the flag.
    #[test]
    fn refresh_failure_sets_sticky_stale() {
        let mut app = test_app();
        app.apply_refresh(Err(GroveError::TmuxNotRunning("boom".to_string())));
        assert!(app.ui.data_stale);
        app.apply_refresh(Ok((Vec::new(), std::collections::HashMap::new())));
        assert!(!app.ui.data_stale);
    }

    /// REFRESH-keypress-preserves-stale: clearing status_message (what the
    /// keypress handler does) must NOT clear the sticky stale flag.
    #[test]
    fn refresh_keypress_preserves_stale() {
        let mut app = test_app();
        app.apply_refresh(Err(GroveError::TmuxNotRunning("boom".to_string())));
        assert!(app.ui.data_stale);
        assert!(app.ui.status_message.is_some());
        // Simulate the keypress clear path (actions::handle_key sets this).
        app.ui.status_message = None;
        assert!(app.ui.data_stale);
        assert!(app.ui.status_message.is_none());
    }

    /// PROJECTS-db-error-distinct-from-empty: a DB error sets projects_error
    /// while a genuinely empty (Ok) project list does not.
    #[test]
    fn projects_db_error_distinct_from_empty() {
        let mut app = test_app();
        app.apply_projects(Err(GroveError::Database("boom".to_string())));
        assert!(app.projects.error);

        let mut app = test_app();
        app.apply_projects(Ok(Vec::new()));
        assert!(!app.projects.error);
    }

    /// PREVIEW-skip-when-activity-unchanged (S14): `should_recapture_preview`
    /// is false when the selected pane's activity equals the last-captured one,
    /// true when it differs (so an idle pane is not re-captured every tick).
    #[test]
    fn preview_skip_when_activity_unchanged() {
        assert!(!super::should_recapture_preview(Some(42), Some(42)));
        assert!(super::should_recapture_preview(Some(43), Some(42)));
        // A fresh selection (no prior capture) must capture.
        assert!(super::should_recapture_preview(Some(1), None));
    }

    /// PREVIEW-capture-bounded (S14): `capture_args` bounds the capture to a
    /// viewport window via `-S -N` rather than dumping full scrollback (`-S -`).
    #[test]
    fn preview_capture_bounded() {
        let args = super::capture_args("%1", 40);
        assert!(args.iter().any(|a| a == "-S"));
        // The start is a bounded negative offset, not the full-history "-".
        let s_idx = args.iter().position(|a| a == "-S").unwrap();
        let start = &args[s_idx + 1];
        assert!(start.starts_with('-'));
        assert_ne!(start, "-");
        assert_eq!(start, "-40");
    }

    /// INIT-construct-valid-empty (S13): `construct` builds a usable App from
    /// parts + projects WITHOUT any tmux/preview fetch, so the run loop can paint
    /// a first frame before the blocking `initial_refresh`. Tree is empty, cursor
    /// at 0, no panic, and preview content is still empty (not fetched).
    #[test]
    fn init_construct_valid_empty() {
        let app = App::construct(
            GroveConfig::default(),
            temp_db(),
            String::new(),
            false,
            false,
        );
        assert!(app.tree.groups.is_empty());
        assert_eq!(app.tree.cursor, 0);
        assert!(app.preview.content.is_empty());
    }
}
