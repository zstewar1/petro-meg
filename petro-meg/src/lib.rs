//! # Petroglyph Meg file library
//!
//! MEGA files are a format used by Petroglyph for various games including Star Wars: Empire at War,
//! Universe at War: Earth Assault, Guardians of Graxia, Rise of Immortals, Grey Goo, and Great War:
//! Western Front.
//!
//! This library provides tools for extracting their contents or authoring them. It is based on the
//! documentation an research by Mike Lankamp found on
//! [modtools.petrolution.net](https://modtools.petrolution.net/docs/MegFileFormat)
//!
//! There are 3 different MEGA file versions, and the version used depends on the game. Based on the
//! [Petrolution Mod Tools Game List](https://modtools.petrolution.net/docs/Games), the following
//! versions are used for each of these games:
//!
//! *   Version 1:
//!     *   Star Wars: Empire at War
//!     *   Star Wars: Empire at War: Forces of Corruption
//!     *   Universe at War: Earth Assault
//! *   Version 2:
//!     *   Guardians of Graxia
//! *   Version 3:
//!     *   Rise of Immortals
//!     *   Grey Goo
//!     *   Great War: Western Front
//!
//! The different versions are represented by types in the [`version`] module. There are two ways to
//! work with versions. If you know what version you need to work with at compile time, you can use
//! the explicit structs [`MegV1`][version::MegV1], [`MegV2`][version::MegV2], and
//! [`MegV3`][version::MegV3]. If you need to dynamically support more than one MEGA file version,
//! you can use the enum [`MegVersion`][version::MegVersion].
//!
//! Note that this project has undergone very minimal testing, especially on the writer side. It
//! definitely reads at least some of the MEGA files from Empire at War and Great War: Western
//! Front, including encrypted ones, but I have not yet tested whether those games will accept MEGA
//! files encoded by it.
//!
//! ## Reading
//!
//! For reading, the mega file versions implement the [`ReadMegMega`][reader::ReadMegMeta] trait.
//! This provides methods to read the metadata from the MEGA file. The metadata in the file consists
//! of a list of [`FileEntry`s][reader::FileEntry] which contain the file names and the files'
//! positions within the original MEGA file.
//!
//! ```
//! use petro_meg::reader::ReadMegMeta;
//! use petro_meg::version::MegV1;
//! let data: &[u8] = // ...
//! #    include_bytes!("example.meg");
//! let files = MegV1.read_meg_meta(data).unwrap();
//! ```
//!
//! `read_meg_meta` works on any type which implements `read`.
//!
//! The [`FileEntry`][reader::FileEntry] does not contain the actual file contents. To get the
//! contents, you have two options depending on what your file source is. If you have the complete
//! file contents in a slice, you can use [`range`][reader::FileEntry::range] to get a range that
//! will slice to the file's content.
//!
//! ```
//! # use petro_meg::reader::ReadMegMeta;
//! # use petro_meg::version::MegV1;
//! let data: &[u8] = // ...
//! #    include_bytes!("example.meg");
//! let files = MegV1.read_meg_meta(data).unwrap();
//! for file in files {
//!     let content = &data[file.range()];
//! }
//! ```
//!
//! If instead your data is a type which implements Seek, you can use
//! [`extract_from`][reader::FileEntry::extract_from] to seek to the start and get a reader which
//! will limit to just the contents.
//!
//! ```
//! # use std::io::{Read, Cursor};
//! # use petro_meg::reader::{ReadMegMeta, MegReadOptions};
//! # use petro_meg::version::MegV1;
//! let options = MegReadOptions::new();
//! let mut mega_file = // ...
//! #     Cursor::new(include_bytes!("example.meg"));
//! let files = MegV1.read_meg_meta(&mut mega_file).unwrap();
//! for file in files {
//!     let mut content_reader = file.extract_from(&mut mega_file, &options).unwrap();
//!     let mut content_bytes = Vec::with_capacity(file.size() as usize);
//!     content_reader.read_to_end(&mut content_bytes).unwrap();
//! }
//! ```
//!
//! To extract Version 3 MEGA files which are encrypted, you must provide a
//! [`MegReadOptions`][reader::MegReadOptions] with the appropriate [`Key`][crypto::Key]. A key
//! consists of 16 bytes, and depends on the game you are extracting from. I do not provide keys,
//! however you can find keys for some petroglyph games on the bottom of the [Petrolution Mod Tools
//! MEGA File page](https://modtools.petrolution.net/docs/MegFileFormat).
//!
//! ### Format Guessing
//!
//! There is no clear flag in the MEGA file format for which version it is, however it is possible
//! to guess, potentially somewhat unreliably, based on the header format. To guess formats, we
//! provide the special version [`GuessVersion`][version::GuessVersion]. `GuessVersion` only works
//! for reads. Additionally, `Option<MegVersion>` implements [`ReadMegMeta`][reader::ReadMegMeta],
//! using the provided version when the option is `Some` or `GuessVersion` when it is `None`.
//!
//! ## Writing
//!
//! For writing files, the various [`version`] types (excluding `GuessVersion`) implement the
//! [`BuildMeg`][writer::BuildMeg] trait, which provides a method to construct a
//! [`MegBuilder`][writer::MegBuilder] for that version. The
//! [`BuildMeg::builder`][writer::BuildMeg::builder] is generic on the type of files to put in the
//! builder.
//!
//! ```
//! # use std::fs::File;
//! use petro_meg::writer::BuildMeg;
//! use petro_meg::version::MegV1;
//! let builder = MegV1.builder::<File>();
//! ```
//!
//! You can put files into the builder with [`insert`][writer::MegBuilder::insert]. This takes a
//! [`MegPathBuf`][path::MegPathBuf] to define the path that the file is inserted under. Unlike
//! [`PathBuf`][std::path::PathBuf], `MegPathBuf` is validated. It enforces that the path is a
//! relative path with no double slashes or relative elements like `..`. `MegPathBuf` and the
//! corresponding [`MegPath`][path::MegPath] also implement case-insensitive comparisons and treat
//! `/` and `\` the same, even on Unix-like systems. Note that when the MEGA file is built, all
//! paths will be converted to ASCII uppercase and path separators will be normalized to `\`.
//!
//! ```
//! # use std::fs::File;
//! # use petro_meg::writer::BuildMeg;
//! # use petro_meg::version::MegV1;
//! # use petro_meg::path::MegPath;
//! let mut builder = MegV1.builder();
//! let path = MegPath::from_str("some/file.txt").unwrap();
//! let data: &[u8] = // ...
//! #   b"Some File Contents";
//! builder.insert(path.to_owned(), data);
//! ```
//!
//! The files inserted into the `MegBuilder` must implement the [`FileContent`][writer::FileContent]
//! trait in addition to [`Read`][std::io::Read]. `FileContent` provides a
//! [`file_len`][writer::FileContent::file_len] method which allows the builder to determine the
//! size of the file while computing the MEGA file's metadata. This allows it to write the output
//! file sequentially, rather than needing to pad the file for the MEGA file header, write the
//! contents, then seek back to the beginning to fill in the header. `FileContent` is currently
//! implemented for [`File`][std::fs::File], `&[u8]`, [`Cursor`][std::io::Cursor] and a few other
//! types, such as `Box<T>` where `T: FileCursor`.
//!
//! Once you have inserted all the files you want into the `MegBuilder`, you can write it to an
//! output using [`MegBuilder::build`][writer::MegBuilder::build].
//!
//! ```
//! # use std::fs::File;
//! # use petro_meg::writer::BuildMeg;
//! # use petro_meg::version::MegV1;
//! # use petro_meg::path::MegPath;
//! # let mut builder = MegV1.builder();
//! # let path = MegPath::from_str("some/file.txt").unwrap();
//! # let data: &[u8] = b"Some File Contents";
//! # builder.insert(path.to_owned(), data);
//! let mut out = // ...
//! #   Vec::new();
//! builder.build(&mut out).unwrap();
//! ```
//!
//! For V3 MEGA files, `MegBuilder` provides a
//! [`set_encryption`][writer::MegBuilder::set_encryption] method which can be used to set an
//! encryption key to use to encrypt the MEGA file's contents.

#[cfg(all(any(feature = "reader", feature = "writer"), feature = "v3"))]
pub mod crypto;
pub mod path;
#[cfg(all(
    any(feature = "v1", feature = "v2", feature = "v3"),
    feature = "reader"
))]
pub mod reader;
pub mod version;
#[cfg(all(
    any(feature = "v1", feature = "v2", feature = "v3"),
    feature = "writer"
))]
pub mod writer;

/// Value used for the second ID field.
#[cfg(all(
    any(feature = "v2", feature = "v3"),
    any(feature = "reader", feature = "writer")
))]
pub(crate) const ID2: u32 = 0x3F7D70A4;
