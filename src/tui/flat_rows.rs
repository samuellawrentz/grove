//! Shared cursor/count math for flat, expand/collapse row models.
//!
//! Both the pane tree (`tree::TreeState`) and the diff view
//! (`source::DiffState`) present a *flat* list of rows where expanding a node
//! mutates the row set under the cursor. The arithmetic for moving the cursor
//! across that mutating set — and never overrunning it — is identical and was
//! previously hand-rolled in each, where the `move_down_by` stale-total overrun
//! lived. It is extracted here **once** so neither caller can reintroduce that
//! bug class (CI gate G3: no second copy of this arithmetic).

/// A flat, expand/collapse row model with a single moving cursor.
///
/// Implemented by both `TreeState` and `DiffState`. The cursor/overrun math
/// lives in the default methods here — the *one* copy of the arithmetic (CI
/// gate G3) — so each model only supplies the row-set size, its cursor, and an
/// optional per-step side effect (auto-expand). `total()` is queried *every*
/// step so a mid-walk collapse that shrinks the set can never be overrun.
pub(crate) trait FlatRows {
    /// Current number of visible rows.
    fn total(&self) -> usize;
    /// The flat cursor position.
    fn cursor(&self) -> usize;
    /// Set the flat cursor position.
    fn set_cursor(&mut self, cursor: usize);
    /// Per-step hook run after each increment/decrement (default: no-op).
    /// Implementors (e.g. diff auto-expand) may mutate the row set and re-seat
    /// the cursor; the loop re-reads `total()` afterward.
    fn on_step(&mut self) {}

    /// Clamp the cursor into `0..total()` (or 0 when empty).
    fn clamp(&mut self) {
        let c = clamp_cursor(self.cursor(), self.total());
        self.set_cursor(c);
    }

    /// Move the cursor down by `n` rows, recomputing the total each step.
    fn move_down_by(&mut self, n: usize) {
        for _ in 0..n {
            if self.cursor() + 1 >= self.total() {
                break;
            }
            self.set_cursor(self.cursor() + 1);
            self.on_step();
        }
        self.clamp();
    }

    /// Move the cursor up by `n` rows.
    fn move_up_by(&mut self, n: usize) {
        for _ in 0..n {
            if self.cursor() == 0 {
                break;
            }
            self.set_cursor(self.cursor() - 1);
            self.on_step();
        }
        self.clamp();
    }
}

/// Clamp `cursor` into the valid range `0..total` (or `0` when empty).
/// The single place the "cursor never sits past the end" invariant is enforced.
pub(crate) fn clamp_cursor(cursor: usize, total: usize) -> usize {
    if total == 0 {
        0
    } else {
        cursor.min(total - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic `FlatRows`: a fixed-size set that collapses (shrinks) once the
    /// cursor crosses `collapse_at`, optionally re-seating the cursor on each
    /// step — exercising the same mutation shape as diff auto-expand.
    struct Synthetic {
        cursor: usize,
        big: usize,
        small: usize,
        collapse_at: usize,
        collapsed: bool,
        reseat_to: Option<usize>,
    }

    impl FlatRows for Synthetic {
        fn total(&self) -> usize {
            if self.collapsed {
                self.small
            } else {
                self.big
            }
        }
        fn cursor(&self) -> usize {
            self.cursor
        }
        fn set_cursor(&mut self, c: usize) {
            self.cursor = c;
        }
        fn on_step(&mut self) {
            if self.cursor >= self.collapse_at {
                self.collapsed = true;
                if let Some(r) = self.reseat_to {
                    self.cursor = r;
                }
            }
        }
    }

    /// FLATROWS-cursor-math-generic (P1, Step 10): the generic move math must
    /// never leave the cursor past the row set, even when `on_step` shrinks the
    /// set mid-walk. Synthetic only — no tree/diff/tmux/git.
    #[test]
    fn flatrows_cursor_math_generic() {
        // Static set: walking past either end clamps in range.
        let mut s = Synthetic {
            cursor: 0,
            big: 5,
            small: 5,
            collapse_at: usize::MAX,
            collapsed: false,
            reseat_to: None,
        };
        s.move_down_by(100);
        assert_eq!(s.cursor, 4, "down past the end clamps to last row");
        s.move_up_by(100);
        assert_eq!(s.cursor, 0, "up past the start clamps to first row");

        // Shrinking set: at cursor>=50 the set collapses from 100 to 6. A
        // once-cached total of 100 would walk far past 6 (the original overrun
        // bug). Recomputing each step must keep the cursor < the final total.
        let mut s = Synthetic {
            cursor: 49,
            big: 100,
            small: 6,
            collapse_at: 50,
            collapsed: false,
            reseat_to: None,
        };
        s.move_down_by(30);
        assert!(
            s.cursor < s.total(),
            "cursor {} overran shrunken total {}",
            s.cursor,
            s.total()
        );

        // Re-seating on_step (auto-expand snapping the cursor back): the walk
        // must still terminate inside the (shrunken) set.
        let mut s = Synthetic {
            cursor: 0,
            big: 100,
            small: 8,
            collapse_at: 1,
            collapsed: false,
            reseat_to: Some(2),
        };
        s.move_down_by(10);
        assert!(s.cursor < s.total());
    }

    #[test]
    fn clamp_cursor_empty_is_zero() {
        assert_eq!(clamp_cursor(7, 0), 0);
        assert_eq!(clamp_cursor(7, 3), 2);
        assert_eq!(clamp_cursor(1, 3), 1);
    }
}
