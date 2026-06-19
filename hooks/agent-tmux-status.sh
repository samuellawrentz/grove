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
jq --arg pane "$PANE_ID" \
   --arg state "$action" \
   --arg kind "$kind" \
   --arg cwd "${PWD:-}" \
   --argjson now "$(date +%s)" \
   '.[$pane] = { state: $state, kind: $kind, cwd: $cwd, updated: $now }' \
   "$STATE_FILE" > "${STATE_FILE}.tmp" \
  && mv "${STATE_FILE}.tmp" "$STATE_FILE"
