mod cli;
mod commands;
mod config;
mod db;
mod error;
mod git;
mod herdr;
mod output;
mod transcript;
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
        } => {
            let opts = commands::init::InitOptions {
                repos: &repos,
                context: context.as_deref(),
                branch: branch.as_deref(),
                base: base.as_deref(),
                interactive,
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
        Commands::Send {
            task_id,
            prompt,
            brief,
        } => commands::send::run(&task_id, &prompt, brief, &ctx)?,
        Commands::Wait {
            task_ids,
            any,
            timeout,
        } => commands::wait::run(&task_ids, any, timeout, &ctx)?,
        Commands::Read {
            task_id,
            turns,
            tools,
            full,
            max_chars,
        } => commands::read::run(&task_id, turns, tools, full, max_chars, &ctx)?,
        Commands::Run {
            task_id,
            prompt,
            brief,
            timeout,
            max_chars,
            tools,
        } => commands::run::run(&task_id, &prompt, brief, timeout, max_chars, tools, &ctx)?,
        Commands::Tui { popup } => {
            // `ctx` is unused in this arm; NLL ends its borrow of config + db
            // here, so they can be moved into the TUI which owns them.
            tui::run(config, db, verbose, popup)?;
        }
        Commands::Add {
            task_id,
            repo,
            branch,
            base,
            dir,
        } => commands::add::run(
            &task_id,
            &repo,
            branch.as_deref(),
            base.as_deref(),
            dir.as_deref(),
            &ctx,
        )?,
    }

    Ok(())
}
