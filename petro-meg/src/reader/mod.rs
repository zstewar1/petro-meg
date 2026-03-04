use std::io::{Read, Seek};
use std::ops::Range;
use std::{io, usize};

use byteorder::{LE, ReadBytesExt as _};
use thiserror::Error;
use tracing::{instrument, trace, warn};

use crate::crypto::{DecryptingReader, Key, round_up_to_block};
use crate::path::{MegPath, MegPathBuf, MegPathError, WIN_PATH_LIMIT};

mod any_version;
mod version1;
mod version2;
mod version3;

pub const ID2: u32 = 0x3F7D70A4;

/// Parser options for the MEGA file parser.
#[derive(Debug, Clone)]
pub struct MegReadOptions {
    /// Whether to validate filename CRCs. If `true`, a mismatched CRC is an error instead of a
    /// warning.
    ///
    /// Default: `true`.
    validate_crc: bool,
    /// Whether to validate file indexes. If `true`, a mismatched file index is an error instead of a
    /// warning.
    ///
    /// Default: `true`.
    validate_index: bool,
    /// Whether to validate that file names are less than the windows 260 character limit.
    ///
    /// Default: `true`.
    validate_name_length: bool,
    /// Whether to validate that the number of names and number of files match.
    ///
    /// Default: `true`.
    validate_name_count: bool,
    /// Whether to validate that the names of files are unique.
    ///
    /// Default: `true`.
    validate_names_unique: bool,
    /// Validate that file start is above data start.
    ///
    /// Default: `true`.
    validate_file_start_data_start: bool,
    /// Validate that all files in an encrypted file are also encrypted.
    ///
    /// Default: `true`.
    validate_consistent_encryption: bool,
    /// If true, file readers will be wrapped in a reader which counts bytes read to ensure the
    /// whole file is complete and returns UnexpectedEof if any contents are missing.
    ///
    /// Default: `true`.
    validate_file_complete: bool,
    /// Encryption key and initial vector used for decrypting V3 MEGA files.
    ///
    /// Default: `None`.
    key: Option<Key>,
}

impl MegReadOptions {
    /// Create a a new default options.
    pub const fn new() -> Self {
        Self {
            validate_crc: true,
            validate_index: true,
            validate_name_length: true,
            validate_names_unique: true,
            validate_name_count: true,
            validate_file_start_data_start: true,
            validate_consistent_encryption: true,
            validate_file_complete: true,
            key: None,
        }
    }

    /// Set the crypto key used for reading.
    pub const fn set_key(&mut self, key: Option<Key>) -> &mut Self {
        self.key = key;
        self
    }
}

