//! Implements MEGA file writing.

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Cursor, Read, Seek, Write};
use std::u32;

use byteorder::{LE, WriteBytesExt as _};
use thiserror::Error;

use crate::crypto::Key;
use crate::path::{MegPath, MegPathBuf, WIN_PATH_LIMIT, hash_normalized};
use crate::version::MegVersion;
use crate::writer::counter::CountingWriter;

pub use self::any_version::AnyVersionSettings;
pub use self::version3::V3Settings;

mod any_version;
mod counter;
mod version1;
mod version2;
mod version3;

/// Error produced when building a MEGA file.
#[derive(Error, Debug)]
pub enum BuildMegError {
    /// Building failed due to an IO error.
    #[error("Failed to build MEGA file due to io error: {0}")]
    IoError(#[from] io::Error),
    /// The number of files to back exceeded the limit.
    #[error("Tried to pack {files} files, but there is a limit of {limit} files.")]
    TooManyFiles { files: usize, limit: u32 },
    /// Error when a MEGA file's name is too long.
    #[error("Name in the MEGA file is too long: len={len}, limit={limit}")]
    NameTooLong { len: usize, limit: u16 },
    /// Error for when an individual file's size won't fit in u32.
    #[error(
        "One of the files to pack exceeded the 32 bit file size limit, with a size of {size} bytes"
    )]
    FileTooLarge { size: u64 },
    /// Error when adding up the header size and individual file sizes would result in
    #[error(
        "The combined offset needed to pack the headers and files exceeded the 32 bit limit for \
        the file start offset"
    )]
    OffsetTooLarge,
    #[error(
        "A file reported an inaccurate size. Reported size: {reported_size}, actual: {actual_size}"
    )]
    InaccurateFileSize {
        reported_size: u32,
        actual_size: u64,
    },
}

/// Builder for a MEGA file.
#[derive(Debug, Default, Clone)]
pub struct MegBuilder<F, V> {
    version_settings: V,
    files: BTreeMap<CrcPathBuf, F>,
    /// Name length limit to apply. Defaults to the windows name length limit.
    name_length_limit: u16,
}

impl<F, V> MegBuilder<F, V> {
    /// Create a new builder with the given version settings.
    fn new(version_settings: V) -> Self {
        MegBuilder {
            version_settings,
            files: Default::default(),
            name_length_limit: WIN_PATH_LIMIT as u16,
        }
    }

    /// Inserts a file into the MEGA file builder. If another file with the same path was already in
    /// the MEGA file, returns the existing file.
    pub fn insert(&mut self, path: MegPathBuf, file: F) -> Option<F> {
        self.files.insert(path.into(), file)
    }

    /// Remove a file from the MEGA file builder.
    pub fn remove<P>(&mut self, path: &P) -> Option<F>
    where
        P: Borrow<MegPath>,
    {
        self.files.remove::<CrcPath>(path.borrow().into())
    }

    /// Gets the length limit for MEGA file path names.
    ///
    /// The name length limit defaults to 260, based on the traditional Windows path length limit.
    ///
    /// If any file name in the builder has a name longer than this limit, [`build`][Self::build]
    /// will return an error.
    pub fn name_length_limit(&self) -> u16 {
        self.name_length_limit
    }

    /// Sets the length limit for MEGA file path names.
    ///
    /// The name length limit defaults to 260, based on the traditional Windows path length limit.
    ///
    /// If any file name in the builder has a name longer than this limit, [`build`][Self::build]
    /// will return an error.
    ///
    /// The length limit is restricted to a u16 because the MEGA file format encodes path name
    /// lengths using only 16 bits, so it is not possible to store a name longer than [`u16::MAX`].
    pub fn set_name_length_limit(&mut self, len: u16) {
        self.name_length_limit = len;
    }
}

impl<F> MegBuilder<F, AnyVersionSettings> {
    /// Updates the MEGA file version that will be used when writing.
    ///
    /// If you set to V1 or V2 when encryption was set to `Some`, the encryption will be ignored.
    pub fn set_version(&mut self, version: MegVersion) {
        self.version_settings.set_version(version);
    }
}

impl<F, V> MegBuilder<F, V>
where
    V: WriteEncrypted,
{
    /// Gets the encryption for the MEGA file builder.
    pub fn encryption(&self) -> Option<&Key> {
        self.version_settings.encryption()
    }

    /// Sets the encryption for the MEGA file builder.
    ///
    /// If set to `None`, the file will not use encryption. If set to `Some`, the file will use
    /// encryption.
    ///
    /// This only applies when the version is V3. When the builder is created from [`MegVersion`]
    /// rather than a specific version struct, using `set_encryption` when the version is V1 or V2
    /// has no effect.
    pub fn set_encryption(&mut self, encryption: Option<Key>) {
        self.version_settings.set_encryption(encryption);
    }
}

