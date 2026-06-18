mod agent;
mod cli;
mod commands;
mod config;
mod db;
mod error;
mod git;
mod output;
mod tmux;
mod tui;
mod validation;

use clap::{CommandFactory, Parser};
use cli::{Cli, Commands};
use error::GroveError;

fn main() {
    // Dynamic shell completion hook. No-op unless the COMPLETE env var is set
    // (i.e. the shell is requesting completions). Must run before Cli::parse().
    clap_complete::env::CompleteEnv::with_factory(Cli::command).complete();

    let cli = Cli::parse();
    let json_mode_flag = cli.json;

    let result = run(cli);

    if let Err(e) = result {
        output::error(json_mode_flag, &e);
        std::process::exit(e.exit_code());
    }
}

fn run(cli: Cli) -> Result<(), GroveError> {
    let json_mode = cli.json;
    let verbose = cli.verbose;

    let (config, json_from_config) =
        config::GroveConfig::load(cli.config.as_deref(), None, None, Some(cli.json))?;
    let json_mode = json_mode || json_from_config;

    let db = db::Db::open()?;

    let ctx = commands::Ctx {
        config: &config,
        db: &db,
        json_mode,
        verbose,
    };

    match cli.command {
        Commands::Register { name, url } => commands::register::run(&name, &url, &ctx)?,
        Commands::Repos => commands::repos::run(&ctx)?,
        Commands::Sync { repo } => commands::sync::run(repo.as_deref(), &ctx)?,
        Commands::Init {
            task_id,
            repos,
            context,
            branch,
            base,
            interactive,
            no_tmux,
            no_claude,
            no_agent,
            agent,
            no_attach,
        } => {
            let opts = commands::init::InitOptions {
                repos: &repos,
                context: context.as_deref(),
                branch: branch.as_deref(),
                base: base.as_deref(),
                interactive,
                no_tmux,
                no_claude,
                no_agent,
                no_attach,
                agent: agent.as_deref(),
            };
            commands::init::run(task_id.as_deref(), &opts, &ctx)?;
        }
        Commands::Close {
            task_id,
            force,
            delete_branches,
            interactive,
        } => commands::close::run(
            task_id.as_deref(),
            force,
            delete_branches,
            interactive,
            &ctx,
        )?,
        Commands::List => commands::list::run(&ctx)?,
        Commands::Attach { task_id } => commands::attach::run(&task_id, &ctx)?,
        Commands::Status { task_id } => commands::status::run(task_id.as_deref(), &ctx)?,
        Commands::Send { task_id, prompt } => commands::send::run(&task_id, &prompt, &ctx)?,
        Commands::Tui { popup } => {
            if !tmux::is_tmux_available() {
                return Err(GroveError::TmuxNotRunning("tmux is not installed".into()));
            }
            if !tmux::is_inside_tmux() {
                return Err(GroveError::TmuxNotRunning(
                    "grove tui must be run inside tmux".into(),
                ));
            }
            // `ctx` is unused in this arm; NLL ends its borrow of config + db
            // here, so they can be moved into the TUI which owns them — no
            // second Db::open / config reload.
            tui::run(config, db, verbose, popup)?;
        }
        Commands::ProjectTouch { path } => {
            ctx.db.upsert_project(&path)?;
        }
        Commands::Compose { target } => {
            if !tmux::is_inside_tmux() {
                return Err(GroveError::TmuxNotRunning(
                    "grove compose must be run inside tmux".into(),
                ));
            }
            commands::compose::run(target.as_deref())?;
        }
        Commands::Add {
            task_id,
            repo,
            branch,
            base,
        } => commands::add::run(&task_id, &repo, branch.as_deref(), base.as_deref(), &ctx)?,
    }

    Ok(())
}
