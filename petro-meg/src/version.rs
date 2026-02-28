use std::fmt;
use std::str::FromStr;

use byteorder::{ByteOrder as _, LE};
use thiserror::Error;

use crate::header::{HeaderV1, ID2};
use crate::path::{WIN_PATH_LIMIT, is_dir_separator, is_valid_component};

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

/// Error returned when the MEGA file version isn't recognized.
#[derive(Error, Debug)]
#[error("Unrecognized MEGA file version")]
pub struct InvalidMegVersion;

/// Error returned when we fail to guess the MEGA file version.
#[derive(Error, Debug)]
pub enum VersionGuessFailure {
    #[error("File contents were not long enough to determine the MegVersion")]
    TooShort,
    #[error("File ID wasn't recognized: id1: 0x{id1:08X}, id2: 0x{id2:08X}")]
    UnrecognizedId { id1: u32, id2: u32 },
}

/// Guess the file version from the file contents.
pub fn guess_version(file: &[u8]) -> Result<MegVersion, VersionGuessFailure> {
    let (v1header, _) = HeaderV1::split_off(file).ok_or(VersionGuessFailure::TooShort)?;
    if v1header.num_filenames == v1header.num_files {
        // If the V1 filename count matches the file count, assume V1.
        return Ok(MegVersion::V1);
    }
    let id1 = v1header.num_filenames;
    let id2 = v1header.num_files;
    if (id1 != u32::MAX && id1 != 0x8FFFFFFF) || id2 != ID2 {
        return Err(VersionGuessFailure::UnrecognizedId { id1, id2 });
    }
    // Only V3 uses the 8FFFFFF flag for "encrypted".
    if id1 == 0x8FFFFFFF {
        return Ok(MegVersion::V3);
    }
    // Now we have to guess between V2 and V3 based on whether the bytes at 0x14-0x18 look more like
    // a number (the V3 filenamesSize field) or a u16 length + 2 bytes of printable text.
    let field_of_interest = file.get(20..24).ok_or(VersionGuessFailure::TooShort)?;
    let name_len = LE::read_u16(&field_of_interest[0..2]);
    if name_len as usize <= WIN_PATH_LIMIT && is_valid_path_chars(&field_of_interest[2..4]) {
        // If the name_len fits within the windows path length limit and the characters at 2..4 are
        // valid path components, then assume its a name and treat the file as version 2.
        Ok(MegVersion::V2)
    } else {
        Ok(MegVersion::V3)
    }
}

/// Return true if the characters are all valid within MEGA file paths.
fn is_valid_path_chars(chars: &[u8]) -> bool {
    chars
        .iter()
        .all(|&ch| is_dir_separator(ch) || is_valid_component(ch))
}
