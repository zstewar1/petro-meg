use std::io::{self, Read, Write};

use byteorder::{LE, WriteBytesExt as _};

use crate::crypto::{EncryptingWriter, Key, round_up_to_block};
use crate::reader::ID2;
use crate::version::MegV3;
use crate::writer::{BuildMeg, MegBuilder, WriteEncrypted, WriteVersion, write_names};

impl BuildMeg for MegV3 {
    type Settings = V3Settings;

    fn builder<F>(self) -> MegBuilder<F, Self::Settings> {
        MegBuilder::new(Default::default())
    }
}

/// Stores version-specific settings for V3 MEGA files.
///
/// You should generally not need to interact with this type directly. When you create a
/// [`MegBuilder`] with [`MegV3::builder`], it will have the type `MegBuilder<F, V3Settings>`. All
/// relevant functionality related to the version-specific settings is exposed as inherent methods
/// on `MegBuilder`.
///
/// `V3Settings`, enables the [`MegBuilder::set_encryption`] method.

#[derive(Default)]
pub struct V3Settings {
    encryption: Option<Key>,
}

impl WriteEncrypted for V3Settings {
    fn encryption(&self) -> Option<&Key> {
        self.encryption.as_ref()
    }

    fn set_encryption(&mut self, encryption: Option<Key>) {
        self.encryption = encryption;
    }
}

impl WriteVersion for V3Settings {
    fn file_limit(&self) -> u32 {
        // For V3 the file limit is lowered to u16 because the name index field has been shortened.
        u16::MAX as u32
    }

    fn header_size(&self) -> u32 {
        6 * size_of::<u32>() as u32
    }

    fn adjust_len(&self, len: u32) -> Option<u32> {
        if self.encryption.is_none() {
            Some(len)
        } else {
            round_up_to_block(len as u64).try_into().ok()
        }
    }

    fn file_record_size(&self) -> u32 {
        let flags_len = size_of::<u16>() as u32;
        let content_len = size_of::<u32>() as u32 * 4 + size_of::<u16>() as u32;
        if self.encryption.is_some() {
            flags_len + round_up_to_block(content_len as u64) as u32
        } else {
            flags_len + content_len
        }
    }

    fn write_header<W: Write>(
        &self,
        writer: &mut W,
        num_files: u32,
        data_offset: u32,
        names_len: u32,
    ) -> io::Result<()> {
        let flags = match self.encryption {
            Some(_) => 0x8FFF_FFFF,
            None => u32::MAX,
        };

        writer.write_u32::<LE>(flags)?;
        writer.write_u32::<LE>(ID2)?;
        writer.write_u32::<LE>(data_offset)?;
        // Num files and num names are always the same.
        writer.write_u32::<LE>(num_files)?;
        writer.write_u32::<LE>(num_files)?;
        // Use the calculated names section length.
        writer.write_u32::<LE>(names_len)?;
        Ok(())
    }

    fn write_names<'p, W, I>(&self, writer: &mut W, names: I) -> io::Result<()>
    where
        W: Write,
        I: IntoIterator<Item = &'p crate::path::MegPath>,
    {
        match self.encryption {
            Some(ref key) => {
                let mut writer = EncryptingWriter::new(writer, key);
                write_names(&mut writer, names)?;
                writer.pad();
                writer.flush()
            }
            None => write_names(writer, names),
        }
    }

    fn write_file_record<W: Write>(
        &self,
        writer: &mut W,
        crc: u32,
        index: u32,
        start: u32,
        size: u32,
    ) -> io::Result<()> {
        match self.encryption {
            Some(ref key) => {
                // Write the flags field directly.
                writer.write_u16::<LE>(1)?;
                let mut writer = EncryptingWriter::new(writer, key);
                write_v3_file_record(&mut writer, crc, index, start, size)?;
                writer.pad();
                writer.flush()
            }
            None => {
                // Write the flags field directly.
                writer.write_u16::<LE>(0)?;
                write_v3_file_record(writer, crc, index, start, size)
            }
        }
    }

    fn write_file<W: Write, R: Read>(&self, writer: &mut W, mut file: R) -> io::Result<u64> {
        match self.encryption {
            Some(ref key) => {
                let mut writer = EncryptingWriter::new(writer, key);
                let size = io::copy(&mut file, &mut writer)?;
                writer.pad();
                writer.flush()?;
                Ok(size)
            }
            None => io::copy(&mut file, writer),
        }
    }
}

fn write_v3_file_record<W: Write>(
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
    // Write the index again as the name value. This should already have been validated.
    writer.write_u16::<LE>(index as u16)?;
    Ok(())
}
