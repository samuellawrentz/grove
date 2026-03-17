#!/usr/bin/env bash
# grove-picker: tmux floating popup for managing Claude Code sessions
#
# Works standalone — no grove CLI required.
# Finds all tmux panes running Claude, shows state, allows interaction.
#
# Usage:
#   grove-picker                    # launch popup (must be inside tmux)
#   tmux bind-key g run-shell "grove-picker"   # bind to key
#
# Requires: tmux, fzf

set -euo pipefail

SELF="$(realpath "$0")"
POPUP_WIDTH="80%"
POPUP_HEIGHT="70%"
POLL_LINES=50
STATE_LINES=10

# --- Claude state detection ---

# Detect state from already-captured content (no extra tmux calls)
detect_claude_state() {
  local content="$1"

  if [[ -z "$content" ]]; then
    echo "active"
    return
  fi

  # Check for waiting patterns (permission prompts)
  if echo "$content" | grep -qiE '(\(y/n\)|\(Y/n\)|Allow this action\?|Do you want to (proceed|continue|allow)|Press Enter to confirm|Approve\?|\[Y/n\]|\[yes/no\]|Want me to)'; then
    echo "waiting"
    return
  fi

  echo "active"
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
      # Single capture for state detection
      local content
      content=$(tmux capture-pane -t "$pane_id" -p -S -"$STATE_LINES" 2>/dev/null || echo "")
      local state
      state=$(detect_claude_state "$content")
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
  tmux send-keys -t "$pane_id" "y" Enter
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

action_switch() {
  local pane_id="$1"
  # Get the session:window.pane target from the pane_id
  local target
  target=$(tmux display-message -t "$pane_id" -p '#{session_name}:#{window_index}.#{pane_index}' 2>/dev/null)
  if [[ -n "$target" ]]; then
    tmux switch-client -t "$target"
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

  # fzf with keybindings
  # Field 1 is pane_id (tab-separated), field 2+ is display
  SELECTED=$(echo "$PANES" | fzf \
    --ansi \
    --no-multi \
    --delimiter=$'\t' \
    --with-nth=2 \
    --header="enter:switch  ctrl-a:accept  ctrl-r:reject  ctrl-p:send query  ctrl-l:refresh" \
    --header-first \
    --prompt="› " \
    --preview="$SELF --preview {1}" \
    --preview-window="right:60%:wrap:follow" \
    --bind="ctrl-a:execute-silent($SELF --action accept {1})+reload($SELF --reload)" \
    --bind="ctrl-r:execute-silent($SELF --action reject {1})+reload($SELF --reload)" \
    --bind="ctrl-p:execute($SELF --send-popup {1})+reload($SELF --reload)" \
    --bind="ctrl-l:reload($SELF --reload)" \
    || true)

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
