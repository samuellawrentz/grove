mod cli;
mod commands;
mod config;
mod error;
mod git;
mod output;
mod state;

use clap::Parser;
use cli::{Cli, Commands};
use error::GroveError;

fn main() {
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

    let mut state = state::GroveState::load()?;

    match cli.command {
        Commands::Register { name, url } => {
            commands::register::run(&name, &url, &config, &mut state, json_mode, verbose)?;
        }
        Commands::Repos => {
            commands::repos::run(&state, json_mode)?;
        }
        Commands::Sync { repo } => {
            commands::sync::run(repo.as_deref(), &config, &mut state, json_mode, verbose)?;
        }
        Commands::Init {
            task_id,
            repos,
            context,
            branch,
            base,
            ..
        } => {
            commands::init::run(
                &task_id,
                &repos,
                context.as_deref(),
                branch.as_deref(),
                base.as_deref(),
                &config,
                &mut state,
                json_mode,
                verbose,
            )?;
        }
        Commands::Close { task_id, force } => {
            commands::close::run(&task_id, force, &config, &mut state, json_mode, verbose)?;
        }
        Commands::List => {
            commands::list::run(&state, json_mode)?;
        }
    }

    Ok(())
}
