use std::io::Read;

use byteorder::{LE, ReadBytesExt as _};

use crate::reader::{
    FileEntry, MegReadError, MegReadOptions, ReadMegMeta, ReaderState,
    read_v1v2_file_record, read_meg_meta,
};
use crate::version::MegV1;

impl ReadMegMeta for MegV1 {
    fn read_meg_meta_opt<R: Read>(
        self,
        mut reader: R,
        options: &MegReadOptions,
    ) -> Result<Vec<FileEntry>, MegReadError> {
        let state = ReadStateV1::read_header(&mut reader, options)?;
        read_meg_meta(state, reader, options)
    }
}

/// Internal state of V1 MEGA file reads.
#[derive(Debug)]
pub(crate) struct ReadStateV1 {
    /// Number of file names in the filenames table.
    pub(super) num_filenames: u32,
    /// Number of file entries. Should be the same as num_filenames.
    pub(super) num_files: u32,
}

impl ReadStateV1 {
    pub(crate) fn read_header<R: Read>(
        reader: &mut R,
        _: &MegReadOptions,
    ) -> Result<Self, MegReadError> {
        Ok(Self {
            num_filenames: reader.read_u32::<LE>()?,
            num_files: reader.read_u32::<LE>()?,
        })
    }
}

impl ReaderState for ReadStateV1 {
    fn num_filenames(&self) -> u32 {
        self.num_filenames
    }

    fn num_files(&self) -> u32 {
        self.num_files
    }

    fn read_file_record<R: Read>(
        &self,
        reader: &mut R,
        _: &MegReadOptions,
        _index: u32,
    ) -> Result<super::FileRecord, MegReadError> {
        read_v1v2_file_record(reader)
    }
}
