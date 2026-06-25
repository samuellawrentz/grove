# Grove Audit Remediation — Plan (rev. after 3-agent review)

Source: 42 adversarially-verified findings (2 P1, 14 P2, 26 P3). Reviewed by self + codex + cursor; consensus folded in (see Changelog).

**Bias:** correctness & security first, pure cleanup last. Phases 1–3 add guards. Phases 4–5 are mostly subtraction — caching replaces re-spawns, one iterator replaces seven walks, sub-structs replace a god-struct; net LOC trends down. Every step independently shippable (compiles, tests green, releasable alone).

Each step: `scope · finding(file:line) · risk · deps`.

---

## Phase 0 — Decisions (resolve before Phase 2)
- **D1 (panic strategy, gates S5):** release `panic="unwind"` (one-line; restores journal + `TerminalGuard` Drop that **init/add/register** all rely on) vs. keep `abort` + explicit init undo on the panic path. **Recommend `unwind`** unless binary size is a hard constraint — then it lands in Phase 1.
- **D2 (scope):** all 6 phases, or land Phases 1–2 first and defer 4/5/6?

## Phase 1 — Security & data-loss (P1). Ship first.

- **S1** Fix tmux-hook shell injection: replace the single-quoted `#{pane_current_path}` splice with `#{q:pane_current_path}` (tmux shell-quote modifier); quote `grove_bin`. Registration runs every launch and `set-hook` overwrites, so the safe text replaces any prior bad hook. · `tmux.rs:274-277` · **risk: low** · dep: none
- **S2** Stop legacy-migration data loss — **harden fully**: parse with `?` (not `unwrap_or_default`); propagate per-row upsert failures (`db.rs:389,426`); run the import in one transaction; rename `state.json`/`recents.json` → `.migrated` **only after** a successful, non-empty import (zero-yield ≠ "superseded"); surface rename failure. · `db.rs:342,364,389,426,432-434` · **risk: med** · dep: none
- **S2-T** (pulled forward from Phase 6) Migration/data-loss characterization tests: corrupt file → error surfaced + file **not** renamed; valid file → imported + renamed; empty/zero-yield → not silently renamed away. · `db.rs` · dep: S2

## Phase 2 — Data integrity & partial failure (P2).

- **S3** Make migrations atomic **and** re-runnable: wrap each `(schema batch + user_version bump)` in `transaction()`, **and** edit `SCHEMA_V5` so a partial prior run is recoverable (`CREATE TABLE IF NOT EXISTS task_repos_v5`, guard the INSERT/DROP/RENAME against a half-applied state). · `db.rs:181-209,683-695` · **risk: med** · dep: none
- **S3-T** Migration upgrade-path tests: v1→…→v5 incremental, from-scratch v0 jump, and a simulated crash-between-batch-and-bump re-open. · `db.rs` · dep: S3
- **S4** `close` partial-failure: **reorder so nothing is destroyed before the worktree is gone**. Attempt/pre-check `git worktree remove` first; on non-force failure, abort **before** killing tmux / deleting the row — task stays fully intact and re-closable (preserves the N4 idempotency invariant, which a naive "keep the row" would break). · `close.rs:61-67,81-95,147-155` · **risk: med** · dep: none
- **S5** Apply D1: if `unwind`, flip `Cargo.toml` profile + delete the misleading unwind-only caveat on the test; if `abort`, add explicit init-undo on panic. · `Cargo.toml:35`, `rollback.rs:121-130,169-185` · **risk: med** · dep: D1
- **S6** Surface persistent refresh failure with a **sticky** stale indicator (separate from the keypress-cleared `status_message`); distinguish empty-projects from a DB error instead of `unwrap_or_default()`. · `app.rs:203-206,93,264` · **risk: low** · dep: none

## Phase 3 — Correctness & UX (P2 + adjacent P3).

- **S7** Fix tree scroll/cursor desync under search: render in the **compacted** display-row space and clamp `scroll_offset ≤ lines.len()`. Its test is written as the **invariant pin** "for any `search_filter` × expand × rebuild, cursor is valid in filtered space and `scroll_offset ≤ lines.len()`" — this test is carried forward unchanged into S17 so the later refactor can't regress it. · `ui.rs:73-172`, `actions.rs:617-626` · **risk: med** · dep: none
- **S8** Re-anchor selection across `rebuild`: capture `selected_pane_id()` before, restore the cursor to that pane id after (panes re-sort every tick). · `tree.rs:118-149` · **risk: med** · dep: none
- **S9** Notepad-focus refresh gate: extend the tick/SIGUSR1 gate beyond `overlay_active()` to also suppress while `focus == Focus::Notepad` (notepad is a `Focus`, not an `Overlay`), **or** re-run `sync_note_to_group()` after each background `refresh_tree`. · `event.rs:135-143`, `app.rs:271-275` · **risk: low** · dep: none

## Phase 4 — Performance: per-tick & startup fan-out. Mostly caching/subtraction.

