# Grove Audit Remediation — Progress Tracker

Tracks `PLAN.md` (24 steps / 6 phases) and `TEST_PLAN.md` (~40 cases + 10 seams + 2 CI gates).
Status keys: `[ ]` todo · `[~]` in progress · `[x]` done · `[!]` blocked.
Decisions locked: **panic="unwind"** · **all 6 phases**.

---

## Steps

### Phase 1 — Security & data-loss (P1)
- [x] **S1** — tmux-hook injection → `#{q:pane_current_path}` + quote grove_bin · tmux.rs:274-277 · *low*
- [x] **S2** — harden legacy migration: `?` parse, propagate upserts, txn, rename only after non-empty import · db.rs:342,364,389,426,432-434 · *med*
- [x] **S5** — flip release profile to `panic="unwind"`; drop misleading test caveat · Cargo.toml:35, rollback.rs:169-185 · *low*
- [x] **Gate P1** — quality gate: fmt clean, clippy zero warnings, structural gates green, full suite 138 green

### Phase 2 — Data integrity & partial failure (P2)
- [x] **S3** — migrations atomic (`transaction()`) **and** V5 re-runnable (static SCHEMA_V5 → idempotent `migrate_v5` Rust method) · db.rs:181-209,683-695 · *med*
- [x] **S4** — `close`: two-pass; remove worktrees before destructive kill+delete; non-force removal failure aborts with nothing destroyed (keeps N4) · close.rs · *med*
- [x] **S6** — sticky `data_stale` + `projects_error` on refresh failure; `apply_refresh`/`apply_projects`/`load_projects` seams · app.rs · *low*
- [x] **Gate P2** — fmt clean, clippy zero warnings, structural green, full suite 146 green

### Phase 3 — Correctness & UX (P2 + P3)
- [x] **S7** — fix scroll/cursor desync under search via `display_rows()`/`cursor_display_index()`/`clamp_scroll()`; ui renders in compacted display space · ui.rs:73-172 · *med*
- [x] **S8** — re-anchor selection to pane id across rebuild (`pane_cursor_position`) · tree.rs:118-149 · *med*
- [x] **S9** — `should_background_refresh()` gates tick/SIGUSR1 while notepad focused · event.rs:135-143 · *low*
- [x] **Gate P3** — fmt clean, clippy zero warnings, structural green, full suite 150 green

### Phase 4 — Performance: per-tick & startup fan-out (P1-perf + P2 + P3)
- [x] **S10** — `ProcessTreeCache`: negative caching w/ TTL, injectable resolver/clock, shell-skip · agent.rs:281-323 · *med*
- [x] **S11** — bounded `ProcessTreeCache`(cap 512) + `ProjectRootCache`(cap 1024) with `gc(live)` · tree.rs:9-44, agent.rs · *low*
- [x] **S12** — process-once guard; always overwrites via `set-hook -g` safe text; `project_hook_set_args` seam · tmux.rs:263-278 · *low* · dep:S1
- [x] **S13** — `construct`/`initial_refresh` split; first frame painted before blocking refresh; no key before first refresh · mod.rs, app.rs, event.rs · *med*
- [x] **S14** — `should_recapture_preview` + `capture_args` (`-S -N` bounded); skip tick capture on unchanged activity · app.rs, tmux.rs · *med*
- [x] **S15** — `diff_dirty`(HEAD+index token) + `diff_token_changed`; skip per-tick diff when unchanged · app.rs, source.rs · *med*
- [x] **S16** — `current_session_cached` (OnceLock), single pane_agents read via `gc_returning`, borrow-not-clone (done in S10 batch), draw_projects alloc drop · app.rs, agent.rs, ui.rs · *low* · dep:S10
- [x] **Gate P4** — fmt clean, clippy zero warnings, structural green, full suite 162 green

### Phase 5 — Architecture & DRY (cleanup; net-negative LOC)
- [x] **S17** — one `rows()` iterator + `row_at_cursor`/`group_header_position` replace seven flat-row walks; ui rides `display_rows` derived from `rows()` · tree.rs, ui.rs · *med* · dep:S7,S8
- [x] **S18** — split App → `PreviewState`/`ProjectsState`/`UiState` sub-structs (~150 access sites updated) · app.rs · *med*
- [x] **S19** — extract `provision_worktree` into commands/util.rs; init+add both route through it · add.rs, init.rs, util.rs · *low*
- [x] **S20** — source.rs split data-fetch vs `DiffState` (new diff_view.rs); actions `respond_to_agent`/`switch_after_launch` dedup; refresh_tree helpers · *low* · dep:S18
- [x] **Gate P5** — fmt clean, clippy zero warnings, structural green (G2/G3 intact), full suite 165 green

