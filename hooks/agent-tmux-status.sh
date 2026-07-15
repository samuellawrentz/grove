#!/usr/bin/env bash
# Generalized agent status hook — writes per-pane state for grove to read.
#
# Usage: agent-tmux-status.sh <kind> <state>
#   kind:  claude | codex | cursor | opencode
#   state: active | waiting | idle | cleanup
#
# Wire one invocation per agent lifecycle event:
#   - waiting : agent is blocked on user input / approval prompt
#   - active  : agent is working (turn started)
#   - idle    : agent finished its turn (grove treats this as "not running")
#   - cleanup : remove this pane's entry (session end)
#
# Requires: jq, and $TMUX_PANE set by tmux.
# State file path overridable via $GROVE_STATE_FILE. The default is user-scoped
# (never a fixed world-writable /tmp name another user could pre-create) and
# must match `state_file_path()` in src/agent.rs:
#   1. $GROVE_STATE_FILE  2. $XDG_RUNTIME_DIR/grove/...  3. /tmp/grove-<user>/...

set -euo pipefail

if [[ -n "${GROVE_STATE_FILE:-}" ]]; then
  STATE_FILE="$GROVE_STATE_FILE"
elif [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
  STATE_FILE="${XDG_RUNTIME_DIR}/grove/claude-panes.json"
else
  STATE_FILE="${TMPDIR:-/tmp}/grove-${USER:-unknown}/claude-panes.json"
fi
mkdir -p "$(dirname "$STATE_FILE")"
PANE_ID="${TMUX_PANE:-}"

# Not running inside tmux — nothing to track.
[[ -z "$PANE_ID" ]] && exit 0

kind="${1:-claude}"
action="${2:-active}"

# Claude pipes a JSON payload on stdin containing `transcript_path` — the exact
# JSONL file for this session. Recording it is what lets `grove read` quote the
# agent's own words instead of scraping the redrawn TUI out of capture-pane.
# Agents that pass nothing (or a tty) just leave it empty; the read path then
# falls back to deriving the path from cwd by convention.
TRANSCRIPT=""
if [[ ! -t 0 ]]; then
  payload="$(cat 2>/dev/null || true)"
  if [[ -n "$payload" ]]; then
    TRANSCRIPT="$(jq -r '.transcript_path // empty' <<<"$payload" 2>/dev/null || true)"
  fi
fi

# Serialize concurrent updates from sibling panes. `mkdir` is atomic on all
# POSIX filesystems, so it works as a lock even where `flock` is absent (macOS).
LOCK_DIR="${STATE_FILE}.lock"
for _ in $(seq 1 50); do
  if mkdir "$LOCK_DIR" 2>/dev/null; then
    trap 'rmdir "$LOCK_DIR" 2>/dev/null' EXIT
    break
  fi
  sleep 0.05
done

# Ensure the file exists with valid JSON (inside the lock).
[[ -s "$STATE_FILE" ]] || echo '{}' > "$STATE_FILE"

if [[ "$action" == "cleanup" ]]; then
  jq --arg pane "$PANE_ID" 'del(.[$pane])' "$STATE_FILE" > "${STATE_FILE}.tmp" \
    && mv "${STATE_FILE}.tmp" "$STATE_FILE"
  exit 0
fi

# Upsert this pane's state, tagged with the agent kind and a timestamp
# (grove expires entries older than its TTL so dead panes drop off).
# An event that carries no transcript path must not erase one we already have:
# only some lifecycle events ship a payload, but the session is the same one.
jq --arg pane "$PANE_ID" \
   --arg state "$action" \
   --arg kind "$kind" \
   --arg cwd "${PWD:-}" \
   --arg transcript "$TRANSCRIPT" \
   --argjson now "$(date +%s)" \
   '.[$pane] = {
      state: $state,
      kind: $kind,
      cwd: $cwd,
      transcript: (if $transcript == "" then (.[$pane].transcript // "") else $transcript end),
      updated: $now
    }' \
   "$STATE_FILE" > "${STATE_FILE}.tmp" \
  && mv "${STATE_FILE}.tmp" "$STATE_FILE"