- **S10** Process-tree agent detection: cache **negative** results with **TTL / window-activity invalidation** (a bare pid-key would miss an agent launched later under the same shell pid), and skip the `pgrep`/`ps` walk for known shells with no agent child. Kills the startup #1 cost + the 5s-tick re-walk. · `agent.rs:281-323` · **risk: med** · dep: none
- **S11** Bound + GC the in-process project-root cache (also fixes the unbounded-`TREE_CACHE` P3). **Not** SQLite-persisted — derived FS state isn't worth the invalidation complexity, and S10 is the bigger win. · `tree.rs:9-44`, `agent.rs:276-323` · **risk: low** · dep: none
- **S12** Register tmux hooks idempotently/once instead of every launch — **must overwrite with the safe (S1) text, never skip/preserve an existing entry**. · `tmux.rs:263-278` · **risk: low** · dep: **S1**
- **S13** Paint the first frame from the cheap construct **before** the blocking refresh (perceived-latency fix). **Behavior change**, not pure perf: add a characterization test that the pre-refresh empty frame has a valid cursor and no key action is processed before the first refresh completes. · `mod.rs:65-74`, `app.rs:79-99`, `event.rs:48-56` · **risk: med** · dep: none
- **S14** Bound preview capture to the viewport (`-S -N`) and skip the tick re-capture when the selected pane's activity is unchanged. · `app.rs:271-275,235-243`, `tmux.rs:191-206` · **risk: med** · dep: none
- **S15** Diff-mode: skip the per-tick `git diff` re-run + reparse unless the repo HEAD/index changed. · `app.rs:210-233`, `source.rs:424-465` · **risk: med** · dep: none
- **S16** Fold cheap P3 perf (small, honest wins): cache `current_session` per process; avoid the redundant `pane_agents` SELECT+clone per tick; borrow (not clone) the state/recorded maps in `detect_agent_in_pane`; reduce `draw_projects` per-frame allocs. · `app.rs:183,164-173`, `agent.rs:423-434`, `ui.rs:230-263` · **risk: low** · dep: S10

## Phase 5 — Architecture & DRY (pure cleanup; net-negative LOC). Ship last.

- **S17** Replace the seven hand-rolled flat-row walks with one `rows() -> impl Iterator<Item = Row>`; route `selected_pane`/`selected_group`/`visible_count`/`pane_positions`/collapse/expand and `ui::draw_tree` through it. Absorbs S7's impl; **carries S7's invariant-pin test unchanged**. · `tree.rs:153-362`, `ui.rs:73-172` · **risk: med** · dep: S7,S8
- **S18** Split `App` into `PreviewState`/`ProjectsState`/`UiState`; handlers borrow only their sub-struct. Pure field move — **soft-ordered** after the Phase-3 behavior fixes to avoid rebasing them onto new paths, but carries no hard dependency. · `app.rs:45-70` · **risk: med** · dep: none (soft: after S6,S7,S13)
- **S19** Extract `provision_worktree(journal,bare,wt,branch,base)` into `commands/util.rs`; call from both `init` and `add` (kills the rollback-sensitive duplication). · `add.rs:48-64`, `init.rs:194-200` · **risk: low** · dep: none
- **S20** Fold remaining P3 dedup/decomposition: split `source.rs` data-fetch vs `DiffState` render; dedup `actions.rs` accept/reject registry walk and `launch_split`/`launch_in_new_window`; break `refresh_tree`'s inline concerns into named helpers. · `source.rs:271-405`, `actions.rs:401-436,591-614`, `app.rs:156-207` · **risk: low** · dep: S18

## Phase 6 — Test backfill (remaining coverage-gap findings).

- **S21** `git.rs` unit tests: `delete_branch` `-d` preserves unmerged / `-D` force-deletes, `remove_worktree`, `has_uncommitted_changes` dirty/clean, `create_worktree`, invalid-UTF8-path arms. · `git.rs` · dep: none
- **S22** `is_workspace` tempdir tests + a `build_groups` workspace-grouping test. **Requires adding an injectable root-resolver seam** (the fn is private and shells to real git today; S11 does not provide this). · `tree.rs:27-61,479-483` · dep: none (adds own seam)
- **S24** Pin `AgentResolver`'s **actual** precedence and **reconcile the contradiction**: resolver prefers `state_kind_map`, but `app.rs:171-175` merges so db-kind wins, and the wrapper passes an empty `state_kind_map`. Decide the intended winner, encode it, test it. Plus `sync` diverged-default-branch behavior pinned/guarded. · `agent.rs:384-389,423-432`, `git.rs:126-130` · dep: none
- **S25** Remaining P3 hardening + tests: `git diff` non-zero exit not rendered "No changes"; PID-file write failure surfaced; consistent mutex-poison recovery; arg-injection validation incl. **`add` branch/base** (not just register/init) and `--` before URLs; state-file `/tmp` path hardening. · `source.rs:447-459`, `tui/mod.rs:61-63`, `agent.rs:13,288,306`, `init.rs:101,111`, `add.rs:33-55`, `register.rs:17,54` · dep: none

---

## Per-phase gate
After each phase: `/thermo-nuclear-code-quality-review` on that phase's diff → triage (fix legit, note deferred) → full test suite. All green + no unaddressed finding before the phase closes.

## Finding-coverage map
- 2 P1 → S1, S2.
- 14 P2 → S3, S4, S5, S6, S7, S8, S14, S15, S17, S18, S19, S21/S22/S24 (test gaps), S3-T (migration test).
- 26 P3 → folded into S9, S10/S11/S16 (perf+cache), S20, S25. Every P3 file:line appears in a step; none dropped.

## Changelog (3-agent review)
S2 strengthened (upsert failures, zero-yield, txn) + tests pulled to Phase 1. S3 now edits SCHEMA_V5 for re-runnability + crash test. S4 reordered to preserve N4. S5 gated on D1, decision elevated. S7↔S17 dep fixed; S7 test is the carried-forward invariant pin. S10 negative cache gains TTL/activity invalidation. S11 de-scoped from SQLite to bounded in-memory GC. S12 dep:S1, overwrite-not-skip. S13 reclassified behavior-change + test. S18 deps relaxed to soft. S22 owns its seam. S24 reconciles precedence contradiction. S25 adds `add` injection.
