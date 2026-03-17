# Grove: Multi-Repo Workspace Manager for AI-Assisted Development

## Product Requirements Document

**Version:** 2.0
**Author:** Travis (AI Chief of Staff)
**Date:** 2026-03-17
**Status:** Draft — Ready for Implementation

---

## 1. Problem

Cross-repo development with AI agents is painful. A single feature touching `plivo-api`, `plivo-web`, and `plivo-python` requires: mkdir, clone 3 repos, create branches, open tmux, split panes, launch Claude Code in each, then mentally track which agents need input. Cleanup is manual and error-prone.

No existing tool (dmux, claude-tmux, OMC PSM) provides: a bare clone registry for instant worktrees, a task abstraction grouping multiple repos, Claude state detection across panes, or agent primitives for programmatic orchestration.

## 2. Solution

Grove is a Rust CLI with three primitives:

1. **Bare clone registry** — shared git object store. Register once, create worktrees instantly without network clones.
2. **Tasks** — folders containing N worktrees from N repos, each on a task-specific branch, with a shared `CONTEXT.md`.
3. **Sessions** — tmux sessions with one pane per repo, optionally running Claude Code.

## 3. Goals (MVP)

1. Bare clone registry with sync
2. Task lifecycle: create, list, add repos, close
3. Tmux integration: auto session/pane creation, attach/detach
4. Claude state detection: `working`, `idle`, `waiting`, `not-running`
5. Claude interaction: accept/reject/send across panes
6. Merge workflow: local merge, push, or PR via `gh`
7. Agent primitives: `wait` (block until state) and `exec` (send + wait)
8. `--json` on every command, predictable exit codes, non-interactive in non-TTY
9. Idempotent operations, atomic state file

## 4. Non-Goals (MVP)

TUI, web dashboard, native app, Linear/Jira integration, multi-user, remote execution, non-tmux multiplexers, non-git VCS, auto conflict resolution, Claude version management.

---

## 5. Core Concepts

### Directory Layout

```
~/repos/                          # Bare clone registry
  plivo-api.git/                  # bare clone
  plivo-web.git/

~/tasks/                          # Active tasks
  TASK-1234/
    CONTEXT.md                    # Shared task context
    plivo-api/                    # worktree (branch: TASK-1234)
    plivo-web/                    # worktree (branch: TASK-1234)

~/.grove/
  config.json                     # User config
  state.json                      # Source of truth (atomic writes)
```

### Tmux Sessions

- Session name: `grove:<task-id>`
- One window (`work`), one pane per repo, layout: `even-vertical`
- Pane titles set to repo names

### Claude States

| State | Meaning |
|-------|---------|
| `working` | Output is changing between polls |
| `idle` | Content stable for N polls, no permission prompt detected |
| `waiting` | Permission/input prompt detected (regex patterns on last 10 lines) |
| `not-running` | No Claude process in pane |

Detection: poll via `tmux capture-pane` at configurable interval (default 500ms). Compare content hashes for stability. Match against configurable regex patterns for `waiting`.

### State File (`~/.grove/state.json`)

Atomic writes (write-temp-then-rename). Contains:
- `version: 1`
- `repos`: map of registered repos (name, url, path, default_branch, timestamps)
- `tasks`: map of active tasks (id, path, session, repos with worktree paths, branches, pane IDs)
- `updated_at`

Agents can read this file directly for zero-overhead queries. Claude state requires live `grove status --json`.

---

## 6. Commands

### Global Flags

`--json` (structured output), `--verbose` (show git commands/timing), `--config <path>`

### Repo Management

| Command | Description |
|---------|-------------|
| `grove register <name> <url>` | Create bare clone, detect default branch, update state |
| `grove repos` | List registered repos with sync status |
| `grove sync [repo]` | `git fetch --all --prune` on all/one repo (parallel, up to 8) |

### Task Lifecycle

| Command | Description |
|---------|-------------|
| `grove init <task-id> [repos...]` | Create task dir, worktrees, tmux session, launch Claude. Idempotent (existing task returns info, exit 0). Interactive repo picker if no repos and TTY. |
| `grove list` | List tasks with Claude state per pane |
| `grove status [task-id]` | Detailed per-pane info (dimensions, last output line, state) |
| `grove attach [task-id]` | Attach to tmux session (interactive picker if no arg + TTY) |
| `grove add <task-id> <repo>` | Add repo to existing task (worktree + pane + Claude) |
| `grove close <task-id>` | Kill session, remove worktrees, delete folder. Refuses if uncommitted changes (override: `--force`) |
| `grove context <task-id>` | Open CONTEXT.md in `$EDITOR` |

