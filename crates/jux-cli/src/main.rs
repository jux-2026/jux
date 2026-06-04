use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "jux",
    version = jux_core::version(),
    about = "Jux agent command line interface."
)]
struct Cli {}

fn main() -> Result<()> {
    let _cli = Cli::parse();
    Ok(())
}
