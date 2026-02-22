use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
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
    #[arg(short = 'v', long = "meg-version", value_enum, default_value_t = MegaFileVersion::V1)]
    mega_version: MegaFileVersion,
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
    #[arg(long, conflicts_with = "out_dir")]
    out: Option<PathBuf>,
    /// Specific files to extract.
    ///
    /// These must exactly match the case of the file names in the MEGA file regardless of the case
    /// mode.
    #[arg(conflicts_with = "all")]
    files: Vec<PathBuf>,
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
    /// Selects which file index for a given path to extract. This can be helpful for improperly
    /// formed MEGA files which contain multiple files with the same path. Applies to all paths. If
    /// a path does not have a file with the specified index, that path will be skipped.
    #[arg(long, default_value_t = 0)]
    index: usize,
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
    let reader = open_reader(&cmd.reader);
    for (path, files) in reader.files() {
        print!("{}", path.display());
        for (idx, file) in files.iter().enumerate() {
            print!("  [{idx}]: {}", file.size());
        }
        println!();
    }
}

/// Executes the extract command on a MEGA file.
fn run_extract(cmd: ExtractCmd) {
    let mut reader = open_reader(&cmd.reader);

    let files_to_extract: Vec<_> = if cmd.all {
        reader
            .files()
            .filter_map(|(path, files)| {
                if files.get(cmd.index).is_some() {
                    Some(path.to_path_buf())
                } else {
                    if files.is_empty() {
                        eprintln!(
                            "Skipping path {} which does not have any files.",
                            path.display()
                        );
                    } else {
                        eprintln!(
                            "Skipping path {} which does not have a file with index {}.",
                            path.display(),
                            cmd.index
                        );
                    }
                    None
                }
            })
            .collect()
    } else if cmd.files.len() > 0 {
        cmd.files
            .into_iter()
            .filter_map(
                |path| match reader.get_files(&path).map(|files| files.get(cmd.index)) {
                    None => {
                        eprintln!("Path {} not found in the MEGA file.", path.display());
                        None
                    }
                    Some(None) => {
                        eprintln!(
                            "Skipping path {} which does not have a file with index {}.",
                            path.display(),
                            cmd.index
                        );
                        None
                    }
                    Some(Some(_)) => Some(path),
                },
            )
            .collect()
    } else {
        Vec::new()
    };

    if files_to_extract.is_empty() {
        eprintln!("Nothing to extract.");
        exit(2);
    }

    let transform_to_output_path = |path: &PathBuf| {
        let mut output_path = if cmd.flatten {
            match path.file_name() {
                None => {
                    eprintln!("Unexpected path {}, unable to flatten", path.display());
                    exit(1);
                }
                Some(name) => name.into(),
            }
        } else {
            path.to_path_buf()
        };
        cmd.case.convert(&mut output_path);
        output_path
    };

    let output_paths: Vec<_> = match (cmd.out, cmd.out_dir) {
        // No outputs specified, write to the PWD with appropriate case transform and flattening.
        (None, None) => files_to_extract
            .iter()
            .map(transform_to_output_path)
            .collect(),
        (Some(out), None) => {
            if files_to_extract.len() > 1 {
                eprintln!("--out can only be used when extracting a single file.");
                exit(1);
            }
            vec![out]
        }
        (None, Some(out_dir)) => files_to_extract
            .iter()
            .map(transform_to_output_path)
            .map(|path| out_dir.join(path))
            .collect(),
        (Some(_), Some(_)) => unreachable!("should be prevented by clap conflicts_with"),
    };

    assert!(files_to_extract.len() == output_paths.len());

    for (mega_path, output_path) in files_to_extract.iter().zip(&output_paths) {
        if let Some(parent) = output_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "Failed to create output directory {}: {e}",
                    parent.display()
                );
                eprintln!("Skipping file {}", mega_path.display());
                continue;
            }
        }
        let mut output = match File::create(output_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Failed to open output file {}: {e}", output_path.display());
                eprintln!("Skipping file {}", mega_path.display());
                continue;
            }
        };
        let mut source = reader.get_reader(mega_path, cmd.index).unwrap();
        if let Err(e) = std::io::copy(&mut source, &mut output) {
            eprintln!(
                "Error while copying {} to {}: {e}",
                mega_path.display(),
                output_path.display()
            );
        }
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
