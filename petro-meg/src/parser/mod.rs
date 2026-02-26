use std::usize;

use thiserror::Error;
use tracing::warn;

use crate::header::FileRecordV1V2;
use crate::path::{MegPath, MegPathError};

pub use v1::{parse as parse_v1, parse_opt as parse_v1_opt};

mod v1;

/// Parser options for the MEGA file parser.
#[derive(Debug, Clone)]
pub struct ParseOptions {
    /// Whether to validate filename CRCs. If true, a mismatched CRC is an error instead of a
    /// warning.
    ///
    /// Default: true.
    validate_crc: bool,
    /// Whether to validate file indexes. If true, a mismatched file index is an error instead of a
    /// warning.
    ///
    /// Default: true.
    validate_index: bool,
    /// If true, the index of the name must be in the valid range. If false, an out of bounds name
    /// index will be treated as empty.
    ///
    /// Default: true.
    validate_name_index: bool,
    /// Whether to validate file names. If true, the file name will be validated, if not, arbitrary
    /// bytes will be allowed in file names.
    ///
    /// Default: true.
    validate_names: bool,
    /// If true, validate the file bounds. If false, the bounds will still be checked, but invalid
    /// bounds will simply slice to as much of the file as is available in bounds.
    ///
    /// Default: true.
    validate_file_bounds: bool,
}

impl ParseOptions {
    pub const fn new() -> Self {
        Self {
            validate_crc: true,
            validate_index: true,
            validate_name_index: true,
            validate_names: true,
            validate_file_bounds: true,
        }
    }
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// MegParseError.
#[derive(Error, Debug)]
pub enum MegParseError {
    #[error("Mega file was only {file_size} but {min_bytes} are needed to parse the header")]
    NotEnoughBytesForHeader { file_size: usize, min_bytes: usize },
    #[error(
        "Hit EOF while parsing the names table. Expected to parse {num_names} and was about to \
        parse the name at index {name_index} from cursor position {cursor_position}, but the file \
        was only {file_size} bytes"
    )]
    EofDuringNameParsing {
        file_size: usize,
        cursor_position: usize,
        num_names: u32,
        name_index: u32,
    },
    #[error(
        "Hit EOF while retrieving the data for the File records table. Expected to have \
        {num_files} File records but file length was only {file_size} bytes and the cursor \
        position is {cursor_position}"
    )]
    EofDuringFileRecordsParsing {
        file_size: usize,
        cursor_position: usize,
        num_files: u32,
    },
    /// Name Validation was enabled, and an invalid MegPath name was encountered.
    #[error("The File name at index {name_index} in the MEGA file was not valid: {path_error}")]
    InvalidName {
        name_index: u32,
        path_error: MegPathError,
    },
    #[error(
        "The File record at index {file_index} expected its name to have a crc of {expected_crc}, \
        but the name's actual crc was {actual_crc}"
    )]
    InvalidCrc {
        file_index: usize,
        expected_crc: u32,
        actual_crc: u32,
    },
    #[error(
        "The File record at index {file_index} specified that it should be at index \
        {index_from_record}"
    )]
    InvalidFileIndex {
        file_index: usize,
        index_from_record: usize,
    },
    #[error(
        "The File record at index {file_index} referenced name index {name_index} but there are \
        only {num_names} names defined"
    )]
    NameIndexOutOfRange {
        file_index: usize,
        name_index: usize,
        num_names: usize,
    },
    #[error(
        "The File record at index {file_index} expected data at position {start} with length \
        {size}, but the file is only {file_size} bytes."
    )]
    FileOutOfBounds {
        file_index: usize,
        /// The full file size of the MEGA file.
        file_size: usize,
        /// Start position of the file record within the mega file.
        start: u32,
        size: u32,
    },
}

/// Stores a name alongside its validated form.
#[derive(Debug, Default, Clone, Copy)]
struct ValidatedName<'b> {
    /// The raw bytes of the name.
    raw_name: &'b [u8],
    /// If the name was valid, this is raw_name cast to a MegPath. Otherwise this is None.
    validated_name: Option<&'b MegPath>,
}