impl Default for MegReadOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// MegParseError.
#[derive(Error, Debug)]
pub enum MegReadError {
    /// Encountered an IO error while parsing.
    #[error("Encountered an IO error while parsing: {0}")]
    IoError(#[from] io::Error),
    /// For V2 or V3, the first two words were not recognized as the correct file id.
    #[error("MEGA file header had an unrecognized file ID: 0x{id1:08X} 0x{id2:08X}")]
    InvalidFileId { id1: u32, id2: u32 },
    /// For V3 only, the header had the 'encrypted' id version/flag but no crypto key was available
    /// in the provided reader options.
    #[error(
        "MEGA file header indicated that it was encrypted, but no key was provided to decrypt it"
    )]
    MissingKey,
    /// Name count validation was enabled and the number of files listed differs from the number of
    /// filenames.
    #[error(
        "MEGA file header had a different number of files and file names. \
        num_filenames={num_filenames}, num_files={num_files}"
    )]
    NameFileCountMismatch { num_filenames: u32, num_files: u32 },
    /// Name Length Validation was enabled, and a name was encountered that exceeded the length
    /// limit.
    #[error(
        "The File name at index {name_index} exceeded the Windows 260 character limit for file \
        paths. Actual length: {name_len}"
    )]
    NameTooLong { name_index: u32, name_len: usize },
    /// An invalid MegPath name was encountered.
    #[error("The File name at index {name_index} in the MEGA file was not valid: {path_error}")]
    InvalidName {
        name_index: u32,
        path_error: MegPathError,
    },
    /// A V3 File record had a flags value other than 0 or 1.
    #[error("The file record at index {file_index} had unrecognized flags: 0x{flags:04X}")]
    InvalidFileFlags { file_index: u32, flags: u16 },
    /// A V3 File record had an encryption flag which didn't match the containing MEGA file.
    #[error(
        "The file record at index {file_index} had encryption={record_encrypted} but the \
        containing MEGA file had encryption={meg_encrypted}"
    )]
    MismatchedEncryption {
        file_index: u32,
        /// Whether the MEGA file used encryption.
        meg_encrypted: bool,
        /// Whether this particular file record used encryption.
        record_encrypted: bool,
    },
    #[error(
        "The File record at index {file_index} specified that it should be at index \
        {index_from_record}"
    )]
    InvalidFileIndex {
        file_index: u32,
        index_from_record: u32,
    },
    #[error(
        "The File record at index {file_index} referenced name index {name_index} but there are \
        only {num_names} names defined"
    )]
    NameIndexOutOfRange {
        file_index: u32,
        name_index: u32,
        num_names: u32,
    },
    #[error(
        "The File record at index {file_index} referenced name index {name_index} but another file \
        already has that name"
    )]
    NameAlreadyUsed {
        file_index: u32,
        name_index: u32,
        num_names: u32,
    },
    #[error(
        "The File record at index {file_index} expected its name to have a crc of {expected_crc}, \
        but the name's actual crc was {actual_crc}"
    )]
    InvalidCrc {
        file_index: u32,
        expected_crc: u32,
        actual_crc: u32,
    },
    #[error(
        "The File record at index {file_index} expected data at position {file_start}, but the \
        MEGA header listed {data_start} as the start of the file data section"
    )]
    FileBelowDataStart {
        file_index: u32,
        file_start: u32,
        data_start: u32,
    },
}

/// Trait for implementing MegMetaReader for various MEGA file versions.
pub trait ReadMegMeta: Sized + private::Sealed {
    fn read_meg_meta<R: Read>(self, reader: R) -> Result<Vec<FileEntry>, MegReadError> {
        const DEFAULT_OPTIONS: &'static MegReadOptions = &MegReadOptions::new();
        self.read_meg_meta_opt(reader, DEFAULT_OPTIONS)
    }

    fn read_meg_meta_opt<R: Read>(
        self,
        reader: R,
        options: &MegReadOptions,
    ) -> Result<Vec<FileEntry>, MegReadError>;
}

mod private {
    pub trait Sealed {}

    impl Sealed for crate::version::MegVersion {}
    impl Sealed for Option<crate::version::MegVersion> {}
    impl Sealed for crate::version::MegV1 {}
    impl Sealed for crate::version::MegV2 {}
    impl Sealed for crate::version::MegV3 {}
    impl Sealed for crate::version::GuessVersion {}
}

/// Version-specific ReaderState. Provides hooks for version-specific operations.
trait ReaderState: std::fmt::Debug + Sized {
    /// Gets the number of filename entries in the filenames table.
    fn num_filenames(&self) -> u32;

    /// Gets the humber of files in the files table.
    fn num_files(&self) -> u32;

    /// Read the names from the MEGA file.
    fn read_names<R: Read>(
        &self,
        reader: &mut R,
        options: &MegReadOptions,
    ) -> Result<Vec<Option<MegPathBuf>>, MegReadError> {
        read_names(reader, self.num_filenames(), options)
    }

    /// Read a single file record from the file.
    ///
    /// Index is provided only for error messages.
    fn read_file_record<R: Read>(
        &self,
        reader: &mut R,
        options: &MegReadOptions,
        index: u32,
    ) -> Result<FileRecord, MegReadError>;
}

