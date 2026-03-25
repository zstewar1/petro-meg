use std::io;

use tracing::warn;

use crate::crypto::Key;
use crate::version::{MegV1, MegV2, MegVersion};
use crate::writer::version3::V3Settings;
use crate::writer::{BuildMeg, MegBuilder, WriteEncrypted, WriteVersion};

impl BuildMeg for MegVersion {
    type Settings = AnyVersionSettings;

    fn builder<F>(self) -> MegBuilder<F, Self::Settings> {
        MegBuilder::new(AnyVersionSettings {
            version: self,
            v3settngs: Default::default(),
        })
    }
}

/// Stores version-specific writer settings for any MEGA file version.
///
/// You should generally not need to interact with this type directly. When you create a
/// [`MegBuilder`] with [`MegVersion::builder`], it will have the type `MegBuilder<F,
/// AnyVersionSettings>`. All relevant functionality related to the version-specific settings is
/// exposed as inherent methods on `MegBuilder`.
///
/// `AnyVersionSettings`, enables the [`MegBuilder::set_version`] and [`MegBuilder::set_encryption`]
/// methods.
pub struct AnyVersionSettings {
    version: MegVersion,
    v3settngs: V3Settings,
}

impl AnyVersionSettings {
    pub(super) fn version(&self) -> MegVersion {
        self.version
    }

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
        if self.version != MegVersion::V3 && self.v3settngs.encryption().is_some() {
            warn!("Encryption key was set, but version was not V3, so encryption is being ignored");
        }
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

impl WriteEncrypted for AnyVersionSettings {
    fn encryption(&self) -> Option<&Key> {
        self.v3settngs.encryption()
    }

    fn set_encryption(&mut self, encryption: Option<Key>) {
        self.v3settngs.set_encryption(encryption);
    }
}