fn record_v1v2_to_file<'b>(
    options: &ParseOptions,
    bytes: &'b [u8],
    names: &[ValidatedName<'b>],
    file_index: usize,
    record: &FileRecordV1V2,
) -> Result<File<'b>, MegParseError> {
    if record.index as usize != file_index {
        let err = MegParseError::InvalidFileIndex {
            file_index,
            index_from_record: record.index as usize,
        };
        if options.validate_index {
            return Err(err);
        }
        warn!("{err}");
    }
    let name = match names.get(record.name as usize) {
        Some(name) => Some(name),
        None => {
            let err = MegParseError::NameIndexOutOfRange {
                file_index,
                name_index: record.name as usize,
                num_names: names.len(),
            };
            if options.validate_name_index {
                return Err(err);
            }
            warn!("{err}");
            None
        }
    };
    // If there is no name, skip CRC validation as that will almost certainly fail, and we'll
    // already have logged a warning about the name index being out of range if we got this far.
    if let Some(name) = name {
        let actual_crc = crc32fast::hash(name.raw_name);
        if record.crc != actual_crc {
            let err = MegParseError::InvalidCrc {
                file_index,
                expected_crc: record.crc,
                actual_crc,
            };
            if options.validate_crc {
                return Err(err);
            }
            warn!("{err}");
        }
    }

    let start = record.start as usize;
    let bound = match start.checked_add(record.size as usize) {
        Some(end) if start <= bytes.len() && end <= bytes.len() => start..end,
        end => {
            let err = MegParseError::FileOutOfBounds {
                file_index,
                file_size: bytes.len(),
                start: record.start,
                size: record.size,
            };
            if options.validate_file_bounds {
                return Err(err);
            }
            warn!("{err}");
            let start = start.min(bytes.len());
            let end = end.unwrap_or(usize::MAX).min(bytes.len());
            start..end
        }
    };
    let contents = &bytes[bound];

    let (raw_name, path) = match name {
        Some(name) => (Some(name.raw_name), name.validated_name),
        None => (None, None),
    };

    Ok(File {
        raw_name,
        path,
        contents,
        expected_size: record.size as usize,
    })
}

/// Entry for a file read from the mega file files table.
#[derive(Debug, Clone)]
pub struct File<'b> {
    /// The raw bytes of the name.
    raw_name: Option<&'b [u8]>,
    /// The validated MEGA file path. If Some, this is the same as raw_name.
    path: Option<&'b MegPath>,
    /// Contents of the file.
    contents: &'b [u8],
    /// The expected size of the file from the header. May differ from the len of contents if file
    /// bounds validation was skipped.
    expected_size: usize,
}

impl<'b> File<'b> {
    /// Gets the raw name from the file. If the name index was out of range and name index
    /// validation was skipped, this will be empty. If name index validation was enabled, this will
    /// always be Some.
    pub fn raw_name(&self) -> Option<&[u8]> {
        self.raw_name
    }

    /// Gets the mega file path. This is generally the same as Name, but with validation. This may
    /// be `None` if either name index validation was skipped, and no name matched, or name
    /// validation was skipped and the name was invalid. If both name validation and name index
    /// validation are enabled, this will always be Some.
    pub fn path(&self) -> Option<&MegPath> {
        self.path
    }

    /// Contents of the file. If file bounds validation was skipped, this may be less than the size
    /// indended in the MEGA file. If bounds validation was enabled, this is the complete file
    /// contents.
    pub fn contents(&self) -> &[u8] {
        self.contents
    }

    /// If true, the contents represents the complete file. If false, the contents is only partial,
    /// some of the file contents was outside the bounds of the MEGA file. If file bounds validation
    /// was enabled, this will always be true.
    pub fn is_complete(&self) -> bool {
        self.contents.len() == self.expected_size
    }
}
