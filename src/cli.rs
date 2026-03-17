use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
        repo: Option<String>,
    },

    /// Create a task with worktrees from registered repos
    Init {
        /// Task identifier
        task_id: String,
        /// Repository names to include
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
        /// Skip tmux session creation (default in Phase 1)
        #[arg(long)]
        no_tmux: bool,
        /// Skip Claude launch (default in Phase 1)
        #[arg(long)]
        no_claude: bool,
    },

    /// Close a task and remove its worktrees
    Close {
        /// Task identifier
        task_id: String,
        /// Force close even with uncommitted changes
        #[arg(long)]
        force: bool,
    },

    /// List active tasks
    List,
}
