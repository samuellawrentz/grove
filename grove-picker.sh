#!/usr/bin/env bash
# grove-picker: tmux floating popup for managing Claude Code sessions
#
# Works standalone — no grove CLI required.
# Finds all tmux panes running Claude, shows state, allows interaction.
#
# Usage:
#   grove-picker                    # launch popup (must be inside tmux)
#   tmux bind-key C-f run-shell "grove-picker"  # bind to key
#
# Requires: tmux, fzf, zoxide

set -euo pipefail

SELF="$(realpath "$0")"
POPUP_WIDTH="95%"
POPUP_HEIGHT="95%"
POLL_LINES=50
STATE_LINES=10
STATE_FILE="/tmp/claude-panes.json"

# --- Claude state detection ---

# Read state from shared hook-written JSON (set by claude-tmux-status.sh)
detect_claude_state() {
  local pane_id="$1"
  jq -r --arg p "$pane_id" '.[$p].state // "active"' "$STATE_FILE" 2>/dev/null || echo "active"
}

get_pane_preview() {
  local pane_id="$1"
  tmux capture-pane -t "$pane_id" -p -e -S -"$POLL_LINES" 2>/dev/null || echo "(no content)"
}

# --- Find Claude panes ---

find_claude_panes() {
  while IFS=$'\t' read -r pane_id session_name window_index pane_index pane_title pane_pid pane_cwd; do
    # Single pgrep check per pane
    if pgrep -P "$pane_pid" -f "claude" >/dev/null 2>&1; then
      local state
      state=$(detect_claude_state "$pane_id")
      local icon
      case "$state" in
        waiting) icon="◉" ;;
        active)  icon="●" ;;
        *)       icon="○" ;;
      esac
      local dir_name
      dir_name=$(basename "$pane_cwd")
      # Include session:window.pane target for direct switching
      printf '%s\t%s %s:%s.%s  %s  [%s]  %s\n' \
        "$pane_id" "$icon" "$session_name" "$window_index" "$pane_index" \
        "$dir_name" "$state" "${pane_title:-}"
    fi
  done < <(tmux list-panes -a -F '#{pane_id}	#{session_name}	#{window_index}	#{pane_index}	#{pane_title}	#{pane_pid}	#{pane_current_path}' 2>/dev/null)
}

# --- Actions ---

action_preview() {
  local pane_id="$1"
  echo "═══ Pane: $pane_id ═══"
  echo ""
  get_pane_preview "$pane_id"
}

action_accept() {
  local pane_id="$1"
  tmux send-keys -t "$pane_id" Enter
}

action_reject() {
  local pane_id="$1"
  tmux send-keys -t "$pane_id" "n" Enter
}

action_send_prompt() {
  local pane_id="$1"
  local prompt="$2"
  if [[ -n "$prompt" ]]; then
    tmux send-keys -t "$pane_id" -l "$prompt"
    tmux send-keys -t "$pane_id" Enter
  fi
}

action_kill() {
  local pane_id="$1"
  # Send SIGTERM to the claude process, then kill the pane
  local pane_pid
  pane_pid=$(tmux display-message -t "$pane_id" -p '#{pane_pid}' 2>/dev/null)
  if [[ -n "$pane_pid" ]]; then
    # Kill claude child processes first
    pkill -TERM -P "$pane_pid" -f "claude" 2>/dev/null || true
    sleep 0.3
    tmux kill-pane -t "$pane_id" 2>/dev/null || true
  fi
}

action_switch() {
  local pane_id="$1"
  # Get the session:window.pane target from the pane_id
  local target
  target=$(tmux display-message -t "$pane_id" -p '#{session_name}:#{window_index}.#{pane_index}' 2>/dev/null)
  if [[ -n "$target" ]]; then
    tmux switch-client -t "$target"
  fi
}

action_new_session() {
  local dir="$1"
  if [[ -n "$dir" && -d "$dir" ]]; then
    local session_name
    session_name=$(tmux display-message -p '#{session_name}' 2>/dev/null)
    tmux new-window -t "$session_name" -c "$dir" "claude --dangerously-skip-permissions"
  fi
}

# --- Dispatch ---

