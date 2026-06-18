#!/usr/bin/env bash
#
# Structural CI gates that the behavior test suite CANNOT catch (TEST_PLAN.md §7).
# Each guards an invariant whose violation still leaves every behavior test green,
# so it must be enforced mechanically rather than by reviewer memory.
#
#   G1  the StepJournal holds no DB-inverse closure (DB rollback is owned solely
#       by Db::transaction; a DELETE/upsert compensation in the journal would
#       re-introduce the non-atomic delete Step 1 removed).
#   G2  the raw pane_agents DB methods are sealed behind PaneAgentStore.
#   G3  the flat-row cursor math has exactly one copy (the FlatRows trait).
#
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
ok()   { printf '  \033[32mok\033[0m   %s\n' "$1"; }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=1; }

# ── G1: journal is fs/git/tmux only ──────────────────────────────────────────
# Match the DB-inverse tokens on CODE lines only (a `N:` line whose content is a
# `//` comment is excluded — the module doc legitimately names the rule).
g1_hits=$(grep -nE 'delete_task|delete_repo|delete_pane_agent|upsert_|DELETE FROM|INSERT INTO' \
        src/commands/rollback.rs 2>/dev/null | grep -vE ':[[:space:]]*//' || true)
if [ -n "$g1_hits" ]; then
    bad "G1: DB-inverse operation in src/commands/rollback.rs (journal must be fs/git/tmux only)"
    printf '%s\n' "$g1_hits" | sed 's/^/       /'
else
    ok "G1: journal holds no DB-inverse closure"
fi

# ── G2a: raw pane_agents methods are not bare `pub` ───────────────────────────
if grep -nE '^[[:space:]]*pub fn (record_pane_agent|list_pane_agents|delete_pane_agent)' \
        src/db.rs >/dev/null 2>&1; then
    bad "G2: a raw pane_agents method is bare 'pub' in src/db.rs (must be pub(crate))"
else
    ok "G2a: raw pane_agents methods are sealed (pub(crate), not pub)"
fi

# ── G2b: raw pane_agents methods called only inside the store (agent.rs/db.rs) ─
leaks=$(grep -rnE '\.(record_pane_agent|list_pane_agents|delete_pane_agent)\(' \
        src --include='*.rs' 2>/dev/null | grep -vE '^src/(agent\.rs|db\.rs):' || true)
if [ -n "$leaks" ]; then
    bad "G2: raw pane_agents method called outside PaneAgentStore:"
    printf '%s\n' "$leaks" | sed 's/^/       /'
else
    ok "G2b: no raw pane_agents calls outside PaneAgentStore"
fi

# ── G3: flat-row move arithmetic single-sourced in flat_rows.rs ───────────────
if grep -nE 'fn move_(down|up)_by' src/tui/source.rs >/dev/null 2>&1; then
    bad "G3: cursor move arithmetic re-defined in src/tui/source.rs (must reuse FlatRows)"
else
    ok "G3: flat-row move arithmetic is single-sourced in flat_rows.rs"
fi

if [ "$fail" -ne 0 ]; then
    printf '\n\033[31mstructural checks FAILED\033[0m\n'
    exit 1
fi
printf '\n\033[32mall structural checks passed\033[0m\n'
