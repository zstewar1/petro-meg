use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::exit;

use clap::{Args, Parser, Subcommand, ValueEnum};
use petro_meg::parse_v1;
use petro_meg::parser::{File, MegParseError};
use petro_meg::path::MegPathBuf;

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

/// How to handle file cases from MEGA files.
#[derive(ValueEnum, Clone, Copy, Debug)]
enum NameCaseMode {
    /// Keep the file name case as-specified.
    Keep,
    /// Convert all file names to lower case.
    Lower,
    /// Convert file names to upper case.
    Upper,
}

impl NameCaseMode {
    fn convert(self, path: &mut PathBuf) {
        match self {
            NameCaseMode::Keep => {}
            NameCaseMode::Lower => path.as_mut_os_str().make_ascii_lowercase(),
            NameCaseMode::Upper => path.as_mut_os_str().make_ascii_uppercase(),
        }
    }
}

#[derive(Args)]
struct ReaderArgs {
    /// MEGA file to read from.
    source: PathBuf,

    /// Which MEGA file version to read the file as.
    #[arg(short = 'v', long = "meg-version", value_enum, default_value_t = MegVersion::V1)]
    mega_version: MegVersion,
}

#[derive(Subcommand)]
enum Commands {
    /// List the contents of a MEGA file.
    List(ListCmd),
    /// Extract the contents of a MEGA file.
    Extract(ExtractCmd),
}

#[derive(Args)]
struct ListCmd {
    #[command(flatten)]
    reader: ReaderArgs,
}

#[derive(Args)]
struct ExtractCmd {
    #[command(flatten)]
    reader: ReaderArgs,
    /// Destination directory to extract into. If not specified, extracts into the current
    /// directory. The directory will be created if it doesn't exist.
    #[arg(long = "out-dir", conflicts_with = "out")]
    out_dir: Option<PathBuf>,
    /// Specifies the destination file to extract to. Can only be used when extracting a single
    /// file.
    ///
    /// When --out is used, no directories will be created.
    #[arg(long, conflicts_with = "out_dir")]
    out: Option<PathBuf>,
    /// Specific files to extract.
    #[arg(conflicts_with = "all")]
    files: Vec<MegPathBuf>,
    /// If true, extract all files.
    #[arg(long, conflicts_with = "files")]
    all: bool,
    /// If set, remove the path from the extracted files.
    #[arg(long)]
    flatten: bool,
    /// How to handle case in MEGA file path names. On windows, file names are treated as
    /// case-insensitive, which isn't true for other operating systems. When working with MEGA files
    /// on other systems, it can be helpful to force file names into a consistent case.
    #[arg(long, conflicts_with = "out", value_enum, default_value_t = NameCaseMode::Keep)]
    case: NameCaseMode,
}

fn main() {
    let args = Cli::parse();
    match args.command {
        Commands::List(ref cmd) => run_list(cmd),
        Commands::Extract(cmd) => run_extract(cmd),
    }
}

/// Executes the list command on a MEGA file.
fn run_list(cmd: &ListCmd) {
    let data = load_file(&cmd.reader.source);
    for file in parse(cmd.reader.mega_version, &data).map(unwrap_file_or_exit) {
        println!("{}: {}", file.path().unwrap(), file.contents().len());
    }
}

