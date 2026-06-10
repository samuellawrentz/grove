use clap::{Parser, Subcommand};
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use std::path::PathBuf;

/// Dynamic completion: registered repo names. Degrades to empty on any error
/// (completion must never fail loudly).
fn complete_repos() -> Vec<CompletionCandidate> {
    let Ok(db) = crate::db::Db::open() else {
        return Vec::new();
    };
    db.list_repos()
        .unwrap_or_default()
        .into_iter()
        .map(|r| CompletionCandidate::new(r.name).help(Some(r.url.into())))
        .collect()
}

/// Dynamic completion: active task ids. Degrades to empty on any error.
fn complete_tasks() -> Vec<CompletionCandidate> {
    let Ok(db) = crate::db::Db::open() else {
        return Vec::new();
    };
    db.list_tasks()
        .unwrap_or_default()
        .into_iter()
        .map(|t| {
            CompletionCandidate::new(t.id).help(Some(t.path.to_string_lossy().into_owned().into()))
        })
        .collect()
}

#[derive(Parser, Debug)]
#[command(
    name = "grove",
    version,
    about = "Multi-repo workspace manager for AI-assisted development"
)]
pub struct Cli {
    /// Output as JSON
    #[arg(long, global = true)]
    pub json: bool,

    /// Show git commands and exit codes
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Path to config file
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Register a bare clone of a repository
    Register {
        /// Name for the repository
        name: String,
        /// Git URL to clone
        url: String,
    },

    /// List registered repositories
    Repos,

    /// Fetch updates for registered repositories
    Sync {
        /// Repository name (omit for all)
        #[arg(add = ArgValueCandidates::new(complete_repos))]
        repo: Option<String>,
    },

    /// Create a task with worktrees from registered repos
    Init {
        /// Task identifier (prompted in interactive mode if omitted)
        task_id: Option<String>,
        /// Repository names to include
        #[arg(add = ArgValueCandidates::new(complete_repos))]
        repos: Vec<String>,
        /// Context text for CONTEXT.md
        #[arg(long)]
        context: Option<String>,
        /// Branch name (default: task-id)
        #[arg(long)]
        branch: Option<String>,
        /// Base branch to create worktree from
        #[arg(long)]
        base: Option<String>,
        /// Interactive mode: select repos and branch from prompts
        #[arg(short, long)]
        interactive: bool,
        /// Skip tmux session creation (default in Phase 1)
        #[arg(long)]
        no_tmux: bool,
        /// Skip Claude launch (default in Phase 1)
        #[arg(long)]
        no_claude: bool,
        /// Skip all agent launches
        #[arg(long)]
        no_agent: bool,
        /// Which agent to launch (overrides default)
        #[arg(long)]
        agent: Option<String>,
        /// Skip auto-attach to tmux window
        #[arg(long)]
        no_attach: bool,
    },

    /// Close a task and remove its worktrees
    Close {
        /// Task identifier (prompted in interactive mode if omitted)
        #[arg(add = ArgValueCandidates::new(complete_tasks))]
        task_id: Option<String>,
        /// Force close even with uncommitted changes
        #[arg(long)]
        force: bool,
        /// Force-delete task branches even if unmerged (close deletes merged branches by default)
        #[arg(long, short = 'D')]
        delete_branches: bool,
        /// Interactive mode: select task from list
        #[arg(short, long)]
        interactive: bool,
    },

    /// List active tasks
    List,

    /// Attach to a task's tmux window
    Attach {
        /// Task identifier
        #[arg(add = ArgValueCandidates::new(complete_tasks))]
        task_id: String,
    },

    /// Show task status with Claude state
    Status {
        /// Task identifier (omit for all tasks)
        #[arg(add = ArgValueCandidates::new(complete_tasks))]
        task_id: Option<String>,
    },

    /// Send a prompt to Claude in a task
    Send {
        /// Task identifier
        #[arg(add = ArgValueCandidates::new(complete_tasks))]
        task_id: String,
        /// Prompt text to send
        prompt: String,
    },

    /// Interactive TUI pane manager
    Tui {
        /// Quit after launching a pane (for tmux popup usage)
        #[arg(long)]
        popup: bool,
    },

    /// Register a directory path as a project (used by tmux hooks)
    #[clap(hide = true)]
    ProjectTouch {
        /// Directory path to register as project
        path: String,
    },

    /// Open editor to compose and send text to a tmux pane
    Compose {
        /// Target pane ID (default: pane above the current one)
        #[arg(long)]
        target: Option<String>,
    },

    /// Add a repo to an existing task
    Add {
        /// Task identifier
        #[arg(add = ArgValueCandidates::new(complete_tasks))]
        task_id: String,
        /// Repository name to add
        #[arg(add = ArgValueCandidates::new(complete_repos))]
        repo: String,
        /// Branch name (default: match existing task branch)
        #[arg(long)]
        branch: Option<String>,
        /// Base branch to create worktree from
        #[arg(long)]
        base: Option<String>,
    },
}