**`grove init` flags:** `--context <text>`, `--branch <name>`, `--base <branch>`, `--no-claude`, `--no-tmux`, `--recreate`, `--attach` (default: TTY=true, non-TTY=false)

### Claude Interaction

| Command | Description |
|---------|-------------|
| `grove accept [task-id]` | Send `y` + Enter to all `waiting` panes |
| `grove reject [task-id]` | Send `n` + Enter to all `waiting` panes |
| `grove send <task-id> <text>` | Send text to panes. Filter: `--repo <name>`, `--state <state>`, `--literal` |

Task-id is optional for accept/reject — resolved from current tmux session or cwd.

### Agent Primitives

| Command | Description |
|---------|-------------|
| `grove wait <task-id>` | Block until panes reach `--state` (default: `idle`). `--timeout <s>`, `--any` (any vs all panes). |
| `grove exec <task-id> <prompt>` | Send prompt + wait for idle. `--timeout <s>`, `--repo <name>`, `--accept` (auto-accept permission prompts). |

### Merge Workflow

| Command | Description |
|---------|-------------|
| `grove merge <task-id>` | Merge task branches into base branches. `--push` (push after merge), `--pr` (create PRs via `gh`), `--pr-title`, `--pr-body`, `--no-delete-branch`. Stops on conflict. |

---

## 7. Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error (git, IO, tmux, timeout) |
| 2 | Task not found |
| 3 | Repo not registered |
| 4 | Tmux session / Claude not running |
| 5 | Uncommitted changes blocking operation |
| 6 | Conflict (branch/worktree already exists) |

### JSON Response Contract

```
Success: { "ok": true, ...command-specific fields }
Error:   { "ok": false, "error": "<code>", "message": "<human>", "exit_code": N, "details": {} }
```

---

## 8. Configuration (`~/.grove/config.json`)

All fields optional, defaults applied for missing values.

| Field | Default | Description |
|-------|---------|-------------|
| `repos_dir` | `~/repos` | Bare clones directory |
| `tasks_dir` | `~/tasks` | Tasks directory |
| `poll_interval_ms` | `500` | Claude state poll interval |
| `stable_count_threshold` | `3` | Consecutive identical polls before `idle` |
| `max_parallel_syncs` | `8` | Concurrent fetch limit |
| `auto_launch_claude` | `true` | Launch Claude on init/add |
| `auto_attach` | `true` | Attach after init (TTY only) |
| `claude_command` | `"claude"` | Claude binary name/path |
| `tmux.layout` | `"even-vertical"` | Pane layout |
| `tmux.session_prefix` | `"grove"` | Session name prefix |
| `claude_patterns.waiting` | (regex list) | Patterns for `waiting` detection |
| `git.fetch_prune` | `true` | Prune on fetch |
| `git.merge_no_ff` | `true` | No-ff merges |
| `git.clone_retries` | `3` | Clone retry count |

**Env var overrides:** `GROVE_CONFIG`, `GROVE_REPOS_DIR`, `GROVE_TASKS_DIR`, `GROVE_JSON`

**Precedence:** CLI flags > env vars > config file > defaults

---

## 9. Design Principles

| Principle | Implementation |
|-----------|---------------|
| Agent-first | `--json` everywhere, predictable exit codes, `exec`/`wait` primitives |
| Idempotent | `grove init` on existing task returns it (exit 0) |
| Non-interactive in non-TTY | Never prompts, fails with exit code + JSON |
| Atomic state | Write-temp-then-rename for `state.json` |
| Zero-overhead reads | Agents read `state.json` directly, no subprocess needed |

---

## 10. Implementation Phases

**Phase 1 (Week 1): Foundation**
— Cargo scaffolding, clap CLI, state/config file handling, `register`, `repos`, `sync`, `init` (no tmux), `close`, `list` (no Claude state)

**Phase 2 (Week 2): Tmux & Claude**
— Tmux session management, Claude launch, state detection engine, `attach`, `add`, `status`, `accept`/`reject`/`send`, `context`, interactive pickers, non-TTY behavior

**Phase 3 (Week 3): Agent & Merge**
— `wait`, `exec`, `merge` (local/push/PR), `--recreate`, task resolution from cwd/tmux

**Phase 4 (Week 4): Polish**
— Error recovery, retry logic, shell completions, `--verbose`, state migration framework, hardening, docs