impl<F: FileContent, V: WriteVersion> MegBuilder<F, V> {
    /// Build the MEGA file, writing the output to Dest.
    ///
    /// If successful, returns the total number of bytes written.
    pub fn build<W: Write>(self, writer: &mut W) -> Result<u64, BuildMegError> {
        let names_start = self.version_settings.header_size();
        let names_len = self.name_records_len()?;
        let names_end = names_start
            .checked_add(names_len)
            .ok_or(BuildMegError::OffsetTooLarge)?;
        let files_len = self.file_records_len()?;
        let files_end = names_end
            .checked_add(files_len)
            .ok_or(BuildMegError::OffsetTooLarge)?;

        #[derive(Copy, Clone)]
        struct DataOffset {
            start: u32,
            /// Size of the file contents.
            size: u32,
            /// Size after accounting for encryption rounding.
            step: u32,
        }

        let mut data_offsets = Vec::with_capacity(self.files.len());
        let mut data_offset = Ok(files_end);
        for file in self.files.values() {
            let start = data_offset?;

            let size = file.file_len()?;
            if size > u32::MAX as u64 {
                return Err(BuildMegError::FileTooLarge { size });
            }
            let size = size as u32;

            let step = self
                .version_settings
                .adjust_len(size)
                .ok_or(BuildMegError::FileTooLarge { size: size as u64 })?;

            // Delay checking this error so we allow a bigger file on the very last iteration.
            //
            // Other readers might choke on that but ours should work.
            data_offset = start.checked_add(step).ok_or(BuildMegError::OffsetTooLarge);
            data_offsets.push(DataOffset { start, size, step });
        }

        // We've now calculated all the data we need to write out the file.
        let mut writer = CountingWriter::new(writer);

        self.version_settings.write_header(
            &mut writer,
            self.files.len() as u32,
            files_end,
            names_len,
        )?;
        debug_assert!(
            writer.total_written() == names_start as u64,
            "miscalculated header length"
        );
        self.version_settings
            .write_names(&mut writer, self.files.keys().map(CrcPathBuf::as_path))?;
        debug_assert!(
            writer.total_written() == names_end as u64,
            "miscalculated names length"
        );
        for (idx, name) in self.files.keys().enumerate() {
            let crc = crc32fast::hash(name.as_path().as_bytes());
            let DataOffset { start, size, .. } = data_offsets[idx];
            self.version_settings
                .write_file_record(&mut writer, crc, idx as u32, start, size)?;
        }
        debug_assert!(
            writer.total_written() == files_end as u64,
            "miscalculated file records length"
        );
        for (idx, mut file) in self.files.into_values().enumerate() {
            let DataOffset { start, size, step } = data_offsets[idx];
            debug_assert!(
                writer.total_written() == start as u64,
                "miscalculated file offset for index {idx}, calculated {start}, actual {}",
                writer.total_written()
            );
            file.ensure_at_start()?;
            writer.move_mark();
            let written_size = self.version_settings.write_file(&mut writer, file)?;
            if written_size != size as u64 {
                return Err(BuildMegError::InaccurateFileSize {
                    reported_size: size,
                    actual_size: written_size,
                });
            }
            debug_assert!(
                writer.written_since_mark() == step as u64,
                "miscalculated file size accounting for encryption: calculated {step}, wrote {}",
                writer.written_since_mark()
            );
        }

        Ok(writer.total_written())
    }

    /// Get the total length of all the name records combined.
    fn name_records_len(&self) -> Result<u32, BuildMegError> {
        let mut names_len = 0u32;
        for name in self.files.keys() {
            let record_len = name_record_len(name.as_path(), self.name_length_limit)?;
            names_len = names_len
                .checked_add(record_len)
                .ok_or(BuildMegError::OffsetTooLarge)?;
        }
        self.version_settings
            .adjust_len(names_len)
            .ok_or(BuildMegError::OffsetTooLarge)
    }

    /// Get the total length of all the file records combined.
    fn file_records_len(&self) -> Result<u32, BuildMegError> {
        let num_files: u32 =
            self.files
                .len()
                .try_into()
                .map_err(|_| BuildMegError::TooManyFiles {
                    files: self.files.len(),
                    limit: self.version_settings.file_limit(),
                })?;
        if num_files > self.version_settings.file_limit() {
            return Err(BuildMegError::TooManyFiles {
                files: self.files.len(),
                limit: self.version_settings.file_limit(),
            });
        }

        num_files
            .checked_mul(self.version_settings.file_record_size())
            .ok_or(BuildMegError::OffsetTooLarge)
    }
}

