---
name: grove
description: Use grove, a Rust CLI for multi-repo git-worktree tasks that also drives coding agents (Claude Code, Codex, OpenCode, Cursor) inside tmux — send a prompt, block until the turn ends, read the reply. Use when managing per-task worktrees across repos, running an agent per task, or orchestrating agents programmatically (send/wait/read/run), or when the user mentions grove, worktree tasks, or agent orchestration over tmux.
---

# grove — Multi-Repo Worktree & Agent Orchestration

grove is a CLI-first, single-binary tool written in Rust. It creates one git worktree per repo for a task, opens a tmux window per task with a coding agent in each pane, and can **drive** that agent — send it a prompt, block until it finishes, and read back what it said — without giving up the tmux window a human can attach to and take over.

## When to Use This Skill

- User wants isolated worktrees across one or more repos for a feature/task
- User wants to run a coding agent (Claude Code, Codex, OpenCode, Cursor) per task
- User wants to drive an agent programmatically: send a prompt, wait for it to finish, read the reply
- User is orchestrating a fleet of agents and wants it token-efficient
- User mentions grove, worktree tasks, `grove init/send/wait/read/run`, or agent orchestration over tmux

## Installation

```bash
# Prebuilt binary (macOS/Linux)
curl -L https://github.com/samuellawrentz/grove/releases/latest/download/grove-macos-arm64.tar.gz | tar xz
sudo mv grove /usr/local/bin/

# Or from source
cargo install --git https://github.com/samuellawrentz/grove.git grove
```

Single binary. Needs `git` and `tmux`; `jq` for the status hook. `grove --version` to verify.

## Core Commands

### Register repos (once)

```bash
grove register <name> <git-url>     # bare clone into the repos dir
grove repos                         # list registered repos
grove sync [<repo>]                 # fetch all, or one
```

A registered repo is a bare clone, so new worktrees cost no network clone.

### Create a task

```bash
# Explicit repos + branch
grove init add-billing api web --branch feat/billing

# Interactive (pick repos + branch)
grove init add-billing -i

# Non-interactive, no agent/tmux (e.g. from inside another agent session)
grove init add-billing api --no-attach --no-tmux --no-agent --json
```

Creates a worktree per repo at `~/tasks/<task>/<repo>/` on a shared branch, a `CONTEXT.md` at the task root (seed with `--context "<text>"`), and by default a tmux window with an agent per pane.

### Manage tasks

```bash
grove list                          # active tasks
grove status [<task>]               # tasks + live agent state
grove attach <task>                 # jump to the task's tmux window
grove add <task> <repo>             # add a repo's worktree to an existing task
grove close <task> [--force]        # remove worktrees; --force discards uncommitted work
```

A repo goes into a task once by default. To hold a *second* worktree of a repo the task already has (e.g. two PRs of one repo on the same ticket), name its directory explicitly:

```bash
grove add <task> <repo> --branch pr-72 --dir <repo>-pr72   # worktree at ~/tasks/<task>/<repo>-pr72/
```

`--dir` must be a plain directory name (`[a-zA-Z0-9._-]+`, no separators). Without it, adding a repo twice is still a conflict. `list`/`status` show aliased worktrees as `<dir> (<repo>)`.

### Drive an agent

```bash
grove send <task> "<prompt>"        # type the prompt + Enter, return immediately
grove wait <task>...                # block until the turn ends
grove read <task>                   # print what the agent last said
grove run  <task> "<prompt>"        # send + wait + read, in one call
```

`read` parses the agent's JSONL transcript (`~/.claude/projects/…`), not the screen — `capture-pane` gives a TUI mid-redraw with ANSI noise and rotated scrollback; the transcript is structured and complete. It returns only what the agent *said*; tool calls and results are dropped unless asked for.

| Flag | On | Meaning |
|------|-----|---------|
| `--brief` | `send`, `run` | Ask the agent to end its turn with a ≤5-line summary |
| `--any` | `wait` | Return when the first task finishes, not all of them |
| `--timeout <secs>` | `wait`, `run` | Give up, exit 9 (default 1800) |
| `--turns <n>` | `read` | Trailing agent turns to show (default 1) |
| `--tools` | `read`, `run` | Annotate turns with the tools the agent called |
| `--full` | `read` | Include tool calls and results — expensive |
| `--max-chars <n>` | `read`, `run` | Cap output; `0` for none (default 4000) |

