use tracing::{debug, instrument, warn};

use crate::header::{FileRecordV1V2, HeaderV1, split_off_name};
use crate::parser::{File, MegParseError, ParseOptions};
use crate::path::{MegPath, WIN_PATH_LIMIT};

use super::{ValidatedName, record_v1v2_to_file};

/// Parse a V1 MegaFile.
#[instrument(skip(bytes))]
pub fn parse(bytes: &[u8]) -> MegFileContentsV1<'_, 'static> {
    const OPTIONS: &'static ParseOptions = &ParseOptions::new();
    MegFileContentsV1::new(bytes, OPTIONS)
}

/// Parse a V1 MegaFile, with specified options.
#[instrument(skip(bytes))]
pub fn parse_opt<'b, 'o>(bytes: &'b [u8], options: &'o ParseOptions) -> MegFileContentsV1<'b, 'o> {
    MegFileContentsV1::new(bytes, options)
}

/// Iterator over the contents of a MEGA file.
pub struct MegFileContentsV1<'b, 'o> {
    options: &'o ParseOptions,
    /// Source bytes of the entire MEGA file.
    bytes: &'b [u8],
    /// Names from the MegFile. None if we haven't read the names section yet. These are not
    /// validated.
    names: Option<Vec<ValidatedName<'b>>>,
    /// Slice of the source bytes containing the the file records. If `names` is `None`, then this
    /// hasn't been
    file_records: &'b [u8],
    /// Index of the next file record in file_records.
    next_index_front: usize,
    /// If an error was encountered while parsing, this is used to fuse the iterator.
    errored_out: bool,
}

impl<'b, 'o> MegFileContentsV1<'b, 'o> {
    fn new(bytes: &'b [u8], options: &'o ParseOptions) -> Self {
        Self {
            options,
            bytes,
            names: None,
            // Not actually the file records yet.
            file_records: bytes,
            next_index_front: 0,
            errored_out: false,
        }
    }
}

impl<'b, 'o> Iterator for MegFileContentsV1<'b, 'o> {
    type Item = Result<File<'b>, MegParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.errored_out {
            return None;
        }
        let names = match get_names_or_start_parse(
            self.options,
            self.bytes,
            &mut self.names,
            &mut self.file_records,
        ) {
            Ok(names) => names,
            Err(err) => {
                self.errored_out = true;
                return Some(Err(err));
            }
        };
        if self.file_records.is_empty() {
            None
        } else {
            // In V1, we pre-check the record length so there's no chance of this failing.
            let (record, rest) = FileRecordV1V2::split_off(self.file_records).unwrap();
            self.file_records = rest;
            let expected_index = self.next_index_front;
            self.next_index_front += 1;

            match record_v1v2_to_file(
                self.options,
                self.bytes,
                names,
                expected_index,
                None,
                &record,
            ) {
                Ok(file) => Some(Ok(file)),
                Err(err) => {
                    self.errored_out = true;
                    Some(Err(err))
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.names.is_none() {
            // If we haven't parsed names yet, we don't know how much is left, but we do know that
            // it can't be more than fits in a u32.
            (0, Some(u32::MAX as usize))
        } else if self.errored_out {
            // If we errored out, we return nothing.
            (0, Some(0))
        } else {
            // If we have parsed the header, then we know exactly the maximum, though we might yield
            // fewer elements if we error, so the minimum is either 0 if we are out of file records
            // or 1 if there is at least one file record left to parse.
            let len = self.file_records.len() / size_of::<FileRecordV1V2>();
            (len.min(1), Some(len))
        }
    }
}

/// If names is Some, return its contents, otherwise start parsing and return the resulting names.
fn get_names_or_start_parse<'b, 'n>(
    options: &ParseOptions,
    bytes: &'b [u8],
    names: &'n mut Option<Vec<ValidatedName<'b>>>,
    file_records: &mut &'b [u8],
) -> Result<&'n [ValidatedName<'b>], MegParseError> {
    Ok(match names {
        Some(names) => names,
        None => {
            let (n, r) = parse_header_and_names(bytes, options)?;
            debug!(
                "File records start offset: {}",
                r.as_ptr() as usize - bytes.as_ptr() as usize
            );
            *file_records = r;
            names.insert(n)
        }
    })
}

/// Reads the v1 header, then parses the names table and returns a slice to use for the file records
/// table.
fn parse_header_and_names<'b>(
    full_file: &'b [u8],
    options: &ParseOptions,
) -> Result<(Vec<ValidatedName<'b>>, &'b [u8]), MegParseError> {
    let (header, mut cursor) =
        HeaderV1::split_off(full_file).ok_or(MegParseError::NotEnoughBytesForHeader {
            file_size: full_file.len(),
            min_bytes: size_of::<HeaderV1>(),
        })?;
    let mut names = Vec::with_capacity(header.num_filenames as usize);
    // Read the number of entries needed to fill the names table.
    for name_index in 0..header.num_filenames {
        let (raw_name, rest) =
            split_off_name(cursor).ok_or(MegParseError::EofDuringNameParsing {
                file_size: full_file.len(),
                cursor_position: cursor.as_ptr() as usize - full_file.as_ptr() as usize,
                num_names: header.num_filenames,
                name_index,
            })?;
        if raw_name.len() > WIN_PATH_LIMIT {
            let err = MegParseError::NameTooLong {
                name_index,
                name_len: raw_name.len(),
            };
            if options.validate_name_length {
                return Err(err);
            }
            warn!("{err}");
        }
        let validated_name = match MegPath::from_bytes(raw_name) {
            Ok(name) => Some(name),
            Err(path_error) => {
                let err = MegParseError::InvalidName {
                    name_index,
                    path_error,
                };
                if options.validate_names {
                    return Err(err);
                }
                warn!("{err}");
                None
            }
        };
        names.push(ValidatedName {
            raw_name,
            validated_name,
        });
        cursor = rest;
    }
    // In V1 and V2 files table starts directly after file names table.
    let files = FileRecordV1V2::slice_n(cursor, header.num_files as usize).ok_or(
        MegParseError::EofDuringFileRecordsParsing {
            file_size: full_file.len(),
            cursor_position: cursor.as_ptr() as usize - full_file.as_ptr() as usize,
            num_files: header.num_files,
        },
    )?;
    Ok((names, files))
}