/// Executes the extract command on a MEGA file.
fn run_extract(cmd: ExtractCmd) {
    let data = load_file(&cmd.reader.source);

    let mut files_to_extract = HashMap::<MegPathBuf, Vec<File>>::new();
    if cmd.all {
        for file in parse(cmd.reader.mega_version, &data).map(unwrap_file_or_exit) {
            files_to_extract
                .entry(file.path().unwrap().to_owned())
                .or_default()
                .push(file);
        }
    } else if cmd.files.len() > 0 {
        let paths_to_extract: HashSet<_> = cmd.files.into_iter().collect();
        for file in parse(cmd.reader.mega_version, &data).map(unwrap_file_or_exit) {
            if paths_to_extract.contains(file.path().unwrap()) {
                files_to_extract
                    .entry(file.path().unwrap().to_owned())
                    .or_default()
                    .push(file);
            }
        }
        let unmatched_paths: BTreeSet<_> = paths_to_extract
            .into_iter()
            .filter(|path| !files_to_extract.contains_key(path))
            .collect();
        if !unmatched_paths.is_empty() {
            eprintln!("The following paths were not found in the MEGA file:");
            for path in unmatched_paths {
                eprintln!("  {path}");
            }
            exit(1);
        }
    } else {
        eprintln!("Must specify either a list of MEGA file paths to extract or --all");
        exit(1);
    };

    if files_to_extract.is_empty() {
        eprintln!("Nothing to extract.");
        exit(2);
    }

    if let Some(out) = cmd.out {
        if files_to_extract.len() > 1 {
            eprintln!("--out can only be used when extracting a single file");
            exit(1);
        }
        // Get the file list.
        let files_to_extract = files_to_extract.into_values().next().unwrap();
        if files_to_extract.len() > 1 {
            eprintln!("--out can only be used when extracting a single file");
            exit(1);
        }
        // Only existing files were added, so the list should not be empty.
        let file = files_to_extract.into_iter().next().unwrap();

        println!("Extracting {} to {}", file.path().unwrap(), out.display());
        if let Err(e) = std::fs::write(&out, file.contents()) {
            eprintln!("Writing to output file {} failed: {e}", out.display());
            exit(1);
        }
        println!("Done");
        exit(0);
    }

    todo!()

    // let output_paths: Vec<_> = match (cmd.out, cmd.out_dir) {
    //     // No outputs specified, write to the PWD with appropriate case transform and flattening.
    //     (None, None) => files_to_extract
    //         .iter()
    //         .map(transform_to_output_path)
    //         .collect(),
    //     (Some(out), None) => {
    //         if files_to_extract.len() > 1 {
    //             eprintln!("--out can only be used when extracting a single file.");
    //             exit(1);
    //         }
    //         vec![out]
    //     }
    //     (None, Some(out_dir)) => files_to_extract
    //         .iter()
    //         .map(transform_to_output_path)
    //         .map(|path| out_dir.join(path))
    //         .collect(),
    //     (Some(_), Some(_)) => unreachable!("should be prevented by clap conflicts_with"),
    // };

    // assert!(files_to_extract.len() == output_paths.len());

    // for (mega_path, output_path) in files_to_extract.iter().zip(&output_paths) {
    //     if let Some(parent) = output_path.parent() {
    //         if let Err(e) = std::fs::create_dir_all(parent) {
    //             eprintln!(
    //                 "Failed to create output directory {}: {e}",
    //                 parent.display()
    //             );
    //             eprintln!("Skipping file {}", mega_path.display());
    //             continue;
    //         }
    //     }
    //     let mut output = match File::create(output_path) {
    //         Ok(f) => f,
    //         Err(e) => {
    //             eprintln!("Failed to open output file {}: {e}", output_path.display());
    //             eprintln!("Skipping file {}", mega_path.display());
    //             continue;
    //         }
    //     };
    //     let mut source = reader.get_reader(mega_path, cmd.index).unwrap();
    //     if let Err(e) = std::io::copy(&mut source, &mut output) {
    //         eprintln!(
    //             "Error while copying {} to {}: {e}",
    //             mega_path.display(),
    //             output_path.display()
    //         );
    //     }
    // }
}

fn load_file(path: impl AsRef<Path>) -> Vec<u8> {
    match std::fs::read(path.as_ref()) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "Unable to read source file {}: {e}",
                path.as_ref().display()
            );
            exit(1);
        }
    }
}

fn parse(
    version: MegVersion,
    data: &[u8],
) -> impl Iterator<Item = Result<File<'_>, MegParseError>> {
    let iter: Box<dyn Iterator<Item = Result<File<'_>, MegParseError>>> = match version {
        MegVersion::V1 => Box::new(parse_v1(data)),
        v => {
            eprintln!("MEGA file version {v:?} is not supported");
            exit(2);
        }
    };
    iter
}

/// Extract the file from the result or exit with an error message.
fn unwrap_file_or_exit<'b>(file: Result<File<'b>, MegParseError>) -> File<'b> {
    match file {
        Ok(file) => file,
        Err(e) => {
            eprintln!("Error while decoding the MEGA file: {e}");
            exit(1);
        }
    }
}
