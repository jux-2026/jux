use anyhow::Result;
use clap::{Parser, Subcommand};
use jux_core::{FileWorkspaceStore, WorkspaceStore};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "jux",
    version = jux_core::version(),
    about = "Jux agent command line interface."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Create and inspect command-driven runs.")]
    Run(RunCommand),
}

#[derive(Debug, Parser)]
struct RunCommand {
    #[command(subcommand)]
    command: RunSubcommand,
}

#[derive(Debug, Subcommand)]
enum RunSubcommand {
    #[command(about = "Create a run in the active session.")]
    New {
        #[arg(help = "User request that starts the run.")]
        request: String,

        #[arg(long, default_value = ".", help = "Workspace root directory.")]
        workspace: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Run(command)) => handle_run(command),
        None => Ok(()),
    }
}

fn handle_run(command: RunCommand) -> Result<()> {
    match command.command {
        RunSubcommand::New { request, workspace } => {
            let store = FileWorkspaceStore::new(workspace);
            let run = store.create_run_in_active_session(request)?;

            println!("{}", run.id);

            Ok(())
        }
    }
}