/// Trait for types which can provide the contents for a file entry within a MEGA file.
///
/// MEGA files store offsets relative to the start of the MEGA file for the positions of the various
/// files' inner content. In order to calculate those offsets, we need more than just a `Read`, we
/// also need to know the length of the file ahead of time, which is what this trait provides with
/// the [`file_len`][Self::file_len] method.
///
/// Pre-calculating the file offsets allows us to avoid needing a [`Seek`]-able
/// [`Write`r][std::io::Write] when calling [`build`][MegBuilder::build] and allows us to avoid
/// seeking backwards to try to write the file headers retroactively.
///
/// The [`file_len`][Self::file_len] must accurately reflect how many bytes would be written if
/// copying the complete contents of this [`FileContent`], e.g. using [`std::io::copy`]. Note that
/// for [`File`] and [`Cursor`], the implementation of [`FileContent`] will rewind the stream.
pub trait FileContent: Read {
    /// Gets the length of the file's contents. This must accurately reflect the number of bytes
    /// which will be added to the MEGA file for this entry. If the number of bytes written when
    /// copying this content to the output does not match the amount reported, an error will be
    /// returned from [`build`][MegBuilder::build].
    fn file_len(&self) -> io::Result<u64>;

    /// Ensure that the FileContent is at the correct start position in order to copy the number of
    /// bytes specified by file_len.
    fn ensure_at_start(&mut self) -> io::Result<()>;
}

impl FileContent for &[u8] {
    fn file_len(&self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }

