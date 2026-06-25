# Grove Audit Remediation — Test Design

Cases that define "done" for each `PLAN.md` step. Class ∈ {`red-now-bug`, `characterization`, `property-equivalence`, `new-module-spec`}. Every case binds to a real public/`pub(crate)` interface (inline `#[cfg(test)]` module or `tests/` CLI via assert_cmd), never a struct shape. Decisions: **panic="unwind"** (S5 → Phase 1), **all 6 phases**.

## Seams (minimal behavior-preserving changes that make cases writable)
- **S-hookbuilder** — extract `fn project_hook_command(grove_bin: &str) -> String` from `register_project_hooks`. Unlocks HOOK-* (S1, S12).
- **S-refresh** — `App::apply_refresh(Result<(panes,states),_>)` + a sticky `data_stale` field; `App::load_projects()` returns/records a DB-error marker distinct from empty. Unlocks REFRESH-*, PROJECTS-* (S6).
- **S-gatefn** — `App::should_background_refresh(&self) -> bool` (= `!overlay_active() && focus != Notepad`). Unlocks GATE-* (S9).
- **S-displayrows** — `TreeState::display_rows()` (compacted, filter-aware) + `clamp_scroll(height)` pure helpers. Unlocks SCROLL-* (S7); reused by S17.
- **S-ptreecache** — `ProcessTreeCache` wrapping the static map with an injectable `resolver: Fn(pid)->Option<AgentKind>` + TTL/activity key. Unlocks PTREE-* (S10) without spawning pgrep/ps.
- **S-boundedcache** — give the root/kind caches a capacity + `gc(live_set)`. Unlocks CACHE-* (S11).
- **S-construct** — split `App::new` into `construct` (cheap, no refresh) + `initial_refresh`. Unlocks INIT-* (S13).
- **S-recap** — `should_recapture_preview(sel_activity,last)` + `capture_args(n)` builder. Unlocks PREVIEW-* (S14).
- **S-difftoken** — `diff_dirty(repo)->token` from HEAD+index. Unlocks DIFF-* (S15).
- **S-rootresolver** — `build_groups`/`resolve_project_root` take an injectable root-resolver. Unlocks WORKSPACE-* (S22).

---

## Phase 1 — Security & data-loss
**S1** (bind: `project_hook_command`)
- `HOOK-uses-q-modifier` · P0 · red-now-bug · seam S-hookbuilder — built string contains `#{q:pane_current_path}`, never a bare `#{pane_current_path}` inside `'…'`.

**S2** (bind: `Db::migrate_state_json` / `migrate_recents`, pub)
- `MIG-corrupt-errors-preserves-file` · P0 · red-now-bug · Today — corrupt JSON → `Err` returned **and** source file still present (not renamed).
- `MIG-zero-yield-not-renamed` · P1 · red-now-bug · Today — well-formed JSON, all entries missing required fields → file **not** renamed to `.migrated`.
- `MIG-valid-imports-then-renames` · P2 · characterization · Today — valid file → rows in DB + file renamed (pins the happy path the fix must keep).
- `MIG-upsert-failure-propagates` · P2 · new-module-spec · seam(test-only: read-only conn) — a failing upsert aborts the import without renaming.

**S5** (bind: build profile)
- `PANIC-release-not-abort` · P1 · structural (G1) — release profile must not set `panic="abort"`.
- keep `undo_runs_on_panic_unwind` · characterization — now reflects the release binary; drop the misleading caveat.

## Phase 2 — Data integrity & partial failure
**S3** (bind: `Db::migrate` via `open_path` + raw `user_version` setup)
- `MIG-v0-to-v5-fresh` · P1 · characterization · Today — fresh DB reaches user_version 5, schema intact.
- `MIG-incremental-v1-to-v5` · P2 · characterization · Today — start at v1 schema/version → upgrades to 5, data preserved.
- `MIG-v5-partial-rerun-recovers` · P1 · red-now-bug · Today — pre-create `task_repos_v5` + set version 4, re-run migrate → completes (no "table already exists"), rows intact.