### Orchestrating from an agent

When the caller is itself an agent, every command is a tool-call round trip and everything it prints lands in a context window. grove is built so a fleet costs little of either:

```bash
grove run  refactor-api "migrate the handlers to the new router" --brief --json
grove wait api web docs --any --json     # whichever finishes first
```

- **Never poll.** `grove status` in a loop costs a round trip every few seconds; `wait` blocks and costs exactly one, however long the turn takes. `--any` makes N parallel tasks cost ~N calls total.
- **Prefer `run`** — three round trips (send + wait + read) collapse into one.
- **Reads are lean by default** — only the final message returns; `--max-chars` caps what a runaway agent can push into your context.
- **`--brief`** makes that final message *be* the report, so the default read is all you need.
- **Branch on the exit code**, not prose. Exit `9` = timeout = *still working*, not failed — the task is untouched and can be waited on again.

## Agent Status Hook

grove knows what an agent is doing via `hooks/agent-tmux-status.sh`, which writes one entry per tmux pane (state, kind, cwd, transcript path). Everything that reads agent state — `list`, `status`, `wait`, `read`, `run`, the TUI — is a view over that file. Without it, agent state reads as `unknown` and `wait`/`run` have no finish line.

Wire it into `~/.claude/settings.json`, one invocation per lifecycle event:

```json
{
  "hooks": {
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "<repo>/hooks/agent-tmux-status.sh claude active" }] }],
    "Notification":     [{ "hooks": [{ "type": "command", "command": "<repo>/hooks/agent-tmux-status.sh claude waiting" }] }],
    "Stop":             [{ "hooks": [{ "type": "command", "command": "<repo>/hooks/agent-tmux-status.sh claude idle" }] }],
    "SessionEnd":       [{ "hooks": [{ "type": "command", "command": "<repo>/hooks/agent-tmux-status.sh claude cleanup" }] }]
  }
}
```

Claude pipes each event a JSON payload containing `transcript_path`; the hook stores it, which is what lets `grove read` quote the agent instead of scraping its screen. Other agents (`codex`, `opencode`, `cursor`) call the same script with their own kind.

## Configuration

`~/.grove/config.json` (all fields optional):

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

Env overrides: `GROVE_CONFIG`, `GROVE_REPOS_DIR`, `GROVE_TASKS_DIR`, `GROVE_JSON`, `GROVE_STATE_FILE`.

## JSON & Exit Codes

Every command supports `--json`:

```
Success: { "ok": true, ...fields }
Error:   { "ok": false, "error": "<code>", "message": "<human>", "exit_code": N }
```

Exit codes: `1` general, `2` task not found, `3` repo not registered, `4` tmux not running, `5` uncommitted changes, `6` conflict, `7` tui, `8` database, `9` timeout (still working, not failed).

## Common Workflows

### Parallel fan-out, end to end

```bash
grove init api-v2 api --branch feat/v2 --no-attach
grove init web-v2 web --branch feat/v2 --no-attach
grove send api-v2 "migrate handlers to the new router" --brief
grove send web-v2 "update the client for the new routes" --brief
grove wait api-v2 web-v2 --timeout 3600 --json   # one blocking call for both
grove read api-v2 --json && grove read web-v2 --json
```

### Clean up a task

```bash
grove close <task>            # removes worktrees, deletes merged branches
grove close <task> --force    # also discards uncommitted work (not recoverable)
```

## Pitfalls

- **Agent busy**: `send`/`run` refuse when the task's agent is mid-turn — keystrokes would interleave. Wait, then send. The guard is keyed to the task's own pane, so an unrelated busy agent never blocks you.
- **Hook not wired**: no hook means no state and no transcript path — `wait` can't tell a finished turn from a fresh one, and `read` falls back to finding the transcript by convention (Claude only, and only if the agent ran from the task dir).
- **Timeout is not failure**: exit 9 leaves the task running and untouched; wait on it again.
- **`--force` on close discards uncommitted work** in the task's worktrees — not recoverable.
- **Non-Claude agents** report state through the same hook but don't write a Claude-shaped transcript, so `read`/`run` are Claude-only in practice.
