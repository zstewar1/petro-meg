use std::io::{self, Read};

use byteorder::{LE, ReadBytesExt as _};
use tracing::{trace, warn};

use crate::crypto::DecryptingReader;
use crate::path::MegPathBuf;
use crate::reader::{
    FileEntry, FileRecord, ID2, MegReadError, MegReadOptions, ReadMegMeta, ReaderState,
    read_meg_meta, read_names,
};
use crate::version::MegV3;

impl ReadMegMeta for MegV3 {
    fn read_meg_meta_opt<R: Read>(
        self,
        mut reader: R,
        options: &MegReadOptions,
    ) -> Result<Vec<FileEntry>, MegReadError> {
        let state = ReadStateV3::read_header(&mut reader, options)?;
        read_meg_meta(state, reader, options)
    }
}

/// Internal state of V3 MEGA file reads.
#[derive(Debug)]
pub(crate) struct ReadStateV3 {
    /// Whether this reader is encrypted.
    pub(super) encrypted: bool,
    /// Start offset of file data.
    pub(super) data_start: u32,
    /// Number of file names in the filenames table.todo!()
    pub(super) num_filenames: u32,
    /// Number of file entries. Should be the same as num_filenames.
    pub(super) num_files: u32,
    /// Size of the filenames block, needed since they may be encrypted.
    pub(super) filenames_size: u32,
}

impl ReadStateV3 {
    pub(crate) fn read_header<R: Read>(
        reader: &mut R,
        options: &MegReadOptions,
    ) -> Result<Self, MegReadError> {
        let flags = reader.read_u32::<LE>()?;
        let id = reader.read_u32::<LE>()?;
        if (flags != u32::MAX && flags != 0x8FFF_FFFF) || id != ID2 {
            return Err(MegReadError::InvalidFileId {
                id1: flags,
                id2: id,
            });
        }
        let encrypted = flags == 0x8FFF_FFFF;
        if encrypted && options.key.is_none() {
            // There is no option to ignore missing AES keys because without a key we just cannot do
            // anything with the contents, compared to many of the other checks which can be fudged
            // reasonably safely.
            return Err(MegReadError::MissingKey);
        }
        Ok(Self {
            encrypted,
            data_start: reader.read_u32::<LE>()?,
            num_filenames: reader.read_u32::<LE>()?,
            num_files: reader.read_u32::<LE>()?,
            filenames_size: reader.read_u32::<LE>()?,
        })
    }
}

impl ReaderState for ReadStateV3 {
    fn num_filenames(&self) -> u32 {
        self.num_filenames
    }

    fn num_files(&self) -> u32 {
        self.num_files
    }

    fn read_names<R: Read>(
        &self,
        reader: &mut R,
        options: &MegReadOptions,
    ) -> Result<Vec<Option<MegPathBuf>>, MegReadError> {
        let mut reader = reader.take(self.filenames_size as u64);
        let (names, leftovers) = if self.encrypted {
            let Some(ref key) = options.key else {
                return Err(MegReadError::MissingKey);
            };
            let mut reader = DecryptingReader::new(reader, key);
            let names = read_names(&mut reader, self.num_filenames, options)?;
            // We don't care about any non-name bytes left in the decrypting reader's buffer.
            (names, reader.into_inner())
        } else {
            let names = read_names(&mut reader, self.num_filenames, options)?;
            (names, reader)
        };
        disacard_leftovers(leftovers)?;
        Ok(names)
    }

    fn read_file_record<R: Read>(
        &self,
        reader: &mut R,
        options: &MegReadOptions,
        file_index: u32,
    ) -> Result<FileRecord, MegReadError> {
        let flags = reader.read_u16::<LE>()?;
        let encrypted = match flags {
            0 => false,
            1 => true,
            _ => return Err(MegReadError::InvalidFileFlags { file_index, flags }),
        };

        if encrypted != self.encrypted {
            let err = MegReadError::MismatchedEncryption {
                file_index,
                meg_encrypted: self.encrypted,
                record_encrypted: encrypted,
            };
            if options.validate_consistent_encryption {
                return Err(err);
            }
            warn!("{err}");
        }

        let record = if encrypted {
            const ENC_RECORD_SIZE: u64 = 32;
            let reader = reader.take(ENC_RECORD_SIZE);
            let Some(ref key) = options.key else {
                return Err(MegReadError::MissingKey);
            };
            let mut reader = DecryptingReader::new(reader, key);
            let record = read_v3_file_record(&mut reader, encrypted)?;
            trace!("Read record at index {file_index}: {record:?}");
            disacard_leftovers(reader.into_inner())?;
            record
        } else {
            read_v3_file_record(reader, encrypted)?
        };

        if record.start < self.data_start {
            let err = MegReadError::FileBelowDataStart {
                file_index,
                file_start: record.start,
                data_start: self.data_start,
            };
            if options.validate_file_start_data_start {
                return Err(err);
            }
            warn!("{err}");
        }

        Ok(record)
    }
}

/// Reads a V3 file record. Note this does not perform decryption, it just reads the raw record. The
/// encrypted arg is just for filling in the encrypted field of the record.
///
/// Unlike V1 and V2, the name field is only a u16 presumably in order to keep the record the same
/// size when accounting for the added flags field.
fn read_v3_file_record<R: Read>(
    reader: &mut R,
    encrypted: bool,
) -> Result<FileRecord, MegReadError> {
    Ok(FileRecord {
        encrypted,
        crc: reader.read_u32::<LE>()?,
        index: reader.read_u32::<LE>()?,
        size: reader.read_u32::<LE>()?,
        start: reader.read_u32::<LE>()?,
        name: reader.read_u16::<LE>()? as u32,
    })
}

/// Discard the leftovers from a 'Take' operation to ensure the underlying reader is positioned in
/// the correct/expected place after.
fn disacard_leftovers<R: Read>(mut reader: io::Take<&mut R>) -> io::Result<()> {
    // If there are bytes left unread after reading the names, skip them.
    if reader.limit() > 0 {
        let expected_discard = reader.limit();
        let discarded = io::copy(&mut reader, &mut io::sink())?;
        if discarded < expected_discard {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Ran out of data while seeking through the lefovers of a prior segment",
            ));
        }
    }
    Ok(())
}
