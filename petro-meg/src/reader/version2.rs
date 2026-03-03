use std::io::Read;

use byteorder::{LE, ReadBytesExt as _};
use tracing::warn;

use crate::reader::{
    FileEntry, ID2, MegReadError, MegReadOptions, ReadMegMeta, ReaderState, read_meg_meta,
    read_unencrypted_file_record,
};
use crate::version::MegV2;

impl ReadMegMeta for MegV2 {
    fn read_meg_meta_opt<R: Read>(
        self,
        mut reader: R,
        options: &MegReadOptions,
    ) -> Result<Vec<FileEntry>, MegReadError> {
        let state = ReadStateV2::read_header(&mut reader, options)?;
        read_meg_meta(state, reader, options)
    }
}

/// Internal state of V2 MEGA file reads.
pub(crate) struct ReadStateV2 {
    /// Start offset of file data.
    pub(super) data_start: u32,
    /// Number of file names in the filenames table.
    pub(super) num_filenames: u32,
    /// Number of file entries. Should be the same as num_filenames.
    pub(super) num_files: u32,
}

impl ReadStateV2 {
    pub(crate) fn read_header<R: Read>(
        reader: &mut R,
        _: &MegReadOptions,
    ) -> Result<Self, MegReadError> {
        let id1 = reader.read_u32::<LE>()?;
        let id2 = reader.read_u32::<LE>()?;
        if id1 != u32::MAX || id2 != ID2 {
            return Err(MegReadError::InvalidFileId { id1: id1, id2: id2 });
        }
        Ok(Self {
            data_start: reader.read_u32::<LE>()?,
            num_filenames: reader.read_u32::<LE>()?,
            num_files: reader.read_u32::<LE>()?,
        })
    }
}

impl ReaderState for ReadStateV2 {
    fn num_filenames(&self) -> u32 {
        self.num_filenames
    }

    fn num_files(&self) -> u32 {
        self.num_files
    }

    fn read_file_record<R: Read>(
        &self,
        reader: &mut R,
        options: &MegReadOptions,
        file_index: u32,
    ) -> Result<super::FileRecord, MegReadError> {
        let record = read_unencrypted_file_record(reader)?;
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