/// A raw file record, not yet interpreted.
#[derive(Debug)]
struct FileRecord {
    /// Encryption flag. Only used by V3 files.
    encrypted: bool,
    /// CRC-32 of the filename.
    crc: u32,
    /// Index of this record in the records table.
    index: u32,
    /// Size of this file in the data section.
    size: u32,
    /// Start of this file relative to the start of the file.
    start: u32,
    /// Index of the name in the names table.
    name: u32,
}

#[instrument(skip_all)]
fn read_meg_meta<S: ReaderState, R: Read>(
    state: S,
    mut reader: R,
    options: &MegReadOptions,
) -> Result<Vec<FileEntry>, MegReadError> {
    trace!("Read Options: {options:?}");
    trace!("Header Read: {state:?}");
    if state.num_filenames() != state.num_files() {
        let err = MegReadError::NameFileCountMismatch {
            num_filenames: state.num_filenames(),
            num_files: state.num_files(),
        };
        if options.validate_name_count {
            return Err(err);
        }
        warn!("{err}");
    }
    let mut names = state.read_names(&mut reader, options)?;

    let mut files = Vec::with_capacity(state.num_files() as usize);
    for file_index in 0..state.num_files() {
        let record = state.read_file_record(&mut reader, options, file_index)?;
        if record.index != file_index {
            let err = MegReadError::InvalidFileIndex {
                file_index,
                index_from_record: record.index,
            };
            if options.validate_index {
                return Err(err);
            }
            warn!("{err}");
        }
        let name =
            names
                .get_mut(record.name as usize)
                .ok_or(MegReadError::NameIndexOutOfRange {
                    file_index,
                    name_index: record.name,
                    num_names: state.num_filenames(),
                })?;
        let name = if options.validate_names_unique {
            name.take().ok_or(MegReadError::NameAlreadyUsed {
                file_index,
                name_index: record.name,
                num_names: state.num_filenames(),
            })?
        } else {
            name.clone().unwrap()
        };
        let actual_crc = crc32fast::hash(name.as_bytes());
        if record.crc != actual_crc {
            let err = MegReadError::InvalidCrc {
                file_index,
                expected_crc: record.crc,
                actual_crc,
            };
            if options.validate_crc {
                return Err(err);
            }
            warn!("{err}");
        }
        files.push(FileEntry {
            name,
            start: record.start,
            size: record.size,
            encrypted: record.encrypted,
        });
    }

    Ok(files)
}

/// Read all the file names from the given reader.
fn read_names<R: Read>(
    mut reader: R,
    num_filenames: u32,
    options: &MegReadOptions,
) -> Result<Vec<Option<MegPathBuf>>, MegReadError> {
    let mut names = Vec::with_capacity(num_filenames as usize);
    // Read the number of entries needed to fill the names table.
    for name_index in 0..num_filenames {
        // All versions use the same u16 file name length.
        let name_len = reader.read_u16::<LE>()? as usize;
        if name_len > WIN_PATH_LIMIT {
            let err = MegReadError::NameTooLong {
                name_index,
                name_len,
            };
            if options.validate_name_length {
                return Err(err);
            }
            warn!("{err}");
        }
        let mut raw_name = vec![0u8; name_len];
        reader.read_exact(&mut raw_name)?;
        let name = match MegPathBuf::from_bytes(raw_name) {
            Ok(name) => name,
            Err(path_error) => {
                return Err(MegReadError::InvalidName {
                    name_index,
                    path_error,
                });
            }
        };
        trace!("Read name at index {name_index}: {name}");
        names.push(Some(name));
    }
    Ok(names)
}

