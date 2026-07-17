//! External-side-effect rollback for mutating commands.
//!
//! `StepJournal` owns the inverse of irreversible **fs/git/tmux** side-effects.
//! Each side-effect is registered as it succeeds; if the journal is dropped
//! without [`StepJournal::commit`], the registered undos run in reverse
//! (LIFO) order.
//!
//! **DB writes are deliberately NOT journaled.** They run last, inside a single
//! [`crate::db::Db::transaction`], which owns its own rollback. A `DELETE`
//! compensation for an `upsert_*` would re-introduce exactly the non-atomic
//! delete that the transaction helper removes — so the journal must never hold a
//! DB-inverse closure. CI gate **G1** enforces this.
//!
//! Caveat: `Drop` cannot return `Result`, so undo failures are best-effort
//! (log-only) — same observability as the hand-rolled rollback it replaces. The
//! journal buys *ordering and completeness*, not undo-failure safety.

use std::path::Path;

use crate::error::GroveError;
use crate::git;

/// Best-effort undo: log a failure (when verbose) but never propagate — `Drop`
/// cannot return `Result`, so rollback completeness is the guarantee, not
/// undo-failure safety.
fn log_undo(verbose: bool, what: &str, result: Result<(), GroveError>) {
    if let Err(e) = result {
        if verbose {
            eprintln!("[grove] rollback: failed to {what}: {e}");
        }
    }
}

pub struct StepJournal<'a> {
    undos: Vec<Box<dyn FnOnce() + 'a>>,
    verbose: bool,
    committed: bool,
}

impl<'a> StepJournal<'a> {
    pub fn new(verbose: bool) -> Self {
        Self {
            undos: Vec::new(),
            verbose,
            committed: false,
        }
    }

    /// Register an arbitrary external undo. Prefer the typed helpers below; this
    /// exists for composition (and is the seam the journal's unit tests drive).
    pub fn defer(&mut self, undo: impl FnOnce() + 'a) {
        self.undos.push(Box::new(undo));
    }

    /// A worktree was created at `worktree_path` from `bare_path`. The undo
    /// removes the worktree and — when `created_branch` is set (the branch was
    /// freshly created here, not reused) — force-deletes that branch. The two
    /// run worktree-first so the branch is no longer checked out.
    pub fn worktree(
        &mut self,
        bare_path: &Path,
        worktree_path: &Path,
        created_branch: Option<&str>,
    ) {
        let bare = bare_path.to_path_buf();
        let wt = worktree_path.to_path_buf();
        let branch = created_branch.map(str::to_string);
        let verbose = self.verbose;
        self.defer(move || {
            log_undo(
                verbose,
                "remove worktree",
                git::remove_worktree(&bare, &wt, verbose),
            );
            if let Some(b) = branch {
                // force: this branch was created by the failed command itself.
                log_undo(
                    verbose,
                    "delete branch",
                    git::delete_branch(&bare, &b, true, verbose),
                );
            }
        });
    }

    /// A directory tree was created; the undo removes it (covers any files
    /// written inside it, e.g. CONTEXT.md).
    pub fn dir(&mut self, path: &Path) {
        let path = path.to_path_buf();
        let verbose = self.verbose;
        self.defer(move || {
            log_undo(
                verbose,
                "remove directory",
                std::fs::remove_dir_all(&path).map_err(GroveError::from),
            );
        });
    }

    /// Disarm: every side-effect succeeded and was durably recorded, so nothing
    /// is undone when the journal drops.
    pub fn commit(mut self) {
        self.committed = true;
        self.undos.clear();
    }
}

impl Drop for StepJournal<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for undo in std::mem::take(&mut self.undos).into_iter().rev() {
            undo();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// SJ-drop-without-commit-undos-reverse (P1): dropping without commit runs
    /// every undo exactly once, in reverse registration order.
    #[test]
    fn drop_without_commit_undos_reverse() {
        let log: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
        {
            let mut journal = StepJournal::new(false);
            for id in [1u32, 2, 3] {
                let log = Rc::clone(&log);
                journal.defer(move || log.borrow_mut().push(id));
            }
            // dropped here without commit()
        }
        assert_eq!(*log.borrow(), vec![3, 2, 1]);
    }

    /// SJ-commit-runs-no-undos (P1): commit() disarms — no undo runs.
    #[test]
    fn commit_runs_no_undos() {
        let log: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
        {
            let mut journal = StepJournal::new(false);
            for id in [1u32, 2, 3] {
                let log = Rc::clone(&log);
                journal.defer(move || log.borrow_mut().push(id));
            }
            journal.commit();
        }
        assert!(log.borrow().is_empty());
    }

    /// SJ-undo-runs-on-panic-unwind (P2): undos run during panic unwinding too.
    /// The release profile uses panic=unwind, so this holds for the release
    /// binary as well (undo failures stay best-effort/log-only).
    #[test]
    fn undo_runs_on_panic_unwind() {
        let log: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
        let log_outer = Rc::clone(&log);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let mut journal = StepJournal::new(false);
            for id in [1u32, 2] {
                let log = Rc::clone(&log_outer);
                journal.defer(move || log.borrow_mut().push(id));
            }
            panic!("boom");
        }));
        assert!(result.is_err());
        assert_eq!(*log.borrow(), vec![2, 1]);
    }

    /// PANIC-release-not-abort: the release profile must unwind (not abort) so
    /// the drop-based undo above also runs in the release binary. Guard against
    /// a regression that reintroduces `panic = "abort"`.
    #[test]
    fn release_profile_does_not_abort() {
        let cargo_toml = include_str!("../../Cargo.toml");
        let release = cargo_toml
            .split("[profile.release]")
            .nth(1)
            .expect("Cargo.toml has a [profile.release] section");
        let release = release.split("\n[").next().unwrap();
        assert!(
            !release.contains("panic = \"abort\""),
            "release profile must not set panic = \"abort\""
        );
    }
}
