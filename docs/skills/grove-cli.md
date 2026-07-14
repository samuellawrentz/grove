---
name: grove-cli
category: software-development
description: Manage multi-repo worktree tasks and drive coding agents (send/wait/read/run) with the Grove CLI.
---

# Grove CLI Skill

Grove manages multi-repository development workflows: it registers bare clones, creates per-task worktrees across several repos at once, opens a tmux window per task, and launches a coding agent (Claude Code, Codex, OpenCode, Cursor) in each pane.

It can also **drive** those agents — send them prompts, block until they finish, and read back what they said — which makes a task scriptable without giving up the tmux window a human can attach to and take over.

## Key Concepts

- **Bare clone registry**: central object store, so a new worktree costs no network clone.
- **Task**: a directory grouping one worktree per repo, on a shared task branch, with a `CONTEXT.md`. This is the unit grove operates on.
- **Pane state**: `hooks/agent-tmux-status.sh` records, per tmux pane, what the agent is doing (`active` / `waiting` / `idle`) and where its transcript lives. Every state read in grove is a view over that one file.
- **Transcript**: the agent's JSONL log (`~/.claude/projects/<slug>/<session>.jsonl`). Grove reads *this*, never the screen.

## Setup

```bash
cargo install --path .          # from a grove checkout
```

Then wire the status hook into `~/.claude/settings.json` (see the README's "Agent status hook" section). Without it, `list`/`status` show `unknown` and `wait`/`run` have no finish line. It needs `jq` and only records inside tmux.

## Managing tasks

```bash
grove register <name> <git-url>              # register a repo (once)
grove repos                                  # list registered repos
grove sync [<repo>]                          # fetch all, or one

grove init <task> <repo>... --branch <br>    # create task + worktrees + tmux + agents
grove init <task> -i                         # interactive: pick repos and branch
grove add <task> <repo>                      # add a repo to an existing task

grove list                                   # active tasks
grove status [<task>]                        # tasks + live agent state
grove attach <task>                          # jump to the task's tmux window
grove close <task> [--force]                 # remove worktrees; --force discards uncommitted work
```

## Driving an agent

```bash
grove send <task> "<prompt>"   # type the prompt + Enter, return immediately
grove wait <task>...           # block until the turn ends
grove read <task>              # print what the agent last said
grove run  <task> "<prompt>"   # send + wait + read, in one call
```

| Flag | On | Meaning |
|------|-----|---------|
| `--brief` | `send`, `run` | Ask the agent to end its turn with a ≤5-line summary |
| `--any` | `wait` | Return when the first task finishes, not all of them |
| `--timeout <secs>` | `wait`, `run` | Give up, exit 9 (default 1800) |
| `--turns <n>` | `read` | Trailing agent turns to show (default 1) |
| `--tools` | `read`, `run` | Annotate turns with the tools the agent called |
| `--full` | `read` | Include tool calls and results — expensive |
| `--max-chars <n>` | `read`, `run` | Cap output; `0` for none (default 4000) |

`read` returns only what the agent *said*: tool calls and tool results are dropped unless asked for. A turn that only called tools is skipped, so `read` answers "what happened" rather than "what was written to the file last".

## Orchestrating grove from an agent

If **you** are an agent driving grove, every command is a tool-call round trip and every byte it prints lands in your context. Grove is built so a fleet of tasks costs little of either — but only if you use it as intended:

- **Never poll.** Looping on `grove status` costs a round trip every few seconds for as long as the agent works, and each one carries your whole prompt. `grove wait` blocks and costs exactly one, however long the turn takes.
- **Prefer `run`.** `send` + `wait` + `read` is three round trips; `run` is one, and returns the reply.
- **Fan out with `--any`.** `grove wait a b c --any` returns whichever finishes first, so supervising N parallel tasks costs ~N calls in total instead of N × (however often you'd poll).
- **Use `--brief`.** It makes the agent's final message *be* the report, which is exactly what `read` returns by default — so you never need `--full`.
- **Keep `--max-chars`.** It is the only thing stopping a runaway sub-agent from pushing 50k characters into your context.
- **Use `--json`** and branch on the exit code, not on prose. Exit `9` = timeout, which means *still working* — not failed.

A parallel fan-out, end to end:

```bash
grove init api-v2 api --branch feat/v2 --no-attach
grove init web-v2 web --branch feat/v2 --no-attach

grove send api-v2 "migrate handlers to the new router" --brief
grove send web-v2 "update the client for the new routes" --brief

grove wait api-v2 web-v2 --timeout 3600 --json   # one blocking call for both
grove read api-v2 --json
grove read web-v2 --json
```

## Pitfalls

- **Agent busy.** `send`/`run` refuse when the task's agent is mid-turn — keystrokes would interleave with its work. Wait, then send. The guard is keyed to the *task's* pane, so an unrelated busy agent never blocks you.
- **Hook not wired.** No hook means no state and no transcript path: `wait` cannot tell a finished turn from a fresh one, and `read` falls back to guessing the transcript from the task's directory (which works, but only for Claude, and only if the agent ran from the task dir).
- **`--force` on close discards uncommitted work** in the task's worktrees. It is not recoverable.
- **Non-Claude agents** report state through the same hook but do not write a Claude-shaped transcript, so `read`/`run` are Claude-only in practice.
- **Timeout is not failure.** Exit 9 leaves the task running and untouched; wait on it again.

## grove-picker

A standalone tmux popup (`grove-picker.sh`) for switching between agent panes. Needs only tmux, fzf, and zoxide — no grove CLI.

```tmux
bind-key C-g run-shell "/path/to/grove-picker.sh"
```

| Key | Action |
|-----|--------|
| `Enter` | Switch to the selected pane |
| `Ctrl-Y` | Accept (send Enter to the agent) |
| `Ctrl-R` | Reject (send `n` + Enter) |
| `Ctrl-X` | Kill the pane |
| `Ctrl-N` | Spawn a new agent session |
| `Ctrl-P` | Send a custom prompt |
