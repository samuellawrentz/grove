mod agent;
mod cli;
mod db;
mod commands;
mod config;
mod error;
mod git;
mod output;
mod recents;
mod state;
mod tmux;
mod tui;
mod validation;

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
            interactive,
            no_tmux,
            no_claude,
            no_agent,
            agent,
            no_attach,
        } => {
            let task_id = match task_id {
                Some(id) => id,
                None => {
                    if !interactive {
                        return Err(GroveError::General(
                            "task_id is required (use -i for interactive mode)".to_string(),
                        ));
                    }
                    dialoguer::Input::new()
                        .with_prompt("Task ID")
                        .interact_text()
                        .map_err(|e| {
                            GroveError::General(format!("interactive input failed: {e}"))
                        })?
                }
            };
            let opts = commands::init::InitOptions {
                repos: &repos,
                context: context.as_deref(),
                branch: branch.as_deref(),
                base: base.as_deref(),
                interactive,
                no_tmux,
                no_claude: no_claude || no_agent,
                no_attach,
                agent: agent.as_deref(),
            };
            commands::init::run(&task_id, &opts, &config, &mut state, json_mode, verbose)?;
        }
        Commands::Close {
            task_id,
            force,
            delete_branches,
            interactive,
        } => {
            commands::close::run(
                task_id.as_deref(),
                force,
                delete_branches,
                interactive,
                &config,
                &mut state,
                json_mode,
                verbose,
            )?;
        }
        Commands::List => {
            commands::list::run(&state, &config, json_mode, verbose)?;
        }
        Commands::Attach { task_id } => {
            commands::attach::run(&task_id, &state, json_mode, verbose)?;
        }
        Commands::Status { task_id } => {
            commands::status::run(task_id.as_deref(), &state, json_mode, verbose)?;
        }
        Commands::Send { task_id, prompt } => {
            commands::send::run(&task_id, &prompt, &state, json_mode, verbose)?;
        }
        Commands::Tui => {
            if !tmux::is_tmux_available() {
                return Err(GroveError::TmuxNotRunning("tmux is not installed".into()));
            }
            if !tmux::is_inside_tmux() {
                return Err(GroveError::TmuxNotRunning(
                    "grove tui must be run inside tmux".into(),
                ));
            }
            tui::run(verbose)?;
        }
        Commands::RecentsAdd { dir } => {
            recents::add(&dir);
        }
        Commands::Add {
            task_id,
            repo,
            branch,
            base,
        } => {
            commands::add::run(
                &task_id,
                &repo,
                branch.as_deref(),
                base.as_deref(),
                &config,
                &mut state,
                json_mode,
                verbose,
            )?;
        }
    }

    Ok(())
}