### Phase 6 — Test backfill (coverage-gap findings)
- [x] **S21** — `git.rs` unit tests (delete -d/-D, remove_worktree, uncommitted, create, invalid-utf8) · git.rs · *low*
- [x] **S22** — `is_workspace` + `build_groups` workspace grouping via `build_groups_with_resolver`/`git_toplevel` seam · tree.rs · *low*
- [x] **S24** — AgentResolver precedence reconciled (**DB kind wins** over state-file kind; wrapper made consistent); sync diverged-branch lossy behavior pinned + WARNING/TODO · agent.rs, git.rs · *med*
- [x] **S25** — diff non-zero exit → Err (not "No changes"); add branch/base validated; `--` before clone URL; pid-file failure surfaced; consistent poison recovery; state-file user-scoped (`$XDG_RUNTIME_DIR`/`/tmp/grove-<user>`) + shell hooks synced · multiple · *low*
- [x] **Gate P6** — fmt clean, clippy zero warnings, structural green, full suite 185 green, release build OK

> **Phase-exit gate (every phase):** after a phase's steps land, run `/thermo-nuclear-code-quality-review` scoped to that phase's diff. Triage findings — fix legit, note deferred with rationale. Re-run the full suite; **all tests pass and no unaddressed finding may remain** before the phase closes. A failing finding or red test blocks sign-off.

---

## Test seams (land before gated cases)
- [x] **S-hookbuilder** — `project_hook_command(grove_bin)` pure (unlocks HOOK-*)
- [x] **S-gatefn** — `App::should_background_refresh()` (unlocks GATE-*)
- [x] **S-displayrows** — `TreeState::display_rows()` + `clamp_scroll()` + `cursor_display_index()` (unlocks SCROLL-*; reused by S17)
- [x] **S-refresh** — `App::apply_refresh(Result)` + `data_stale` + `load_projects()` error marker (unlocks REFRESH-*, PROJECTS-*)
- [x] **S-ptreecache** — `ProcessTreeCache` w/ injectable resolver + TTL (unlocks PTREE-*)
- [x] **S-boundedcache** — capacity + `gc(live_set)` on caches (unlocks CACHE-*)
- [x] **S-construct** — split `App::new` → `construct` + `initial_refresh` (unlocks INIT-*)
- [x] **S-recap** — `should_recapture_preview()` + `capture_args(n)` (unlocks PREVIEW-*)
- [x] **S-difftoken** — `diff_dirty(repo)->token` (unlocks DIFF-*)
- [x] **S-rootresolver** — injectable root-resolver in build_groups (unlocks WORKSPACE-*)

---

## Test cases  ·  NAME · prio · class · writability

### Phase 1
- [x] HOOK-uses-q-modifier · P0 · red-now-bug · S-hookbuilder
- [x] MIG-corrupt-errors-preserves-file · P0 · red-now-bug · Today (state + recents variants)
- [x] MIG-zero-yield-not-renamed · P1 · red-now-bug · Today (state + recents variants)
- [x] MIG-valid-imports-then-renames · P2 · characterization · Today
- [x] MIG-upsert-failure-propagates · P2 · covered by txn rollback (`?` propagation in `transaction()`)
- [x] PANIC-release-not-abort · P1 · structural · rollback.rs test + PANIC structural gate

### Phase 2
- [x] MIG-v0-to-v5-fresh · P1 · characterization · Today
- [x] MIG-incremental-v1-to-v5 · P2 · characterization · Today
- [x] MIG-v5-partial-rerun-recovers · P1 · red-now-bug · Today
- [x] CLOSE-nonforce-removal-fail-preserves · P1 · red-now-bug · Today (worktree-lock scaffold)
- [x] CLOSE-idempotent-retry · P2 · characterization · Today
- [x] REFRESH-failure-sets-sticky-stale · P2 · new-module-spec · S-refresh
- [x] REFRESH-keypress-preserves-stale · P2 · red-now-bug · S-refresh
- [x] PROJECTS-db-error-distinct-from-empty · P2 · new-module-spec · S-refresh (marker logic via apply_projects; raw-DB error path noted as indirect)

### Phase 3
- [x] SCROLL-deep-cursor-filtered-not-blank · P1 · red-now-bug · S-displayrows
- [x] SCROLL-invariant-pin · P1 · property-equivalence · S-displayrows (written against public seam; carried into S17)
- [x] REBUILD-keeps-selected-across-reorder · P1 · red-now-bug · Today
- [x] GATE-suppress-under-notepad · P2 · red-now-bug · S-gatefn