**S4** (bind: `tests/cli_tests.rs` `grove close`, real git worktree)
- `CLOSE-nonforce-removal-fail-preserves` · P1 · red-now-bug · Today(test-only scaffold: `git worktree lock`) — locked worktree, non-force close → aborts, task row + worktree dir + tmux still present.
- `CLOSE-idempotent-retry` · P2 · characterization · Today — after the aborted close, a force close fully cleans up (N4 still holds).

**S6** (bind: `App::apply_refresh`, `App::load_projects`, `handle_key`)
- `REFRESH-failure-sets-sticky-stale` · P2 · new-module-spec · seam S-refresh — Err input sets `data_stale=true`; Ok clears it.
- `REFRESH-keypress-preserves-stale` · P2 · red-now-bug · seam S-refresh — a keypress clears `status_message` but **not** `data_stale`.
- `PROJECTS-db-error-distinct-from-empty` · P2 · new-module-spec · seam S-refresh — DB error records an error marker; genuine empty does not.

## Phase 3 — Correctness & UX
**S7** (bind: `TreeState::display_rows`, `clamp_scroll`)
- `SCROLL-deep-cursor-filtered-not-blank` · P1 · red-now-bug · seam S-displayrows — 30-pane group A + matching pane in B at deep row, search active → match row is within the visible window and `start ≤ display_len`.
- `SCROLL-invariant-pin` · P1 · property-equivalence · seam S-displayrows — **carried into S17**: ∀ (filter × expand-mask × rebuild-driven cursor), `scroll_offset ≤ display_rows.len()` and cursor maps to a real display row.

**S8** (bind: `TreeState::rebuild` + `selected_pane_id`, inline)
- `REBUILD-keeps-selected-across-reorder` · P1 · red-now-bug · Today — select pane X, rebuild with a state change that floats another pane above X → `selected_pane_id()==X`.

**S9** (bind: `App::should_background_refresh`)
- `GATE-suppress-under-notepad` · P2 · red-now-bug · seam S-gatefn — `focus=Notepad, overlay=None` → returns false (today only overlay is checked).

## Phase 4 — Performance
**S10** (bind: `ProcessTreeCache`)
- `PTREE-negative-cached` · P1 · new-module-spec · seam S-ptreecache — first None caches; second call within TTL does not invoke the resolver.
- `PTREE-ttl-rewalk` · P2 · new-module-spec · seam S-ptreecache — after TTL/activity change the resolver runs again (a later-launched agent is found).
- `PTREE-shell-skip` · P2 · new-module-spec · seam S-ptreecache — known shell, no agent child → cheap None without deep walk.

**S11** (bind: bounded cache `gc`)
- `CACHE-evicts-dead-and-bounds` · P2 · new-module-spec · seam S-boundedcache — entries for non-live panes removed; size stays ≤ capacity.

**S12** (bind: `project_hook_command` + registration op)
- `HOOK-register-overwrites-not-skips` · P2 · characterization · seam S-hookbuilder — idempotent path still emits the safe (`#{q:}`) text via an overwriting `set-hook`, never preserves a stale entry.

**S13** (bind: `App::construct`)
- `INIT-construct-valid-empty` · P2 · new-module-spec · seam S-construct — after `construct` (pre-refresh): empty tree, cursor 0, no panic, no preview fetch.

**S14** (bind: `should_recapture_preview`, `capture_args`)
- `PREVIEW-skip-when-activity-unchanged` · P2 · new-module-spec · seam S-recap.
- `PREVIEW-capture-bounded` · P2 · characterization · seam S-recap — capture args use a bounded `-S -N`, not full history.

**S15** (bind: `diff_dirty`)
- `DIFF-skip-when-unchanged` · P2 · new-module-spec · seam S-difftoken — identical HEAD+index token → no refetch.

**S16** (bind: `detect_agent_in_pane` new borrow signature)
- `DETECT-borrow-equiv-clone` · P2 · property-equivalence — borrowed-map result equals the old cloned-map result across inputs (oracle = current code).
- `SESSION-cached-once` · P3 · characterization — `current_session` resolved once per process.

