use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::exit;

use clap::{Args, ValueEnum};
use globset::{Candidate, GlobBuilder, GlobSet};
use petro_meg::parser::{self, File, MegParseError};
use petro_meg::path::{MegPath, MegPathBuf};
use regex::bytes::RegexSetBuilder;

use crate::MegVersion;

#[derive(Args)]
struct ReaderArgs {
    /// MEGA file to read from.
    source: PathBuf,

    /// Which MEGA file version to read the file as.
    #[arg(short = 'v', long = "meg-version", value_enum, default_value_t = MegVersion::V1)]
    mega_version: MegVersion,
}

#[derive(Args)]
pub(crate) struct ListCmd {
    #[command(flatten)]
    reader: ReaderArgs,
}

impl ListCmd {
    /// Executes the list command on a MEGA file.
    pub(crate) fn run(&self) {
        let data = load_file(&self.reader.source);
        for file in parse(self.reader.mega_version, &data).map(unwrap_file_or_exit) {
            println!("{}: {}", file.path().unwrap(), file.contents().len());
        }
    }
}

/// How to match filenames from the MEGA file.
#[derive(ValueEnum, Clone, Copy, Debug)]
enum PathMatchMode {
    /// Match the literal file names (default).
    Literal,
    /// Match any file names which match the given globs.
    ///
    /// Note: When glob is used, only '/' path separator is recognized, '\' is used for escapes. The
    /// '/' separator will still match '\' in MEGA File names.
    Glob,
    /// Match any file names which match the given regex. This mode gives the most control, as it
    /// does not attempt to match mixed path separators and allows for case-sensitive matching,
    /// unless you use (?i) in the regex.
    Regex,
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
    fn convert(self, path: &mut Path) {
        match self {
            NameCaseMode::Keep => {}
            NameCaseMode::Lower => path.as_mut_os_str().make_ascii_lowercase(),
            NameCaseMode::Upper => path.as_mut_os_str().make_ascii_uppercase(),
        }
    }
}

#[derive(Args)]
pub(crate) struct ExtractCmd {
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
    files: Vec<String>,
    /// How to interpret the value of 'files'.
    #[arg(long = "match-mode", value_enum, default_value_t = PathMatchMode::Literal)]
    path_match_mode: PathMatchMode,
    /// If true, extract all files.
    #[arg(long, conflicts_with = "files")]
    all: bool,
    /// If set, remove the path from the extracted files.
    #[arg(long)]
    flatten: bool,
    /// How to handle case in MEGA file path names. On windows, file names are treated as
    /// case-insensitive, which isn't true for other operating systems. When working with MEGA files
    /// on other systems, it can be helpful to force file names into a consistent case.
    #[arg(long = "convert-case", value_enum, default_value_t = NameCaseMode::Keep)]
    convert_case_mode: NameCaseMode,
    /// If true, don't actually write the output file. Useful for checking that you have the input
    /// regex/glob correct.
    #[arg(long = "dry-run")]
    dry_run: bool,
}

