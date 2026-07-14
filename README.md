# Grove

A CLI tool for managing multi-repo workspaces in AI-assisted development. Grove eliminates the manual overhead of cross-repository feature work — cloning repos, creating branches, setting up tmux sessions, and launching Claude Code instances.

## Core Concepts

- **Bare clone registry** — shared git object store for instant worktree creation without network clones
- **Tasks** — directories grouping N worktrees from N repos, each on a task-specific branch, with a shared `CONTEXT.md`
- **Sessions** — tmux sessions with one pane per repo, optionally running Claude Code

## Install

### Prebuilt binary (recommended)

Download the latest release for your platform and drop it on your `PATH`:

```bash
# macOS (Apple Silicon)
curl -L https://github.com/samuellawrentz/grove/releases/latest/download/grove-macos-arm64.tar.gz | tar xz
sudo mv grove /usr/local/bin/

# macOS (Intel)
curl -L https://github.com/samuellawrentz/grove/releases/latest/download/grove-macos-x86_64.tar.gz | tar xz
sudo mv grove /usr/local/bin/

# Linux (x86_64)
curl -L https://github.com/samuellawrentz/grove/releases/latest/download/grove-linux-x86_64.tar.gz | tar xz
sudo mv grove /usr/local/bin/

grove --version
```

On macOS, if Gatekeeper blocks the binary: `xattr -d com.apple.quarantine /usr/local/bin/grove`.

### From source

```bash
cargo install --git https://github.com/samuellawrentz/grove   # no clone needed
# or, from a local checkout:
cargo install --path .
```

## Usage

### Register repos

```bash
grove register plivo-api git@github.com:org/plivo-api.git
grove register plivo-web git@github.com:org/plivo-web.git
```

### Create a task

```bash
# Explicit repos and branch
grove init add-billing plivo-api plivo-web --branch feat/billing

# Interactive mode
grove init add-billing -i
```

This creates worktrees for each repo on the task branch, generates a `CONTEXT.md`, and (by default) opens a tmux session with Claude Code in each pane.

### Manage tasks

```bash
grove list                    # list active tasks
grove close add-billing       # clean up worktrees and session
grove close add-billing --force  # close even with uncommitted changes
```

### Sync repos

```bash
grove sync          # fetch all repos (parallel)
grove sync plivo-api  # fetch one repo
grove repos         # list registered repos
```

### Drive an agent

Grove can drive the agent inside a task's pane, so a task is scriptable without giving up the tmux window a human can attach to and take over at any moment.

```bash
grove send add-billing "run the tests"     # type + Enter, return immediately
grove wait add-billing                     # block until the turn ends
grove read add-billing                     # print what the agent last said
grove run  add-billing "run the tests"     # all three, in one call
```

`read` parses the agent's JSONL transcript (`~/.claude/projects/…`), not the screen. `capture-pane` shows a TUI mid-redraw — ANSI noise, wrapped lines, scrollback that has already dropped the interesting part. The transcript is structured and complete.

| Flag | On | Description |
|------|-----|-------------|
| `--brief` | `send`, `run` | Ask the agent to end its turn with a ≤5-line summary |
| `--any` | `wait` | Return when the *first* task finishes, not all of them |
| `--timeout <secs>` | `wait`, `run` | Give up and exit 9 (default 1800) |
| `--turns <n>` | `read` | How many trailing agent turns to show (default 1) |
| `--tools` | `read`, `run` | Annotate turns with the tools the agent called |
| `--full` | `read` | Include tool calls and results too — expensive |
| `--max-chars <n>` | `read`, `run` | Cap output; `0` for no cap (default 4000) |

