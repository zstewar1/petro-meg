use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::exit;

use clap::{Args, Parser, Subcommand, ValueEnum};
use petro_meg::parse_v1;
use petro_meg::parser::{File, MegParseError};
use petro_meg::path::MegPathBuf;

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
    let args = Cli::parse();
    match args.command {
        Commands::List(ref cmd) => read::run_list(cmd),
        Commands::Extract(cmd) => read::run_extract(cmd),
    }
}