impl ExtractCmd {
    /// Executes the extract command on a MEGA file.
    pub(crate) fn run(&self) {
        let data = load_file(&self.reader.source);

        let mut files_to_extract = HashMap::<MegPathBuf, Vec<File>>::new();
        if self.all {
            for file in parse(self.reader.mega_version, &data).map(unwrap_file_or_exit) {
                files_to_extract
                    .entry(file.path().unwrap().to_owned())
                    .or_default()
                    .push(file);
            }
        } else if self.files.len() > 0 {
            match self.path_match_mode {
                PathMatchMode::Literal => {
                    let mut had_errors = false;
                    let paths_to_extract: HashSet<_> = self
                        .files
                        .iter()
                        .filter_map(|file| match MegPath::from_str(file) {
                            Ok(path) => Some(path),
                            Err(e) => {
                                eprintln!("{file} is not a valid MEGA path: {e}");
                                had_errors = true;
                                None
                            }
                        })
                        .collect();
                    if had_errors {
                        exit(1);
                    }
                    for file in parse(self.reader.mega_version, &data).map(unwrap_file_or_exit) {
                        if paths_to_extract.contains(file.path().unwrap()) {
                            files_to_extract
                                .entry(file.path().unwrap().to_owned())
                                .or_default()
                                .push(file);
                        }
                    }
                    let unmatched_paths: BTreeSet<_> = paths_to_extract
                        .into_iter()
                        .filter(|&path| !files_to_extract.contains_key(path))
                        .collect();
                    if !unmatched_paths.is_empty() {
                        eprintln!("The following paths were not found in the MEGA file:");
                        for path in unmatched_paths {
                            eprintln!("  {path}");
                        }
                        exit(1);
                    }
                }
                PathMatchMode::Glob => {
                    let mut had_errors = false;
                    let globs: Vec<_> = self
                        .files
                        .iter()
                        .filter_map(|glob| {
                            let res = GlobBuilder::new(glob)
                                .backslash_escape(true)
                                .case_insensitive(true)
                                .empty_alternates(true)
                                .literal_separator(true)
                                .build();
                            match res {
                                Ok(glob) => Some(glob),
                                Err(e) => {
                                    eprintln!("{glob} is not a valid glob: {e}");
                                    had_errors = true;
                                    None
                                }
                            }
                        })
                        .collect();
                    if had_errors {
                        exit(1);
                    }
                    let globs = match GlobSet::new(globs) {
                        Ok(globs) => globs,
                        Err(e) => {
                            eprintln!("Unable to combine globs into a globset: {e}");
                            exit(1);
                        }
                    };
                    for file in parse(self.reader.mega_version, &data).map(unwrap_file_or_exit) {
                        // Like the rust standard library, globset annoyingly has non-configurable
                        // platform-dependent behavior for how it handles path separators. Before we can
                        // match anything, we need to first convert all '\\' to '/'. globset runs their
                        // own normalization, but only on windows.
                        let path = normalize_path_to_unix(file.raw_name().unwrap());
                        let candidate = Candidate::from_bytes(&path);
                        if globs.is_match_candidate(&candidate) {
                            files_to_extract
                                .entry(file.path().unwrap().to_owned())
                                .or_default()
                                .push(file);
                        }
                    }
                }
                PathMatchMode::Regex => {
                    let regex_set = match RegexSetBuilder::new(&self.files).unicode(false).build() {
                        Ok(set) => set,
                        Err(e) => {
                            eprintln!("Error parsing regex: {e}");
                            exit(1);
                        }
                    };
                    for file in parse(self.reader.mega_version, &data).map(unwrap_file_or_exit) {
                        if regex_set.is_match(file.raw_name().unwrap()) {
                            files_to_extract
                                .entry(file.path().unwrap().to_owned())
                                .or_default()
                                .push(file);
                        }
                    }
                }
            }
        } else {
            eprintln!("Must specify either a list of MEGA file paths to extract or --all");
            exit(1);
        };

        if files_to_extract.is_empty() {
            eprintln!("Nothing to extract.");
            exit(2);
        }

        if let Some(ref out) = self.out {
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

            if self.dry_run {
                println!(
                    "Would extract {} bytes from {} to {}",
                    file.contents().len(),
                    file.path().unwrap(),
                    out.display()
                );
            } else {
                println!("Extracting {} to {}", file.path().unwrap(), out.display());
                if let Err(e) = std::fs::write(out, file.contents()) {
                    eprintln!("Writing to output file {} failed: {e}", out.display());
                    exit(1);
                }
            }
            println!("Done");
            exit(0);
        }

        let base_path = match self.out_dir.as_deref() {
            Some(dir) => dir,
            None => Path::new(""),
        };
        for files in files_to_extract.into_values() {
            for (idx, file) in files.iter().enumerate() {
                let mut out_path = file.path().unwrap().to_path_buf();
                self.convert_case_mode.convert(&mut out_path);
                if files.len() > 1 {
                    out_path.add_extension(format!("{idx}"));
                }
                let out_path = base_path.join(out_path);
                if self.dry_run {
                    println!(
                        "Would extract {} bytes from {} to {}",
                        file.contents().len(),
                        file.path().unwrap(),
                        out_path.display()
                    );
                } else {
                    println!(
                        "Extracting {} to {}",
                        file.path().unwrap(),
                        out_path.display()
                    );
                    if let Some(parent) = out_path.parent() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            eprintln!(
                                "Failed to create output directory {}: {e}",
                                parent.display()
                            );
                            eprintln!("Skipping file {}", file.path().unwrap());
                            continue;
                        }
                    }
                    if let Err(e) = std::fs::write(&out_path, file.contents()) {
                        eprintln!("Failed to open output file {}: {e}", out_path.display());
                        eprintln!("Skipping file {}", file.path().unwrap());
                        continue;
                    }
                }
            }
        }
        println!("Done");
    }
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
        MegVersion::V1 => Box::new(parser::parse_v1(data)),
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

/// Replaces all instances of '\\' with '/', irrespective of the current platform.
fn normalize_path_to_unix(path: &[u8]) -> Cow<'_, [u8]> {
    let mut path = Cow::Borrowed(path);
    for i in 0..path.len() {
        if path[i] == b'\\' {
            path.to_mut()[i] = b'/';
        }
    }
    path
}