## Phase 5 — Architecture & DRY (pins written BEFORE the refactor, using OLD code as oracle)
**S17** (bind: `rows()` vs old walks)
- `ROWS-iterator-equiv-old-walks` · P1 · property-equivalence · Today — random trees: `rows()` agrees with `visible_count`/`selected_pane`/`pane_positions`/`selected_group` at every cursor + expand mask. (Plus SCROLL-invariant-pin carried from S7.)

**S18** (bind: existing action/refresh suite)
- `APP-split-behavior-preserved` · P2 · characterization — full existing TUI suite stays green; add pins for `PreviewState`/`ProjectsState` refresh methods.

**S19** (bind: `tests/cli_tests.rs` init/add rollback)
- `PROVISION-rollback-equiv` · P1 · property-equivalence · Today — pin init **and** add rollback-on-failure (incl. `created_branch` force-delete) BEFORE extraction; helper must keep both green.

**S20** (bind: existing suites)
- `DECOMP-behavior-preserved` · P2 · characterization — source.rs/actions.rs/refresh_tree decomposition keeps existing tests green.

## Phase 6 — Test backfill
**S21** `git.rs` (tempfile bare repos)
- `GIT-delete-d-preserves-unmerged` · P0 · new-module-spec · Today.
- `GIT-delete-D-force-removes` · P1 · new-module-spec · Today.
- `GIT-remove-worktree` / `GIT-uncommitted-true-false` / `GIT-create-worktree` / `GIT-invalid-utf8-path-errs` · P2 · new-module-spec · Today.

**S22** (bind: `is_workspace`, `build_groups` via S-rootresolver)
- `WORKSPACE-2plus-children-true` / `-one-child-false` / `-own-git-false` / `-unreadable-false` · P2 · new-module-spec · seam S-rootresolver.
- `WORKSPACE-siblings-collapse-one-group` · P2 · new-module-spec · seam S-rootresolver.

**S24** (bind: `AgentResolver::resolve`, `sync`)
- `RESOLVER-precedence-pinned` · P2 · new-module-spec · Today — encode + assert the **reconciled** state-kind-vs-db-kind winner (flag the app.rs↔resolver contradiction in the fix).
- `SYNC-diverged-default-branch` · P2 · new-module-spec · Today — local default ahead of origin → assert intended FF-only/guarded behavior.

**S25** hardening specs
- `DIFF-broken-repo-not-no-changes` · P2 · new-module-spec.
- `ARG-add-branch-base-validated` / `ARG-register-url-double-dash` · P2 · new-module-spec.
- `PIDFILE-write-failure-surfaced`, `POISON-recovery-consistent`, `STATEFILE-path-hardened` · P3 · new-module-spec/characterization.

---

## Structural CI gates (invariants the behavior suite can't catch)
- **G1** — build/check fails if `[profile.release]` sets `panic = "abort"` (guards S5).
- **G2** — grep gate: no bare `#{pane_current_path}` inside a single-quoted `run-shell` string anywhere in `src/` (guards S1 regression).
- Wire into `scripts/structural-checks.sh` in CI.

## Authoring order
1. **Writable-today red-nows (no seam):** MIG-corrupt, MIG-zero-yield, MIG-valid (S2); MIG-v5-partial-rerun, MIG-v0-v5, MIG-incremental (S3); REBUILD-keeps-selected (S8); CLOSE-nonforce-preserves (S4). Confirm red.
2. **G1/G2 structural gates** (S5/S1 regression guards).
3. **Seam S-hookbuilder** → HOOK-* ; **S-gatefn** → GATE-*.
4. **Seam S-displayrows** → SCROLL-* (incl. the invariant pin).
5. **Seam S-refresh** → REFRESH-*/PROJECTS-*.
6. **Phase-4 seams** (S-ptreecache, S-boundedcache, S-construct, S-recap, S-difftoken) → their cases.
7. **Equivalence/characterization pins LAST, before their refactor:** ROWS-iterator-equiv (S17), PROVISION-rollback-equiv (S19), DETECT-borrow-equiv (S16), APP-split / DECOMP (S18/S20).
8. **Phase-6 backfill specs** (S21/S22/S24/S25).