Requires the [status hook](#agent-status-hook) — that is what tells grove when a turn ends and where the transcript lives.

### Orchestrating from an agent

When the caller is itself an agent, every command it runs costs a tool-call round trip and everything it prints lands in a context window. Grove is built so a fleet of tasks costs very little of either:

```bash
grove run  refactor-api "migrate the handlers to the new router" --brief --json
grove wait api web docs --any --json     # whichever finishes first
```

- **Never poll.** `grove status` in a loop costs a round trip every few seconds for as long as the agent works. `wait` blocks and costs exactly one, however long the turn takes. `--any` extends that to a fleet: N tasks cost ~N calls total.
- **`run` over `send` + `wait` + `read`.** Three round trips collapse into one.
- **Reads are lean by default.** Only the agent's final message comes back; tool calls and their results — the bulk of any transcript — are dropped unless you ask for them. `--max-chars` caps what a runaway agent can push into your context.
- **`--brief`** makes that final message *be* the report, so the default read is all you ever need.

### Global flags

| Flag | Description |
|------|-------------|
| `--json` | Structured JSON output |
| `--verbose` | Show git commands and exit codes |
| `--config <path>` | Custom config file path |

## Configuration

Grove uses `~/.grove/config.json` (all fields optional, sensible defaults apply):

```json
{
  "repos_dir": "~/repos",
  "tasks_dir": "~/tasks",
  "max_parallel_syncs": 8,
  "auto_launch_claude": true,
  "claude_command": "claude",
  "tmux": { "layout": "even-vertical", "session_prefix": "grove" },
  "git": { "fetch_prune": true, "clone_retries": 3 }
}
```

Environment variable overrides: `GROVE_CONFIG`, `GROVE_REPOS_DIR`, `GROVE_TASKS_DIR`, `GROVE_JSON`.

## Agent status hook

`hooks/agent-tmux-status.sh` is how grove knows what an agent is doing. It writes one entry per tmux pane — state, agent kind, cwd, and the path to the session transcript — and everything that reads agent state (`list`, `status`, `wait`, `read`, `run`, the TUI) is a view over that file.

Without it, `grove list` still works, but agent state reads as `unknown` and `wait`/`run` have no finish line to wait for.

Wire it into `~/.claude/settings.json`, one invocation per lifecycle event:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      { "hooks": [{ "type": "command", "command": "~/src/personal/grove/hooks/agent-tmux-status.sh claude active" }] }
    ],
    "Notification": [
      { "hooks": [{ "type": "command", "command": "~/src/personal/grove/hooks/agent-tmux-status.sh claude waiting" }] }
    ],
    "Stop": [
      { "hooks": [{ "type": "command", "command": "~/src/personal/grove/hooks/agent-tmux-status.sh claude idle" }] }
    ],
    "SessionEnd": [
      { "hooks": [{ "type": "command", "command": "~/src/personal/grove/hooks/agent-tmux-status.sh claude cleanup" }] }
    ]
  }
}
```

Requires `jq`, and only records anything when running inside tmux (`$TMUX_PANE`). Claude pipes each event a JSON payload containing `transcript_path`; the hook stores it, which is what lets `grove read` quote the agent instead of scraping its screen. Other agents (`codex`, `opencode`, `cursor`) can call the same script with their own kind — they just won't get transcript reads unless they pass a compatible payload, and `read` then falls back to finding the transcript by convention.

State file location: `$GROVE_STATE_FILE`, else `$XDG_RUNTIME_DIR/grove/claude-panes.json`, else `$TMPDIR/grove-$USER/claude-panes.json`.

## grove-picker

A standalone tmux floating popup (`grove-picker.sh`) for managing Claude Code sessions across panes. No grove CLI required — just tmux, fzf, and zoxide.

```bash
# Launch directly
./grove-picker.sh

# Bind to Ctrl-G in tmux
tmux bind-key C-g run-shell "path/to/grove-picker.sh"
```

**Key bindings in the picker:**

| Key | Action |
|-----|--------|
| `Enter` | Switch to pane |
| `Ctrl-Y` | Accept (send Enter to Claude) |
| `Ctrl-R` | Reject (send `n` + Enter) |
| `Ctrl-X` | Kill pane |
| `Ctrl-N` | Spawn new Claude session |
| `Ctrl-P` | Send custom prompt |

## Raycast extension

A Raycast extension in `raycast/` wraps the CLI — list/create tasks, browse repos, and sync, all from Raycast.

The extension isn't on the Raycast Store yet, so install it as a local dev extension:

1. Install the `grove` CLI (see [Install](#install)).
2. Get the extension source (it lives in the `raycast/` folder of this repo).
3. Build and import it:
   ```bash
   cd raycast
   npm install
   npm run dev        # imports into Raycast and starts dev mode
   ```
   Raycast picks up the extension while `npm run dev` runs. To keep it after stopping, use **Import Extension** in Raycast and point it at the `raycast/` folder.
4. Open Raycast → **Grove** commands. In the extension preferences, set **Grove Binary Path** to your `grove` location (e.g. `/usr/local/bin/grove` or `~/.cargo/bin/grove`) and pick your **Terminal App**.

## JSON Output Contract

```
Success: { "ok": true, ...fields }
Error:   { "ok": false, "error": "<code>", "message": "<human>", "exit_code": N }
```

Errors are distinguishable by exit code, so a script can branch without parsing prose: `1` general, `2` task not found, `3` repo not registered, `4` tmux not running, `5` uncommitted changes, `6` conflict, `7` tui, `8` database, `9` timeout.

A timeout (`9`) from `wait`/`run` means the agent is *still working*, not that anything failed — the task is untouched and can be waited on again.

## License

MIT
