use std::io;

use crate::version::{MegV1, MegV2, MegVersion};
use crate::writer::version3::BuildV3Settings;
use crate::writer::{BuildMeg, MegBuilder, WriteVersion};

impl BuildMeg for MegVersion {
    type Settings = AnyVersionSettings;

    fn builder<F>(self) -> MegBuilder<F, Self::Settings> {
        MegBuilder::new(AnyVersionSettings {
            version: self,
            v3settngs: Default::default(),
        })
    }
}

/// WriteVersion that allows writing any MEGA file version.
pub struct AnyVersionSettings {
    version: MegVersion,
    v3settngs: BuildV3Settings,
}

impl AnyVersionSettings {
    pub(super) fn set_version(&mut self, version: MegVersion) {
        self.version = version;
    }
}

macro_rules! delegate {
    ($self:ident, $method:ident $(, $param:ident)*) => {
        match $self.version {
            MegVersion::V1 => MegV1.$method($($param),*),
            MegVersion::V2 => MegV2.$method($($param),*),
            MegVersion::V3 => $self.v3settngs.$method($($param),*),
        }
    }
}

impl WriteVersion for AnyVersionSettings {
    fn file_limit(&self) -> u32 {
        delegate!(self, file_limit)
    }

    fn header_size(&self) -> u32 {
        delegate!(self, header_size)
    }

    fn adjust_len(&self, len: u32) -> Option<u32> {
        delegate!(self, adjust_len, len)
    }

    fn file_record_size(&self) -> u32 {
        delegate!(self, file_record_size)
    }

    fn write_header<W: io::Write>(
        &self,
        writer: &mut W,
        num_files: u32,
        data_offset: u32,
        names_len: u32,
    ) -> io::Result<()> {
        delegate!(
            self,
            write_header,
            writer,
            num_files,
            data_offset,
            names_len
        )
    }

    fn write_names<'p, W, I>(&self, writer: &mut W, names: I) -> io::Result<()>
    where
        W: io::Write,
        I: IntoIterator<Item = &'p crate::path::MegPath>,
    {
        delegate!(self, write_names, writer, names)
    }

    fn write_file_record<W: io::Write>(
        &self,
        writer: &mut W,
        crc: u32,
        index: u32,
        start: u32,
        size: u32,
    ) -> io::Result<()> {
        delegate!(self, write_file_record, writer, crc, index, start, size)
    }

    fn write_file<W: io::Write, R: io::Read>(&self, writer: &mut W, file: R) -> io::Result<u64> {
        delegate!(self, write_file, writer, file)
    }
}