### Phase 4
- [x] PTREE-negative-cached · P1 · new-module-spec · S-ptreecache
- [x] PTREE-ttl-rewalk · P2 · new-module-spec · S-ptreecache
- [x] PTREE-shell-skip · P2 · new-module-spec · S-ptreecache
- [x] CACHE-evicts-dead-and-bounds · P2 · new-module-spec · S-boundedcache (agent.rs + tree.rs)
- [x] HOOK-register-overwrites-not-skips · P2 · characterization · S-hookbuilder
- [x] INIT-construct-valid-empty · P2 · new-module-spec · S-construct
- [x] PREVIEW-skip-when-activity-unchanged · P2 · new-module-spec · S-recap
- [x] PREVIEW-capture-bounded · P2 · characterization · S-recap
- [x] DIFF-skip-when-unchanged · P2 · new-module-spec · S-difftoken
- [x] DETECT-borrow-equiv-clone · P2 · property-equivalence · Today (oracle=current)
- [x] SESSION-cached-once · P3 · characterization · Today

### Phase 5
- [x] ROWS-iterator-equiv-old-walks · P1 · property-equivalence (inline old-walk oracle; SCROLL-invariant-pin carried unchanged)
- [x] APP-split-behavior-preserved · P2 · characterization (full suite green pre/post split)
- [x] PROVISION-rollback-equiv · P1 · property-equivalence (init+add reused-branch-preserved pins + existing created-branch force-delete pins)
- [x] DECOMP-behavior-preserved · P2 · characterization (full suite green through source/actions/refresh decomposition)

### Phase 6
- [x] GIT-delete-d-preserves-unmerged · P0 · new-module-spec · Today
- [x] GIT-delete-D-force-removes · P1 · new-module-spec · Today
- [x] GIT-remove-worktree / -uncommitted / -create / -invalid-utf8 · P2 · new-module-spec · Today
- [x] WORKSPACE-{2plus-true,one-false,own-git-false,unreadable-false,siblings-collapse} · P2 · new-module-spec · S-rootresolver
- [x] RESOLVER-precedence-pinned · P2 · new-module-spec · Today (DB kind wins, reconciled)
- [x] SYNC-diverged-default-branch · P2 · new-module-spec · Today (lossy behavior pinned + WARNING/TODO)
- [x] DIFF-broken-repo-not-no-changes / ARG-add-branch-base / ARG-register-double-dash / PIDFILE / POISON / STATEFILE · P2-P3 · new-module-spec · Today

---

## CI structural gates
- [x] **G1 (PANIC)** — `[profile.release]` must not set `panic="abort"` (guards S5) — added as `PANIC` check (script already used G1-G3 for other invariants)
- [x] **G2 (HOOK)** — no bare `#{pane_current_path}` in a single-quoted `run-shell` string in `src/` (guards S1) — added as `HOOK` check
- [x] Wire into `scripts/structural-checks.sh` — both gates added and passing

---

## Strategy
- Write the **writable-today red-nows first** (no code change) → confirm red → proves the bugs real + harness works.
- Then **per step: red → fix → green.** Land one step's fix, its reds flip. Don't batch.
- **Seam tests land with their seam**, not up front.
- **Equivalence/characterization pins** (ROWS, PROVISION, DETECT, APP-split, DECOMP) written **before** the refactor they guard, using OLD code as oracle; must stay green through it. SCROLL-invariant-pin from S7 is carried verbatim into S17.
- **End of each phase: thermo-nuclear gate** — review the phase diff, fix findings, full suite green before moving on.

## Authoring order
1. Writable-today red-nows (MIG-*, REBUILD-keeps-selected, CLOSE-nonforce, MIG-v5-partial) → confirm red.
2. G1/G2 structural gates.
3. Seam S-hookbuilder → HOOK-* ; S-gatefn → GATE-*.
4. Seam S-displayrows → SCROLL-* (incl. invariant pin).
5. Seam S-refresh → REFRESH-*/PROJECTS-*.
6. Phase-4 seams (ptreecache, boundedcache, construct, recap, difftoken) → their cases.
7. Equivalence/characterization pins last, each before its refactor (S16/S17/S18/S19/S20).
8. Phase-6 backfill specs (S21/S22/S24/S25).

---

