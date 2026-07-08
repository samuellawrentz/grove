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

## License

MIT