/// Common implementation for read_file_record for both V1 and V2 MEGA files.
///
/// V1 and V2 are never encrypted and use a 32 bit name field.
fn read_v1v2_file_record<R: Read>(reader: &mut R) -> Result<FileRecord, MegReadError> {
    Ok(FileRecord {
        encrypted: false,
        crc: reader.read_u32::<LE>()?,
        index: reader.read_u32::<LE>()?,
        size: reader.read_u32::<LE>()?,
        start: reader.read_u32::<LE>()?,
        name: reader.read_u32::<LE>()?,
    })
}

/// Entry for a file read from the mega file files table.
pub struct FileEntry {
    /// The path/name of the file.
    name: MegPathBuf,
    /// Start offset of the this file within the MEGA file.
    start: u32,
    /// Size of this file within the MEGA file.
    size: u32,
    /// Whether this file was encrypted.
    encrypted: bool,
}

impl FileEntry {
    /// Gets the MEGA file path that the file entry was stored under.
    pub fn name(&self) -> &MegPath {
        &self.name
    }

    /// Get the range of the original MEGA file occupied by this file's contents.
    pub fn range(&self) -> Range<usize> {
        let start: usize = self
            .start
            .try_into()
            .expect("start was outside the range of usize");
        let size: usize = self
            .size
            .try_into()
            .expect("size was outside the range of usize");
        let end = start
            .checked_add(size)
            .expect("start+size overflowed usize");
        start..end
    }

    /// Get the size of the file's contents in the MEGA file.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Returns true if the file was encrypted.
    pub fn encrypted(&self) -> bool {
        self.encrypted
    }

    /// Extract this file from the given reader. The provided reader must represent the same MEGA
    /// file that this FileEntry belongs to.
    pub fn extract_from<'read, R: Read + Seek + 'read>(
        &self,
        mut reader: R,
        options: &MegReadOptions,
    ) -> io::Result<FileSegmentReader<'read>> {
        let new_position = reader.seek(io::SeekFrom::Start(self.start as u64))?;
        if new_position != self.start as u64 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "Tried to seek to {} but position after seek was {new_position}",
                    self.start
                ),
            ));
        }

        #[inline(always)]
        fn maybe_check_len<'read, R: Read + 'read>(
            reader: io::Take<R>,
            options: &MegReadOptions,
        ) -> Box<dyn Read + 'read> {
            if options.validate_file_complete {
                Box::new(ReadLenChecker { inner: reader })
            } else {
                Box::new(reader)
            }
        }

        let reader = if self.encrypted {
            let Some(ref key) = options.key else {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    MegReadError::MissingKey,
                ));
            };
            let amount_to_read = round_up_to_block(self.size as u64);
            // The inner 'take' is to grab the correct block contents for the file, rounded up to
            // the next full block.
            let reader = reader.take(amount_to_read);
            // Then we decrypt.
            let reader = DecryptingReader::new(reader, key);
            // Then we have to apply another Take to restrict the output to only the portion of the
            // last block that's actually supposed to be in the file.
            let reader = reader.take(self.size as u64);
            maybe_check_len(reader, options)
        } else {
            // If its not encrypted we just take the file contents and maybe check the length limit.
            let reader = reader.take(self.size as u64);
            maybe_check_len(reader, options)
        };
        Ok(FileSegmentReader(reader))
    }
}

/// Reads a segment representing a single FileEntry from a reader representing a larger MEGA file.
pub struct FileSegmentReader<'read>(Box<dyn Read + 'read>);

impl<'read> Read for FileSegmentReader<'read> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        self.0.read_vectored(bufs)
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.0.read_to_end(buf)
    }

    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        self.0.read_to_string(buf)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.0.read_exact(buf)
    }
}

/// Checks that a read returns the full file contents.
struct ReadLenChecker<R> {
    inner: io::Take<R>,
}

impl<R: Read> Read for ReadLenChecker<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        if read == 0 && self.inner.limit() > 0 {
            Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Data for this file is incomplete.",
            ))
        } else {
            Ok(read)
        }
    }
}
