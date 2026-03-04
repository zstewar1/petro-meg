use std::io::{self, Write};

use byteorder::{LE, WriteBytesExt as _};

use crate::version::MegV1;
use crate::writer::{BuildMeg, MegBuilder, WriteVersion};

impl BuildMeg for MegV1 {
    type Settings = Self;

    fn builder<F>(self) -> MegBuilder<F, Self::Settings> {
        MegBuilder::new(self)
    }
}

impl WriteVersion for MegV1 {
    fn header_size(&self) -> u32 {
        2 * size_of::<u32>() as u32
    }

    fn write_header<W: Write>(
        &self,
        writer: &mut W,
        num_files: u32,
        _data_offset: u32,
        _names_len: u32,
    ) -> io::Result<()> {
        writer.write_u32::<LE>(num_files)?;
        writer.write_u32::<LE>(num_files)?;
        Ok(())
    }
}
