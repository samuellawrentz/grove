#!/usr/bin/env bash
# grove-sidebar: persistent tmux sidebar for managing all panes
#
# Event-driven: zero CPU when idle. Redraws only on USR1 or keypress.
# Shows ALL panes across ALL sessions/windows, grouped by project directory.
# Claude panes annotated with state from hook-written JSON.
#
# Usage:
#   grove-sidebar --toggle     # toggle sidebar (for Ctrl-F binding)
#
# Requires: tmux, jq (optional, for claude state)

set -uo pipefail

SELF="$(realpath "$0")"
PID_FILE="/tmp/grove-sidebar.pid"
PANE_ID_FILE="/tmp/grove-sidebar.pane"
SIDEBAR_WIDTH=35
STATE_FILE="/tmp/claude-panes.json"

# --- Toggle mode (called by Ctrl-F keybinding) ---

if [[ "${1:-}" == "--toggle" ]]; then
  if [[ -f "$PID_FILE" ]]; then
    OLD_PID=$(cat "$PID_FILE" 2>/dev/null || true)
    if [[ -n "$OLD_PID" ]] && kill -0 "$OLD_PID" 2>/dev/null; then
      # Sidebar exists → kill it
      SIDEBAR_PANE=$(cat "$PANE_ID_FILE" 2>/dev/null || true)
      [[ -n "$SIDEBAR_PANE" ]] && tmux kill-pane -t "$SIDEBAR_PANE" 2>/dev/null || true
      kill "$OLD_PID" 2>/dev/null || true
      rm -f "$PID_FILE" "$PANE_ID_FILE"
      exit 0
    fi
    rm -f "$PID_FILE" "$PANE_ID_FILE"
  fi
  # No sidebar → create in current window
  tmux split-window -hbdl "$SIDEBAR_WIDTH" "$SELF"
  exit 0
fi

# --- Hooks ---

HOOK_NAMES=(after-split-window pane-exited window-pane-changed session-window-changed after-kill-pane)

install_hooks() {
  echo $$ > "$PID_FILE"
  # Store our pane id for toggle detection
  tmux display-message -p '#{pane_id}' > "$PANE_ID_FILE" 2>/dev/null || true
  for hook in "${HOOK_NAMES[@]}"; do
    tmux set-hook -g "grove-sb-${hook}" "${hook}" \
      "run-shell -b 'kill -USR1 $$ 2>/dev/null || true'" 2>/dev/null || true
  done
}

cleanup() {
  for hook in "${HOOK_NAMES[@]}"; do
    tmux set-hook -gu "grove-sb-${hook}" 2>/dev/null || true
  done
  rm -f "$PID_FILE" "$PANE_ID_FILE"
  tput cnorm 2>/dev/null || true
  tput rmcup 2>/dev/null || true
  stty sane 2>/dev/null || true
}

# --- Data ---

declare -A DIR_PANES
declare -a DIR_ORDER=()
declare -A DIR_COLLAPSED
declare -a NAV_ITEMS=()
declare -a NAV_DISPLAY=()
CURSOR=0
MY_PANE_ID=""

collect_panes() {
  DIR_PANES=()
  DIR_ORDER=()
  MY_PANE_ID=$(tmux display-message -p '#{pane_id}' 2>/dev/null || true)

  while IFS=$'\t' read -r pane_id session_name window_index pane_index pane_pid pane_cwd; do
    [[ "$pane_id" == "$MY_PANE_ID" ]] && continue

    local dir_name icon label state proc
    dir_name=$(basename "$pane_cwd")
    local target="${session_name}:${window_index}.${pane_index}"

    if pgrep -P "$pane_pid" -f "claude" >/dev/null 2>&1; then
      state=$(jq -r --arg p "$pane_id" '.[$p].state // "active"' "$STATE_FILE" 2>/dev/null || echo "active")
      case "$state" in
        waiting) icon="◉" ;; active) icon="●" ;; *) icon="○" ;;
      esac
      label="$target claude [$state]"
    else
      proc=$(ps -o comm= -p "$pane_pid" 2>/dev/null || echo "shell")
      icon="·"
      label="$target $proc"
    fi

    if [[ -z "${DIR_PANES[$dir_name]+x}" ]]; then
      DIR_ORDER+=("$dir_name")
      DIR_PANES[$dir_name]=""
    fi
    [[ -n "${DIR_PANES[$dir_name]}" ]] && DIR_PANES[$dir_name]+=$'\n'
    DIR_PANES[$dir_name]+="${pane_id}"$'\t'"${icon}"$'\t'"${label}"
  done < <(tmux list-panes -a -F '#{pane_id}	#{session_name}	#{window_index}	#{pane_index}	#{pane_pid}	#{pane_current_path}' 2>/dev/null)
}