## Log
- 2026-06-19 — **Phase 6 DONE. ALL 6 PHASES COMPLETE.** S21 (git.rs unit tests incl. invalid-utf8 arms). S22 (`build_groups_with_resolver`/`git_toplevel` seam + is_workspace/grouping tests). S24 (resolver precedence reconciled — **DB kind wins**, wrapper made consistent; sync diverged-branch lossy behavior pinned + WARNING/TODO). S25 (diff non-zero exit → Err; add branch/base validated; `--` before clone URL; pid-file failure surfaced; consistent mutex poison recovery; state-file hardened to user-scoped path + shell hooks synced). Final: fmt+clippy clean, **185 tests green** (127 bin + 58 integration), structural gates green, release build (panic=unwind) OK.
- 2026-06-19 — **Phase 5 DONE.** S17 (single `rows()` traversal + `row_at_cursor`/`group_header_position` replace ~7 walks, ~-55 LOC tree.rs; ui rides `display_rows` derived from `rows()`). S18 (App split into `PreviewState`/`ProjectsState`/`UiState`; ~150 access sites updated; tree/db/config/notepad stay on App). S19 (`provision_worktree` in commands/util.rs shared by init+add). S20 (source.rs data-fetch vs new diff_view.rs `DiffState`; actions `respond_to_agent`/`switch_after_launch`; refresh_tree → `gc_and_read_kinds`/`upsert_projects_if_groups_changed`). Pins: ROWS-equiv, PROVISION-rollback-equiv, APP-split + DECOMP (suite). fmt+clippy clean, 165 green.
- 2026-06-19 — **Phase 4 DONE.** S10 (`ProcessTreeCache`: negative caching, TTL, injectable resolver+clock, shell-skip). S11 (bounded ProcessTreeCache cap 512 + ProjectRootCache cap 1024 + `gc(live)`). S12 (process-once guard, always overwrites safe `set-hook -g`, `project_hook_set_args` seam). S13 (`construct`/`initial_refresh` split, first frame before blocking refresh, no key before first refresh). S14 (`should_recapture_preview`+`capture_args` `-S -N`, skip on unchanged activity). S15 (`diff_dirty` HEAD+index token, skip per-tick diff). S16 (`current_session_cached` OnceLock, single pane_agents read via `gc_returning`, borrow-not-clone, draw_projects alloc drop). All PTREE/CACHE/HOOK/INIT/PREVIEW/DIFF/DETECT/SESSION tests green. fmt+clippy clean, 162 green.
- 2026-06-19 — **Phase 3 DONE.** S7 (`display_rows`/`cursor_display_index`/`clamp_scroll` seam; ui.rs renders + scrolls in compacted display space, fixing filtered-search blank/desync). S8 (rebuild captures `selected_pane_id` and re-anchors via `pane_cursor_position`). S9 (`should_background_refresh` adds notepad-focus to the tick/SIGUSR1 gate). Tests: SCROLL-deep-cursor, SCROLL-invariant-pin (seam-only, S17-portable), REBUILD-keeps-selected, GATE-suppress-under-notepad. fmt+clippy clean, 150 green.
- 2026-06-19 — **Phase 2 DONE.** S3 (each migration step wrapped in `transaction()`; SCHEMA_V5 const replaced by idempotent `migrate_v5` recovering any half-applied state). S4 (close split into two passes — worktrees removed before any destruction; non-force removal failure aborts intact, force unchanged, N4 preserved). S6 (sticky `data_stale`/`projects_error`, `apply_refresh`/`apply_projects`/`load_projects` seams). Tests: MIG-v0/v1/partial-rerun, CLOSE-nonforce-preserves + force-retry, REFRESH-sticky/keypress, PROJECTS-db-error. fmt+clippy clean, 146 green.
- 2026-06-19 — **Phase 1 DONE.** S1 (tmux `#{q:}` hook + `project_hook_command` seam), S2 (migration: `?` parse, txn-wrapped import, propagated upserts, rename only on count>0), S5 (release `panic` unwind, stale caveat fixed, structural test). Tests: HOOK-uses-q-modifier, MIG-corrupt/zero-yield/valid (state+recents), PANIC-release-not-abort. Gates PANIC+HOOK added to structural-checks.sh (G1-G3 names already taken). fmt+clippy clean, 138 tests green.
- 2026-06-19 — Tracker created from PLAN.md + TEST_PLAN.md. Source: 42 verified audit findings (2 P1, 14 P2, 26 P3). Plan reviewed by self + codex + cursor; consensus folded (S2 hardened, S3 re-runnable, S4 reordered for N4, S7↔S17 pin, S10 TTL cache, S11 de-scoped from SQLite, S12 overwrite-safe, S24 precedence reconcile, S25 +add injection). Decisions: panic="unwind", all 6 phases.
