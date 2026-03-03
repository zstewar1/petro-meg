use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// Identifies the version of a MEGA file.
#[derive(Clone, Copy, Debug)]
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
        f.write_str(match self {
            MegVersion::V1 => "v1",
            MegVersion::V2 => "v2",
            MegVersion::V3 => "v3",
        })
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

/// Allows generically selecing to parse a V1 MEGA file.
#[derive(Default)]
pub struct MegV1;

/// Allows generically selecing to parse a V2 MEGA file.
#[derive(Default)]
pub struct MegV2;

/// Allows generically selecing to parse a V3 MEGA file.
#[derive(Default)]
pub struct MegV3;

/// A "version selector" which tells the parser to guess which parser version to use. Not usable for
/// encoding.
#[derive(Default)]
pub struct GuessVersion;

/// Error returned when the MEGA file version isn't recognized.
#[derive(Error, Debug)]
#[error("Unrecognized MEGA file version")]
pub struct InvalidMegVersion;