build_nav_list() {
  NAV_ITEMS=()
  NAV_DISPLAY=()

  for dir in "${DIR_ORDER[@]}"; do
    local collapsed="${DIR_COLLAPSED[$dir]:-0}"
    local arrow="▼"; [[ "$collapsed" == "1" ]] && arrow="▶"

    NAV_ITEMS+=("dir:$dir")
    NAV_DISPLAY+=("$arrow $dir/")

    if [[ "$collapsed" != "1" ]]; then
      while IFS=$'\t' read -r pane_id icon label; do
        [[ -z "$pane_id" ]] && continue
        NAV_ITEMS+=("pane:$pane_id")
        NAV_DISPLAY+=("  $icon $label")
      done <<< "${DIR_PANES[$dir]}"
    fi
  done

  # Clamp cursor
  local max=$(( ${#NAV_ITEMS[@]} - 1 ))
  (( max < 0 )) && max=0
  (( CURSOR > max )) && CURSOR=$max
}

# --- Render ---

render() {
  local cols rows
  cols=$(tput cols 2>/dev/null || echo 35)
  rows=$(tput lines 2>/dev/null || echo 24)

  tput clear
  tput cup 0 0

  printf '\033[1;35m Sessions \033[0m\n'
  printf '\033[2m'
  printf '─%.0s' $(seq 1 "$((cols - 1))")
  printf '\033[0m\n'

  local visible=$((rows - 4))
  local total=${#NAV_ITEMS[@]}

  if (( total == 0 )); then
    printf '\033[2m  No panes found\033[0m\n'
    tput cup $((rows - 1)) 0
    printf '\033[2mCtrl-F:close\033[0m'
    return
  fi

  # Scroll window
  local scroll=0
  (( CURSOR >= visible )) && scroll=$((CURSOR - visible + 1))

  local i
  for (( i = scroll; i < total && i < scroll + visible; i++ )); do
    local display="${NAV_DISPLAY[$i]:0:$((cols - 2))}"
    if (( i == CURSOR )); then
      printf '\033[7m%-*s\033[0m\n' "$((cols - 1))" "$display"
    else
      printf '%s\n' "$display"
    fi
  done

  tput cup $((rows - 1)) 0
  printf '\033[2mj/k:nav  enter:focus  h/l:fold  q:quit\033[0m'
}

# --- Actions ---

focus_pane() {
  local item="${NAV_ITEMS[$CURSOR]:-}"
  [[ "$item" != pane:* ]] && return
  local pane_id="${item#pane:}"
  local session window_index
  session=$(tmux display-message -t "$pane_id" -p '#{session_name}' 2>/dev/null || true)
  window_index=$(tmux display-message -t "$pane_id" -p '#{window_index}' 2>/dev/null || true)
  [[ -z "$session" || -z "$window_index" ]] && return
  local my_session
  my_session=$(tmux display-message -p '#{session_name}' 2>/dev/null || true)
  # Switch session if needed, then window, then pane
  [[ "$session" != "$my_session" ]] && tmux switch-client -t "$session" 2>/dev/null || true
  tmux select-window -t "${session}:${window_index}" 2>/dev/null || true
  tmux select-pane -t "$pane_id" 2>/dev/null || true
  # Exit — cleanup trap removes hooks/PID files, tmux kills the pane on process exit
  exit 0
}

toggle_fold() {
  local item="${NAV_ITEMS[$CURSOR]:-}"
  [[ "$item" != dir:* ]] && return
  local dir="${item#dir:}"
  if [[ "${DIR_COLLAPSED[$dir]:-0}" == "1" ]]; then
    DIR_COLLAPSED[$dir]=0
  else
    DIR_COLLAPSED[$dir]=1
  fi
  build_nav_list
  render
}

# --- Main ---

NEEDS_REDRAW=0
trap_usr1() { NEEDS_REDRAW=1; }

main() {
  tput smcup
  tput civis
  stty -echo -icanon 2>/dev/null || true
  install_hooks
  trap cleanup EXIT
  trap trap_usr1 USR1

  collect_panes
  build_nav_list
  render

  while true; do
    if (( NEEDS_REDRAW )); then
      NEEDS_REDRAW=0
      local old_item="${NAV_ITEMS[$CURSOR]:-}"
      collect_panes
      build_nav_list
      # Preserve cursor on same item
      if [[ -n "$old_item" ]]; then
        for (( i = 0; i < ${#NAV_ITEMS[@]}; i++ )); do
          [[ "${NAV_ITEMS[$i]}" == "$old_item" ]] && CURSOR=$i && break
        done
      fi
      render
    fi

    local key="" rc=0
    IFS= read -rsn1 -t 1 key || rc=$?
    (( rc != 0 )) && continue

    case "$key" in
      j) (( CURSOR < ${#NAV_ITEMS[@]} - 1 )) && (( CURSOR++ )) || true; render ;;
      k) (( CURSOR > 0 )) && (( CURSOR-- )) || true; render ;;
      h|l) toggle_fold ;;
      "") focus_pane ;;  # Enter — switch to pane, sidebar stays alive
      q) exit 0 ;;
    esac
  done
}

main