# Called by fzf --bind execute() for actions
if [[ "${1:-}" == "--action" ]]; then
  action="$2"
  pane_id="$3"
  shift 3
  case "$action" in
    accept)  action_accept "$pane_id" ;;
    reject)  action_reject "$pane_id" ;;
    send)    action_send_prompt "$pane_id" "$*" ;;
    preview) action_preview "$pane_id" ;;
    switch)  action_switch "$pane_id" ;;
    kill)    action_kill "$pane_id" ;;
  esac
  exit 0
fi

# Called by fzf for the preview window
if [[ "${1:-}" == "--preview" ]]; then
  pane_id="$2"
  action_preview "$pane_id"
  exit 0
fi

# Called for the send-prompt inline
if [[ "${1:-}" == "--send-popup" ]]; then
  pane_id="$2"
  printf "Send to %s\n" "$pane_id"
  printf "─────────────────────────────\n"
  printf "Type your prompt (Enter to send, Ctrl-C to cancel):\n\n"
  read -r -e prompt
  if [[ -n "$prompt" ]]; then
    action_send_prompt "$pane_id" "$prompt"
    echo ""
    echo "Sent."
    sleep 0.5
  fi
  exit 0
fi

# New session: zoxide directory picker → spawn claude in new tmux window
if [[ "${1:-}" == "--new-session" ]]; then
  DIR=$(zoxide query -l | fzf \
    --prompt="directory › " \
    --header="Pick a directory to start Claude in" \
    --header-first \
    --preview="ls -la --color=always {}" \
    --preview-window="right:60%:wrap" \
    || true)
  if [[ -n "$DIR" ]]; then
    action_new_session "$DIR"
  fi
  exit 0
fi

# Reload: re-scan panes and output for fzf
if [[ "${1:-}" == "--reload" ]]; then
  find_claude_panes
  exit 0
fi

# --- Inner mode: the fzf picker ---

if [[ "${GROVE_PICKER_INNER:-}" == "1" ]]; then
  PANES=$(find_claude_panes)

  if [[ -z "$PANES" ]]; then
    echo "No Claude sessions found in tmux."
    echo ""
    echo "Start Claude Code in a tmux pane and try again."
    sleep 2
    exit 0
  fi

  # Background process to auto-refresh the preview every 1s via fzf --listen
  FZF_PORT=$((10000 + RANDOM % 50000))
  (
    sleep 0.5  # wait for fzf to start
    while true; do
      curl -s "localhost:$FZF_PORT" -d 'refresh-preview' >/dev/null 2>&1 || break
      sleep 1
    done
  ) &
  REFRESH_PID=$!
  trap "kill $REFRESH_PID 2>/dev/null" EXIT

  # fzf with keybindings
  # Field 1 is pane_id (tab-separated), field 2+ is display
  SELECTED=$(echo "$PANES" | fzf \
    --listen "$FZF_PORT" \
    --ansi \
    --no-multi \
    --delimiter=$'\t' \
    --with-nth=2 \
    --header="enter:switch  ctrl-y:accept  ctrl-r:reject  ctrl-x:kill  ctrl-n:new  ctrl-p:send query  ctrl-l:refresh" \
    --header-first \
    --prompt="› " \
    --preview="$SELF --preview {1}" \
    --preview-window="right:60%:wrap:follow" \
    --bind="ctrl-y:execute-silent($SELF --action accept {1})+reload($SELF --reload)" \
    --bind="ctrl-r:execute-silent($SELF --action reject {1})+reload($SELF --reload)" \
    --bind="ctrl-x:execute-silent($SELF --action kill {1})+reload($SELF --reload)" \
    --bind="ctrl-n:become($SELF --new-session)" \
    --bind="ctrl-p:execute($SELF --send-popup {1})+reload($SELF --reload)" \
    --bind="ctrl-l:reload($SELF --reload)" \
    --bind="j:down,k:up" \
    || true)

  kill $REFRESH_PID 2>/dev/null || true

  if [[ -z "$SELECTED" ]]; then
    exit 0
  fi

  # Extract pane_id (first tab-separated field)
  PANE_ID=$(echo "$SELECTED" | cut -f1)
  action_switch "$PANE_ID"
  exit 0
fi

# --- Outer invocation: launch the popup ---

if [[ -z "${TMUX:-}" ]]; then
  echo "grove-picker must be run inside tmux"
  exit 1
fi

export GROVE_PICKER_INNER=1
exec tmux display-popup \
  -w "$POPUP_WIDTH" \
  -h "$POPUP_HEIGHT" \
  -T " Claude Sessions " \
  -E "$SELF"
