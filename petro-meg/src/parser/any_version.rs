use std::io::{self, Read};

use byteorder::{ByteOrder as _, LE, ReadBytesExt as _};

use crate::parser::version1::ReadStateV1;
use crate::parser::version2::ReadStateV2;
use crate::parser::version3::ReadStateV3;
use crate::parser::{FileEntry, ID2, MegReadError, MegReadOptions, ReadMegMeta, read_meg_meta};
use crate::path::{WIN_PATH_LIMIT, is_valid_path_chars};
use crate::version::{GuessVersion, MegVersion};

impl ReadMegMeta for GuessVersion {
    fn read_meg_meta_opt<R: Read>(
        self,
        mut reader: R,
        options: &MegReadOptions,
    ) -> Result<Vec<FileEntry>, MegReadError> {
        let id1 = reader.read_u32::<LE>()?;
        let id2 = reader.read_u32::<LE>()?;

        // If the numbers match, guess that this is V1.
        if id1 == id2 {
            let state = ReadStateV1 {
                num_filenames: id1,
                num_files: id2,
            };
            return read_meg_meta(state, reader, options);
        }
        if (id1 != u32::MAX && id1 != 0x8FFF_FFFF) || id2 != ID2 {
            return Err(MegReadError::InvalidFileId { id1, id2 });
        }
        let data_start = reader.read_u32::<LE>()?;
        let num_filenames = reader.read_u32::<LE>()?;
        let num_files = reader.read_u32::<LE>()?;
        // Only Version 3 uses 0x8FFF_FFFF for encrypted, so if we have that, guess V3.
        if id1 == 0x8FFF_FFFF {
            if options.key.is_none() {
                return Err(MegReadError::MissingKey);
            }
            let filenames_size = reader.read_u32::<LE>()?;
            let state = ReadStateV3 {
                encrypted: true,
                data_start,
                num_filenames,
                num_files,
                filenames_size,
            };
            return read_meg_meta(state, reader, options);
        }
        // In this unfortunate case, we will need to read ahead and possibly rewind, because we have
        // to guess whether the next pair of values looks more like a path or a filenames table
        // size.
        let mut bytes = [0u8; 4];
        reader.read_exact(&mut bytes)?;

        let name_len = LE::read_u16(&bytes[..2]);
        if name_len as usize > WIN_PATH_LIMIT || !is_valid_path_chars(&bytes[2..]) {
            // If the name length exceeds the windows path limit or the first two characters of the
            // name aren't valid as path characters, assume this is a V3 file and we actually have
            // the filenames_size.
            let filenames_size = LE::read_u32(&bytes);
            let state = ReadStateV3 {
                encrypted: false,
                data_start,
                num_filenames,
                num_files,
                filenames_size,
            };
            return read_meg_meta(state, reader, options);
        }
        // If it looks like a V2 name instead, we'll have to rewind to put the reader back. But we
        // don't have a guarantee of "seekability" here, so instead we'll chain a cursor over the
        // bytes we just read to reinsert them back at the front of the reader.
        let state = ReadStateV2 {
            data_start,
            num_filenames,
            num_files,
        };
        let reader = io::Cursor::new(bytes).chain(reader);
        read_meg_meta(state, reader, options)
    }
}

impl ReadMegMeta for MegVersion {
    fn read_meg_meta_opt<R: Read>(
        self,
        mut reader: R,
        options: &MegReadOptions,
    ) -> Result<Vec<FileEntry>, MegReadError> {
        match self {
            MegVersion::V1 => {
                let state = ReadStateV1::read_header(&mut reader, options)?;
                read_meg_meta(state, reader, options)
            }
            MegVersion::V2 => {
                let state = ReadStateV2::read_header(&mut reader, options)?;
                read_meg_meta(state, reader, options)
            }
            MegVersion::V3 => {
                let state = ReadStateV3::read_header(&mut reader, options)?;
                read_meg_meta(state, reader, options)
            }
        }
    }
}
