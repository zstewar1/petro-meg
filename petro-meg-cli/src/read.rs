use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, BufReader};
use std::num::ParseIntError;
use std::path::{Path, PathBuf};
use std::process::exit;

use clap::{Args, ValueEnum};
use globset::{Candidate, GlobBuilder, GlobSet};
use petro_meg::crypto::Key;
use petro_meg::path::MegPath;
use petro_meg::reader::{FileEntry, FileSegmentReader, MegReadOptions, ReadMegMeta};
use petro_meg::version::MegVersion;
use regex::bytes::RegexSetBuilder;
use thiserror::Error;

#[derive(Args)]
struct ReaderArgs {
    /// MEGA file to read from.
    source: PathBuf,

    /// Which MEGA file version to read the file as.
    #[arg(short = 'v', long = "meg-version")]
    mega_version: Option<MegVersion>,

    /// Key and initial vector for encrypted MEGA file reading. This should consist of two 128 bit
    /// hexadecimal numbers separated by a colon. The first value is the key and the second is the
    /// initial vector.
    #[arg(long, value_parser = parse_key)]
    key: Option<Key>,
}

#[derive(Error, Debug)]
enum ParseKeyError {
    #[error("Expected format <KEY>:<IV>, but no ':' was found")]
    NoSeparator,
    #[error("Expected format <KEY>:<IV>, but found an extra ':'")]
    ExtraSeparator,
    #[error("Expected key to be 32 hex characters but got {len} bytes")]
    InvalidKeyLen { len: usize },
    #[error("Expected initial vector to be 32 hex characters but got {len} bytes")]
    InvalidIVLen { len: usize },
    #[error("Could not parse the key: {0}")]
    InvalidKey(ParseIntError),
    #[error("Could not parse the initial vector: {0}")]
    InvalidIV(ParseIntError),
}

/// Parse a MEGA file key out of a string consisting of <HEX KEY>:<HEX IV>
fn parse_key(s: &str) -> Result<Key, ParseKeyError> {
    let s = s.trim();
    let mut split = s.split(':');
    // The first output of split should always be Some, since even the empty string produces at
    // least a single empty string result.
    let key = split.next().ok_or(ParseKeyError::NoSeparator)?.trim();
    let iv = split.next().ok_or(ParseKeyError::NoSeparator)?.trim();
    if split.next().is_some() {
        return Err(ParseKeyError::ExtraSeparator);
    }
    if key.len() != 32 {
        return Err(ParseKeyError::InvalidKeyLen { len: key.len() });
    }
    if iv.len() != 32 {
        return Err(ParseKeyError::InvalidIVLen { len: iv.len() });
    }
    let key = u128::from_str_radix(key, 16).map_err(ParseKeyError::InvalidKey)?;
    let iv = u128::from_str_radix(iv, 16).map_err(ParseKeyError::InvalidIV)?;
    // The correct byte order here should be big endian because number parsing treats the first
    // bytes in the input as the high order bytes.
    Ok(Key::new(key.to_be_bytes(), iv.to_be_bytes()))
}

/// Holds the version, read options and opened MEGA file.
struct ReadContext {
    /// Version of the MEGA file being read.
    mega_version: Option<MegVersion>,
    /// File being read.
    file: File,
    /// Read options used for the reader.
    options: MegReadOptions,
}

impl ReadContext {
    /// Read the MEGA file metadata for the context.
    fn read_meg_meta(&mut self) -> Vec<FileEntry> {
        let res = self
            .mega_version
            .read_meg_meta_opt(BufReader::new(&mut self.file), &self.options);
        match res {
            Ok(files) => files,
            Err(e) => {
                eprintln!("Failed to read the MEGA file files list: {e}");
                exit(1);
            }
        }
    }

    /// Gets the reader for the given file.
    fn read_file(&mut self, file: &FileEntry) -> FileSegmentReader<'_> {
        match file.extract_from(&mut self.file, &self.options) {
            Ok(segment) => segment,
            Err(e) => {
                eprintln!("Unable to create file extractor for {}: {e}", file.name());
                exit(1);
            }
        }
    }
}

#[derive(Args)]
pub(crate) struct ListCmd {
    #[command(flatten)]
    reader: ReaderArgs,
}

