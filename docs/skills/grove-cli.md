---
name: grove-cli
category: software-development
description: Manage multi-repo workspaces and Claude Code sessions with the Grove CLI.
---

# Grove CLI Skill

Grove is a command-line interface (CLI) tool designed to streamline multi-repository development workflows, especially when working with AI coding assistants like Claude Code. It simplifies tasks such as cloning repositories, creating feature branches, setting up isolated worktrees, and managing `tmux` sessions.

## Key Concepts

-   **Bare clone registry**: A central store for Git objects, enabling fast worktree creation without redundant network clones.
-   **Tasks**: A dedicated directory that groups multiple worktrees (from different repositories) for a specific feature or task. Each task has its own task-specific branch and a shared `CONTEXT.md` file.
-   **Sessions**: `tmux` sessions configured by Grove, typically with one pane per repository, and optionally pre-configured to run Claude Code in each pane.
-   **grove-picker**: A standalone `tmux` floating popup for interactively managing Claude Code sessions across different panes.

## Installation

To install Grove, navigate to its source directory and use `cargo`:

1.  Navigate to the Grove source directory (e.g., `/home/samm/src/personal/grove`).
2.  Run the installation command:
    ```bash
    cargo install --path .
    ```

## Usage

### Register Repositories

Before creating tasks, register your repositories with Grove:

```bash
grove register <repo_name> <git_url>
# Example:
# grove register plivo-api git@github.com:org/plivo-api.git
# grove register plivo-web git@github.com:org/plivo-web.git
```

### Create a Task

Create a new task, which sets up worktrees and a `tmux` session:

**Explicit repos and branch:**

```bash
grove init <task_name> <repo1_name> [repo2_name...] --branch <branch_name>
# Example:
# grove init add-billing plivo-api plivo-web --branch feat/billing
```

**Interactive mode (select repos and branch):**

```bash
grove init <task_name> -i
# Example:
# grove init add-billing -i
```

This command creates dedicated worktrees for each specified repository on the given task branch, generates a `CONTEXT.md` file within the task directory, and by default, launches a `tmux` session with Claude Code running in each pane.

### Manage Tasks

-   **List active tasks:**
    ```bash
    grove list
    ```
-   **Close a task:** Cleans up worktrees and the associated `tmux` session.
    ```bash
    grove close <task_name>
    # Example:
    # grove close add-billing
    ```
-   **Force close a task (even with uncommitted changes):**
    ```bash
    grove close <task_name> --force
    # Example:
    # grove close add-billing --force
    ```

### Sync Repositories

Fetch the latest changes for registered repositories:

-   **Sync all registered repositories (in parallel):**
    ```bash
    grove sync
    ```
-   **Sync a specific repository:**
    ```bash
    grove sync <repo_name>
    # Example:
    # grove sync plivo-api
    ```
-   **List all registered repositories:**
    ```bash
    grove repos
    ```

### Configuration

Grove's configuration is managed via `~/.grove/config.json`. All fields are optional and have sensible defaults.
You can override default settings such as `repos_dir`, `tasks_dir`, `max_parallel_syncs`, `auto_launch_claude`, and `tmux` layout.

### grove-picker (Tmux Integration)

The `grove-picker` is a standalone `tmux` floating popup script (`grove-picker.sh`) that helps manage Claude Code sessions across `tmux` panes.

-   **Launch directly:**
    ```bash
    /home/samm/src/personal/grove/grove-picker.sh
    ```
-   **Bind to a `tmux` key (e.g., `Ctrl-G`):**
    Add this to your `~/.tmux.conf`:
    ```tmux
    bind-key C-g run-shell "/path/to/grove-picker.sh"
    ```
    (Replace `/path/to/grove-picker.sh` with the actual path, e.g., `/home/samm/src/personal/grove/grove-picker.sh`)

**Key bindings within the `grove-picker`:**

| Key      | Action                                |
| :------- | :------------------------------------ |
| `Enter`  | Switch to the selected pane           |
| `Ctrl-Y` | Accept (send Enter to Claude)         |
| `Ctrl-R` | Reject (send `n` + Enter to Claude)   |
| `Ctrl-X` | Kill the current pane                 |
| `Ctrl-N` | Spawn a new Claude session            |
| `Ctrl-P` | Send a custom prompt to Claude        |

## Pitfalls

-   **Uncommitted changes when closing a task:** If you try to `grove close` a task with uncommitted changes in its worktrees, Grove will prevent the closure to avoid data loss. Use `grove close <task_name> --force` to override this, but be aware that uncommitted changes will be lost.
-   **Incorrect Git URLs during registration:** Ensure the Git URLs provided to `grove register` are correct and accessible. Otherwise, task creation will fail.
-   **`tmux` not running:** Grove relies heavily on `tmux` for session management. Make sure `tmux` is installed and running before creating tasks that involve `tmux` sessions.
-   **Claude Code not in PATH:** If `auto_launch_claude` is enabled in `config.json` but the `claude` command is not in your system's `PATH`, Grove won't be able to launch Claude Code sessions automatically.
