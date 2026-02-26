use clap::{Parser, Subcommand, ValueEnum};

use crate::read::{ExtractCmd, ListCmd};

mod read;

/// Utility for working with Petroglyph MEGA files.
#[derive(Parser)]
#[command(version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum MegVersion {
    V1,
    V2,
    V3,
}

#[derive(Subcommand)]
enum Commands {
    /// List the contents of a MEGA file.
    List(ListCmd),
    /// Extract the contents of a MEGA file.
    Extract(ExtractCmd),
}

fn main() {
    tracing_subscriber::fmt::init();

    let args = Cli::parse();
    match args.command {
        Commands::List(cmd) => cmd.run(),
        Commands::Extract(cmd) => cmd.run(),
    }
}
