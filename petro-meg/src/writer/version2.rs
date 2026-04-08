use std::io::{self, Write};

use byteorder::{LE, WriteBytesExt as _};

use crate::ID2;
use crate::version::MegV2;
use crate::writer::{BuildMeg, MegBuilder, WriteVersion};

impl BuildMeg for MegV2 {
    type Settings = Self;

    fn builder<F>(self) -> MegBuilder<F, Self::Settings> {
        MegBuilder::new(self)
    }
}

impl WriteVersion for MegV2 {
    fn header_size(&self) -> u32 {
        5 * size_of::<u32>() as u32
    }

    fn write_header<W: Write>(
        &self,
        writer: &mut W,
        num_files: u32,
        data_offset: u32,
        _names_len: u32,
    ) -> io::Result<()> {
        writer.write_u32::<LE>(u32::MAX)?;
        writer.write_u32::<LE>(ID2)?;
        writer.write_u32::<LE>(data_offset)?;
        writer.write_u32::<LE>(num_files)?;
        writer.write_u32::<LE>(num_files)?;
        Ok(())
    }
}
