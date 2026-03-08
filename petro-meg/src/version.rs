//! Proives types for selecting different MEGA file versions.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// Identifies any MEGA file version.
///
/// Allows dynamically selecting a MEGA file version at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MegVersion {
    /// V1 MEGA file used for Empire at War/Forces of Corruption and Universe at War
    V1,
    /// V2 MEGA file used for Guardians of Graxia.
    V2,
    /// V3 MEGA file used for Rise of Immortals, Grey Goo, and Great War Western Front.
    V3,
}

impl fmt::Display for MegVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            MegVersion::V1 => "v1",
            MegVersion::V2 => "v2",
            MegVersion::V3 => "v3",
        };
        fmt::Display::fmt(s, f)
    }
}

impl From<MegV1> for MegVersion {
    fn from(_: MegV1) -> Self {
        MegVersion::V1
    }
}

impl From<MegV2> for MegVersion {
    fn from(_: MegV2) -> Self {
        MegVersion::V2
    }
}

impl From<MegV3> for MegVersion {
    fn from(_: MegV3) -> Self {
        MegVersion::V3
    }
}

impl FromStr for MegVersion {
    type Err = InvalidMegVersion;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.len() {
            1 => version_char_to_num(s.as_bytes()[0]),
            2 if s.is_ascii() && s.as_bytes()[0].eq_ignore_ascii_case(&b'v') => {
                version_char_to_num(s.as_bytes()[1])
            }
            _ => Err(InvalidMegVersion),
        }
    }
}

fn version_char_to_num(ch: u8) -> Result<MegVersion, InvalidMegVersion> {
    Ok(match ch {
        b'1' => MegVersion::V1,
        b'2' => MegVersion::V2,
        b'3' => MegVersion::V3,
        _ => return Err(InvalidMegVersion),
    })
}

/// Identifies MEGA file version 1.
///
/// Allows generically selecing a MEGA file version at compile time.
#[derive(Default, Debug)]
pub struct MegV1;

impl fmt::Display for MegV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("v1", f)
    }
}

/// Identifies MEGA file version 2.
///
/// Allows generically selecing a MEGA file version at compile time.
#[derive(Default, Debug)]
pub struct MegV2;

impl fmt::Display for MegV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("v2", f)
    }
}

/// Identifies MEGA file version 3.
///
/// Allows generically selecing a MEGA file version at compile time.
#[derive(Default, Debug)]
pub struct MegV3;

impl fmt::Display for MegV3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("v3", f)
    }
}

/// Acts like a MEGA file version, but only for reading, not writing.
///
/// Its implementation of [`ReadMegMeta`][crate::reader::ReadMegMeta] uses heuristics to try to
/// guess the MEGA file version from the headers.
#[derive(Default, Debug)]
pub struct GuessVersion;

/// Error returned when the MEGA file version isn't recognized.
#[derive(Error, Debug)]
#[error("Unrecognized MEGA file version")]
pub struct InvalidMegVersion;
