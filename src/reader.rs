use std::collections::BTreeMap;
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};

use byteorder::{LE, ReadBytesExt as _};
use tracing::{instrument, warn};

/// Reads contents from a Mega file.
pub struct MegaFileReader<R> {
    /// File listing for the file, pulled from the file names and files table.
    files: BTreeMap<PathBuf, Vec<File>>,
    /// Source reader to pull file contents from.
    source: R,
}

impl<R> MegaFileReader<R>
where
    R: Read,
{
    /// Parse a V1 MegaFile.
    #[instrument(skip(source))]
    pub fn parse_v1(mut source: R) -> io::Result<Self> {
        // V1 Header, per petrolution.net:
        // Header:
        // +0000h  numFilenames  uint32   ; Number of filenames in the Filename Table
        // +0004h  numFiles      uint32   ; Number of files in the File Table
        let num_filenames = source.read_u32::<LE>()? as usize;
        let num_files = source.read_u32::<LE>()? as usize;
        let names = parse_filenames(&mut source, num_filenames)?;
        let files = parse_files_v1_v2(&mut source, names, num_files)?;
        Ok(Self { files, source })
    }
}

impl<R> MegaFileReader<R> {
    /// Get an iterator over file names contained in this MegaFile.
    ///
    /// Note that the mega file could contain file names that don't contain any actual file entries.
    pub fn file_names(
        &self,
    ) -> impl Iterator<Item = &Path> + DoubleEndedIterator + ExactSizeIterator {
        self.files.keys().map(|pb| pb.as_ref())
    }

    /// Get an iterator over file names contained in this MegaFile and the entries associated with
    /// each name.
    ///
    /// Note that the mega file could contain file names that don't contain any actual file entries.
    pub fn files(
        &self,
    ) -> impl Iterator<Item = (&Path, &[File])> + DoubleEndedIterator + ExactSizeIterator {
        self.files
            .iter()
            .map(|(pb, files)| (pb.as_ref(), files.as_ref()))
    }

    /// Gets the files associated with the given path, if any. Otherwise returns an empty slice.
    pub fn get_files(&self, path: &Path) -> &[File] {
        self.files.get(path).map_or(&[], AsRef::as_ref)
    }

    /// Get a reader which reads the file with the given index at the specified path.
    pub fn get_reader(&mut self, path: &Path, entry_idx: usize) -> Option<FileReader<&mut R>> {
        let entries = self.files.get(path)?;
        let entry = entries.get(entry_idx)?;
        Some(FileReader {
            seek_pos: entry.start,
            has_seeked: false,
            remaining_bytes: entry.size,
            source: &mut self.source,
        })
    }
}

/// Entry for a file read from the mega file files table.
#[derive(Clone)]
pub struct File {
    /// Size of the file.
    size: u32,
    /// Index of the start of the file.
    start: u32,
}

impl File {
    /// Gets the size of this file.
    pub fn size(&self) -> u32 {
        self.size
    }
}

/// Reader for a single file entry.
pub struct FileReader<R> {
    /// Position to seek to before reading.
    seek_pos: u32,
    /// Whether the initial seek to get the the right position in the mega file has been performed
    /// yet.
    has_seeked: bool,
    /// Number of bytes we expect to read from the file.
    remaining_bytes: u32,
    /// Source to read file contents from.
    source: R,
}

impl<'a, R> Read for FileReader<R>
where
    R: Read + Seek,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.has_seeked {
            let new_pos = self
                .source
                .seek(io::SeekFrom::Start(self.seek_pos as u64))?;
            if new_pos != self.seek_pos as u64 {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "Needed to seek to {} but seek ended at {new_pos}",
                        self.seek_pos
                    ),
                ));
            }
            self.has_seeked = true;
        }
        let mut reader = self.source.by_ref().take(self.remaining_bytes as u64);
        let res = reader.read(buf);
        // This should always succeed unless the Take has somehow expanded the remaining bytes.
        self.remaining_bytes = reader.limit().try_into().unwrap();
        match res {
            Ok(0) if self.remaining_bytes > 0 => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Reader ran out of bytes before we read the full expected file size",
            )),
            res => res,
        }
    }
}

/// Parse the filenames table.
///
/// Caller is responsible for ensuring that the reader is at the start of the filenames table.
///
/// This operation is the same across Mega file versions. For v3, the filenames might need to be
/// decrypted first.
fn parse_filenames(source: &mut impl Read, num_filenames: usize) -> io::Result<Vec<PathBuf>> {
    // File name table records follow this format, per petrolution.net:
    //   +0000h  length        uint16   ; Length of the filename, in characters
    //   +0004h  name          length   ; The ASCII filename
    let mut result = Vec::with_capacity(num_filenames);
    for _ in 0..num_filenames {
        let length = source.read_u16::<LE>()? as usize;
        let mut buf = vec![0u8; length];
        source.read_exact(&mut buf[..])?;
        let name = match String::from_utf8(buf) {
            Ok(str) if str.is_ascii() => str,
            Ok(str) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("File name must be ASCII, got {str}"),
                ));
            }
            Err(e) => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, e));
            }
        };
        result.push(PathBuf::from(name));
    }
    Ok(result)
}

/// Implements file entry parsing for V1 and V2 files. V3 files use a different format for file
/// entries.
///
/// Caller is responsible for ensuring that the reader is at the start of the files table.
fn parse_files_v1_v2(
    source: &mut impl Read,
    names: Vec<PathBuf>,
    num_files: usize,
) -> io::Result<BTreeMap<PathBuf, Vec<File>>> {
    // We start by building a parallel vector to the names vector, then zip them into a map later.
    // This lets us handle multiple entries mapping to the same name without cloning the names. Is
    // this actually better? Maybe?
    let mut entry_list = vec![Vec::with_capacity(1); names.len()];

    // Per petrolution.net, V1 and V2 files tables have this format:
    //   +0000h  crc           uint32   ; CRC-32 of the filename
    //   +0004h  index         uint32   ; Index of this record in the table
    //   +0008h  size          uint32   ; Size of the file, in bytes
    //   +000Ch  start         uint32   ; Start of the file, in bytes , from the start of the Mega File
    //   +0010h  name          uint32   ; Index in the Filename Table of the filename
    for idx in 0..num_files {
        let expected_crc = source.read_u32::<LE>()?;
        let expected_idx = source.read_u32::<LE>()? as usize;
        let size = source.read_u32::<LE>()?;
        let start = source.read_u32::<LE>()?;
        let name_idx = source.read_u32::<LE>()? as usize;

        if expected_idx != idx {
            warn!("File entry at index {idx} expects to be at index {expected_idx}");
        }

        let name = match names.get(name_idx) {
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "File entry at index {idx} points to name index {name_idx} which is out of \
                        bounds (only {} names available)",
                        names.len()
                    ),
                ));
            }
            Some(name) => name,
        };

        let crc = crc32fast::hash(name.as_os_str().as_encoded_bytes());
        if crc != expected_crc {
            warn!(
                "File entry at index {idx} with name {name:?} expected CRC {expected_crc:08X} but \
                the actual CRC was {crc:08X}"
            );
        }

        entry_list[name_idx].push(File { size, start });
    }

    Ok(names.into_iter().zip(entry_list.into_iter()).collect())
}