impl ListCmd {
    /// Executes the list command on a MEGA file.
    pub(crate) fn run(self) {
        let mut context = args_to_context(self.reader);
        let files = context.read_meg_meta();
        for file in files {
            println!("{}: {}", file.name(), file.size());
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
    /// If set, print the matched files to stdout instead of writing them to disk.
    #[arg(long, conflicts_with = "out_dir", conflicts_with = "out")]
    print: bool,
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
    pub(crate) fn run(self) {
        let mut context = args_to_context(self.reader);

        let mut files_to_extract = context.read_meg_meta();
        // If we are extracting all files, leave the full list, otherwise filter it.
        if !self.all {
            if self.files.is_empty() {
                eprintln!("Must specify either a list of MEGA file paths to extract or --all");
                exit(1);
            }
            match self.path_match_mode {
                PathMatchMode::Literal => {
                    let mut had_errors = false;
                    let mut paths_to_extract: BTreeSet<_> = self
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
                    // We remove matched paths because we currently don't allow setting any option
                    // to allow MEGA files with duplicate paths, so each path in the MEGA file will
                    // be unique if we got this far.
                    files_to_extract.retain(|file| paths_to_extract.remove(file.name()));
                    if !paths_to_extract.is_empty() {
                        eprintln!("The following paths were not found in the MEGA file:");
                        for path in paths_to_extract {
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
                    files_to_extract.retain(|file| {
                        // Like the rust standard library, globset annoyingly has non-configurable
                        // platform-dependent behavior for how it handles path separators. Before we
                        // can match anything, we need to first convert all '\\' to '/'. globset
                        // runs their own normalization, but only on windows.
                        let path = normalize_path_to_unix(file.name().as_bytes());
                        let candidate = Candidate::from_bytes(&path);
                        globs.is_match_candidate(&candidate)
                    });
                }
                PathMatchMode::Regex => {
                    let regex_set = match RegexSetBuilder::new(&self.files).unicode(false).build() {
                        Ok(set) => set,
                        Err(e) => {
                            eprintln!("Error parsing regex: {e}");
                            exit(1);
                        }
                    };
                    files_to_extract.retain(|file| regex_set.is_match(file.name().as_bytes()));
                }
            }
        }

        if files_to_extract.is_empty() {
            eprintln!("Nothing to extract.");
            exit(2);
        }

        if self.print {
            use std::io::Write;
            {
                let mut stdout = std::io::stdout().lock();
                for file in files_to_extract.iter() {
                    let res = writeln!(stdout, "{}: {} bytes:", file.name(), file.size());
                    if let Err(e) = res {
                        eprintln!("Error writing file information to stdout: {e}");
                        exit(1);
                    }
                    if !self.dry_run {
                        let mut segment = context.read_file(file);
                        let res = io::copy(&mut segment, &mut stdout)
                            .and_then(|_| stdout.write_all(b"\n\n"));
                        if let Err(e) = res {
                            eprintln!("Error writing file contents to stdout: {e}");
                            exit(1);
                        }
                    }
                }
            }
            println!("Done");
            exit(0);
        }

        if let Some(ref out) = self.out {
            if files_to_extract.len() > 1 {
                eprintln!("--out can only be used when extracting a single file");
                exit(1);
            }
            let file = files_to_extract.into_iter().next().unwrap();

            if self.dry_run {
                println!(
                    "Would extract {} bytes from {} to {}",
                    file.size(),
                    file.name(),
                    out.display()
                );
            } else {
                println!("Extracting {} to {}", file.name(), out.display());
                if let Err(e) = write_to(out, context.read_file(&file)) {
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
        for file in files_to_extract.iter() {
            let mut out_path = file.name().to_path_buf();
            self.convert_case_mode.convert(&mut out_path);
            let out_path = base_path.join(out_path);
            if self.dry_run {
                println!(
                    "Would extract {} bytes from {} to {}",
                    file.size(),
                    file.name(),
                    out_path.display()
                );
            } else {
                println!("Extracting {} to {}", file.name(), out_path.display());
                if let Some(parent) = out_path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        eprintln!(
                            "Failed to create output directory {}: {e}",
                            parent.display()
                        );
                        eprintln!("Skipping file {}", file.name());
                        continue;
                    }
                }
                if let Err(e) = write_to(&out_path, context.read_file(file)) {
                    eprintln!("Failed to write output file {}: {e}", out_path.display());
                    eprintln!("Skipping file {}", file.name());
                    continue;
                }
            }
        }
        println!("Done");
    }
}

/// Converts the args to a
fn args_to_context(args: ReaderArgs) -> ReadContext {
    let file = match File::open(&args.source) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Unable to open source file {}: {e}", args.source.display());
            exit(1);
        }
    };

    let mut options = MegReadOptions::new();
    options.set_key(args.key);

    ReadContext {
        mega_version: args.mega_version,
        file,
        options,
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

fn write_to(path: &Path, mut reader: impl io::Read) -> io::Result<()> {
    let mut dest = File::create(path)?;
    io::copy(&mut reader, &mut dest)?;
    Ok(())
}
