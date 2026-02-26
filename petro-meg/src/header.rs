use byteorder::{ByteOrder as _, LE};

/// V1 MEGA File Header.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub(crate) struct HeaderV1 {
    // Format is based on documentation on petrolution.net:
    /// +0000h  numFilenames  uint32   ; Number of filenames in the Filename Table
    pub(crate) num_filenames: u32,
    /// +0004h  numFiles      uint32   ; Number of files in the File Table
    pub(crate) num_files: u32,
}

impl HeaderV1 {
    /// Read a header from the start of the given bytes.
    ///
    /// Panics if there aren't enough bytes.
    pub(crate) fn read_from(b: &[u8]) -> Self {
        assert!(b.len() >= size_of::<Self>());
        Self {
            num_filenames: LE::read_u32(&b[0..4]),
            num_files: LE::read_u32(&b[4..8]),
        }
    }

    /// If the given bytes are long enough to read a HeaderV1, return the header and Rest, otherwise
    /// return none.
    pub(crate) fn split_off(b: &[u8]) -> Option<(Self, &[u8])> {
        let (header, rest) = b.split_at_checked(size_of::<Self>())?;
        Some((Self::read_from(header), rest))
    }
}

/// Split a single name record off the front of the given slice, returning the path and the
/// remaining bytes.
pub(crate) fn split_off_name(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    // Format for each name record is based on documentation on petrolution.net:
    //   +0000h  length        uint16   ; Length of the filename, in characters
    //   +0004h  name          length   ; The ASCII filename
    let (name_len, bytes) = bytes.split_at_checked(size_of::<u16>())?;
    let name_len = LE::read_u16(name_len) as usize;
    bytes.split_at_checked(name_len)
}

/// V1 and V2 MEGA File file record.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub(crate) struct FileRecordV1V2 {
    // Format is based on documentation on petrolution.net:
    /// +0000h  crc           uint32   ; CRC-32 of the filename
    pub(crate) crc: u32,
    /// +0004h  index         uint32   ; Index of this record in the table
    pub(crate) index: u32,
    /// +0008h  size          uint32   ; Size of the file, in bytes
    pub(crate) size: u32,
    /// +000Ch  start         uint32   ; Start of the file, in bytes , from the start of the Mega File
    pub(crate) start: u32,
    /// +0010h  name          uint32   ; Index in the Filename Table of the filename
    pub(crate) name: u32,
}

impl FileRecordV1V2 {
    /// Slice the input bytes to a length to hold `num_files` FileRecord entries, without reading
    /// them yet.
    pub(crate) fn slice_n(b: &[u8], num_files: usize) -> Option<&[u8]> {
        let num_bytes = num_files * size_of::<FileRecordV1V2>();
        b.get(0..num_bytes)
    }

    /// Read a header from the start of the given bytes.
    ///
    /// Panics if there aren't enough bytes.
    pub(crate) fn read_from(b: &[u8]) -> Self {
        assert!(b.len() >= size_of::<Self>());
        Self {
            crc: LE::read_u32(&b[0..4]),
            index: LE::read_u32(&b[4..8]),
            size: LE::read_u32(&b[8..12]),
            start: LE::read_u32(&b[12..16]),
            name: LE::read_u32(&b[16..20]),
        }
    }

    /// If the given bytes are long enough to read a [`FileRecordV1V2`], return the record and
    /// remaining bytes, otherwise return none.
    pub(crate) fn split_off(b: &[u8]) -> Option<(Self, &[u8])> {
        let (record, rest) = b.split_at_checked(size_of::<Self>())?;
        Some((Self::read_from(record), rest))
    }
}