    fn ensure_at_start(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl FileContent for File {
    fn file_len(&self) -> io::Result<u64> {
        self.metadata().map(|meta| meta.len())
    }

    fn ensure_at_start(&mut self) -> io::Result<()> {
        self.rewind()
    }
}

impl<T> FileContent for Cursor<T>
where
    T: AsRef<[u8]>,
{
    fn file_len(&self) -> io::Result<u64> {
        Ok(self.get_ref().as_ref().len() as u64)
    }

    fn ensure_at_start(&mut self) -> io::Result<()> {
        self.rewind()
    }
}

impl<T> FileContent for BufReader<T>
where
    T: FileContent,
{
    fn file_len(&self) -> io::Result<u64> {
        self.get_ref().file_len()
    }

    fn ensure_at_start(&mut self) -> io::Result<()> {
        // Flush the buffer by consuming all bytes. Neither BufRead nor BufReader provides a public
        // 'discard buffer' method.
        self.consume(self.buffer().len());
        self.get_mut().ensure_at_start()
    }
}

impl<T> FileContent for Box<T>
where
    T: FileContent,
{
    fn file_len(&self) -> io::Result<u64> {
        T::file_len(self)
    }

    fn ensure_at_start(&mut self) -> io::Result<()> {
        T::ensure_at_start(self)
    }
}

/// Trait for selecting MEGA file write versions.
#[allow(private_bounds)]
pub trait BuildMeg: Sized + Sealed {
    /// Type that controls version-specific settings for the MEGA file builder.
    type Settings: WriteVersion;

    /// Creates a new MEGA file builder for this Meg version.
    fn builder<F>(self) -> MegBuilder<F, Self::Settings>;
}

trait Sealed {}

impl Sealed for crate::version::MegVersion {}
impl Sealed for crate::version::MegV1 {}
impl Sealed for crate::version::MegV2 {}
impl Sealed for crate::version::MegV3 {}

/// Trait for implementations of
pub trait WriteVersion {
    /// Get the limit on the number of files.
    fn file_limit(&self) -> u32 {
        u32::MAX
    }

    /// Return the size of the MEGA file header for this version.
    fn header_size(&self) -> u32;

    /// Gets the size of a File Record for this write version.
    fn file_record_size(&self) -> u32 {
        // V1 and V2 file records are always 5 u32 words.
        5 * size_of::<u32>() as u32
    }

    /// Adjust the length of the names table or a file's content to account for padding for
    /// encryption. Return None on overflow.
    fn adjust_len(&self, len: u32) -> Option<u32> {
        // V1 and V2 have no encryption so have no adjustments to length.
        Some(len)
    }

    /// Write the header to the file.
    fn write_header<W: Write>(
        &self,
        writer: &mut W,
        num_files: u32,
        data_offset: u32,
        names_len: u32,
    ) -> io::Result<()>;

    /// Write the names to the file.
    fn write_names<'p, W, I>(&self, writer: &mut W, names: I) -> io::Result<()>
    where
        W: Write,
        I: IntoIterator<Item = &'p MegPath>,
    {
        // V1 and V2 just write the names directly.
        write_names(writer, names)
    }

    /// Write a single file record.
    fn write_file_record<W: Write>(
        &self,
        writer: &mut W,
        crc: u32,
        index: u32,
        start: u32,
        size: u32,
    ) -> io::Result<()> {
        write_v1v2_file_record(writer, crc, index, start, size)
    }

    /// Write a single file.
    ///
    /// Return the actual number of bytes that came from `file`.
    fn write_file<W: Write, R: Read>(&self, writer: &mut W, mut file: R) -> io::Result<u64> {
        // V1 and V2 just copy the file exactly as is.
        io::copy(&mut file, writer)
    }
}

/// Trait for MEGA file version settings that allow configuring an encryption key.
pub trait WriteEncrypted {
    /// Get the encryption key set for the builder, if one is used.
    fn encryption(&self) -> Option<&Key>;

    /// Set the encryption key to use, or None to disable encryption.
    fn set_encryption(&mut self, encryption: Option<Key>);
}

/// Write out the name records.
fn write_names<'p, W: Write, I: IntoIterator<Item = &'p MegPath>>(
    writer: &mut W,
    names: I,
) -> io::Result<()> {
    for name in names {
        writer.write_u16::<LE>(name.len() as u16)?;
        writer.write_all(name.as_bytes())?;
    }

    Ok(())
}

/// Write a V1 or V2 file record.
fn write_v1v2_file_record<W: Write>(
    writer: &mut W,
    crc: u32,
    index: u32,
    start: u32,
    size: u32,
) -> io::Result<()> {
    writer.write_u32::<LE>(crc)?;
    writer.write_u32::<LE>(index)?;
    // In the record format, size actually comes before start.
    writer.write_u32::<LE>(size)?;
    writer.write_u32::<LE>(start)?;
    // Write the index again as the name value.
    writer.write_u32::<LE>(index)?;
    Ok(())
}

/// Implements ordering based on the CRC-32 of the path. Enforces normalization.
///
/// Since we enforce normalization, PartialEq can just be derived, as equal values will yield equal
/// CRC32s.
#[derive(PartialEq, Eq, Debug, Clone)]
#[repr(transparent)]
struct CrcPathBuf(MegPathBuf);

impl CrcPathBuf {
    /// Gets the normalized path.
    fn as_path(&self) -> &MegPath {
        &self.0
    }
}

impl From<MegPathBuf> for CrcPathBuf {
    fn from(mut value: MegPathBuf) -> Self {
        value.make_normalized();
        Self(value)
    }
}

impl PartialOrd for CrcPathBuf {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CrcPathBuf {
    fn cmp(&self, other: &Self) -> Ordering {
        crc32fast::hash(self.0.as_bytes())
            .cmp(&crc32fast::hash(other.0.as_bytes()))
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl Borrow<CrcPath> for CrcPathBuf {
    fn borrow(&self) -> &CrcPath {
        self.0.as_path().into()
    }
}

/// Wraps a MegPath to make it implement PartialEq and Ord with CrcPathBuf as if it was normalized.
///
/// Like CrcPathBuf, we can derive PartialEq and Eq.
#[derive(PartialEq, Eq, Debug)]
#[repr(transparent)]
struct CrcPath(MegPath);

impl From<&MegPath> for &CrcPath {
    fn from(value: &MegPath) -> Self {
        // SAFETY: Reprs are identical.
        unsafe { std::mem::transmute(value) }
    }
}

impl PartialOrd for CrcPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CrcPath {
    fn cmp(&self, other: &Self) -> Ordering {
        crc32_normalized(self.0.as_bytes())
            .cmp(&&crc32_normalized(other.0.as_bytes()))
            .then_with(|| self.0.cmp(&other.0))
    }
}

fn crc32_normalized(bytes: &[u8]) -> u32 {
    let mut state = crc32fast::Hasher::new();
    hash_normalized(bytes, &mut state);
    state.finalize()
}

/// Gets the number of bytes needed for the given name's name record. Errors if the name exceeds the
/// length limit.
fn name_record_len(name: &MegPath, limit: u16) -> Result<u32, BuildMegError> {
    if name.len() > limit as usize {
        Err(BuildMegError::NameTooLong {
            len: name.len(),
            limit,
        })
    } else {
        // Add the 2 bytes needed for the length field of the name record.
        let record_len = size_of::<u16>() + name.len();
        Ok(record_len as u32)
    }
}
