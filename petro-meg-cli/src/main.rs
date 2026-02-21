use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::exit;

use clap::{Args, Parser, Subcommand, ValueEnum};
use petro_meg::reader::MegaFileReader;

/// Utility for working with Petroglyph MEGA files.
#[derive(Parser)]
#[command(version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(ValueEnum, Clone, Debug)]
enum MegaFileVersion {
    V1,
    V2,
    V3,
}

#[derive(Args)]
struct ReaderArgs {
    /// MEGA file to read from.
    source: PathBuf,

    /// Which MEGA file version to read the file as.
    #[arg(short = 'v', long = "meg-version", value_enum, default_value_t = MegaFileVersion::V1)]
    mega_version: MegaFileVersion,
}

#[derive(Subcommand)]
enum Commands {
    /// List the contents of a MEGA file.
    List(ListCmd),
}

#[derive(Args)]
struct ListCmd {
    #[command(flatten)]
    reader: ReaderArgs,
}

fn main() {
    let args = Cli::parse();
    match args.command {
        Commands::List(ref list) => run_list(list),
    }
}

/// Executes the list command on a MEGA file.
fn run_list(list: &ListCmd) {
    let reader = open_reader(&list.reader);
    for (path, files) in reader.files() {
        print!("{}", path.display());
        for (idx, file) in files.iter().enumerate() {
            print!("  [{idx}]: {}", file.size());
        }
        println!();
    }
}

fn open_reader(args: &ReaderArgs) -> MegaFileReader<BufReader<File>> {
    let file = match File::open(&args.source) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Unable to open source file {:?}: {e}", args.source);
            exit(1);
        }
    };
    let file = BufReader::new(file);
    let reader = match args.mega_version {
        MegaFileVersion::V1 => MegaFileReader::parse_v1(file),
        MegaFileVersion::V2 => {
            eprintln!("V2 Mega files are not supported yet.");
            exit(1);
        }
        MegaFileVersion::V3 => {
            eprintln!("V3 Mega files are not supported yet.");
            exit(1);
        }
    };
    match reader {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to parse MEGA file headers: {e}");
            exit(1);
        }
    }
}
