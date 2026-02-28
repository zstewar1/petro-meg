//! Provides utility types for working with MEGA file paths.

use std::borrow::{Borrow, BorrowMut};
use std::cmp::Ordering;
use std::fmt;
use std::hash::Hash;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::str::FromStr;

use thiserror::Error;

macro_rules! impl_path_cmp {
    (@variants $lhs:ident, $rhs:ident) => {
        impl_path_cmp!(@impl $lhs, $rhs);
        impl_path_cmp!(@impl &$lhs, $rhs);
        impl_path_cmp!(@impl $lhs, &$rhs);
    };
    (@impl $lhs:ty, $rhs:ty) => {
        impl PartialEq<$rhs> for $lhs {
            fn eq(&self, other: &$rhs) -> bool {
                self.components().eq(other.components())
            }
        }

        impl PartialOrd<$rhs> for $lhs {
            fn partial_cmp(&self, other: &$rhs) -> Option<Ordering> {
                Some(self.components().cmp(other.components()))
            }
        }
    };
}

/// Path length limit for Windows, which we also apply to MEGA paths, in some contexts.
pub(crate) const WIN_PATH_LIMIT: usize = 260;

/// Represents a path in a Petroglyph MEGA file.
///
/// The path type performs relatively strict validation to avoid constructing MEGA files which don't
/// work with the game. The actual validation requirements needed to work with the game(s) are
/// somewhat unclear. If it turns out that actual game files or common mods use paths which don't
/// satisfy the validation, the validation can be relaxed.
///
/// The current requirements are:
///
/// -   Be ASCII-7 only.
/// -   Not contain any characters which are invalid in windows file names (e.g. ':', '<', etc.)
/// -   Be relative (no leading '/', '\', or Drive letter)
/// -   Not contain path traversal operations (e.g. "..")
/// -   Not have any component ending in '.' or ' ' (space)
/// -   Not have any empty components (e.g. a "//" producing an empty directory name between the two
///     slashes or a trailing '/' or '\' producing an empty file name).
///
/// We do not check for windows reserved names such as 'COM'.
///
/// To match how paths are handled on most windows file systems and the fact that most Petroglyph
/// seem to merge file names from mods into the game in a case-insensitive way, [`MegPath`] is
/// case-insensitive for equality, hashing, and comparisons, though the original case is preserved.
///
/// Both '/' and '\' are treated equally for equality and comparisons, though the original separator
/// is preserved.
///
/// Methods are provided to normalize both path separators and cases.
///
/// Because MegPaths are ASCII only, they can be safely converted to `&str`.
#[derive(Eq)]
#[repr(transparent)]
pub struct MegPath([u8]);

impl MegPath {
    /// An empty MegPath with no components.
    // SAFETY: Empty slice is a valid path.
    pub const EMPTY: &'static MegPath = unsafe { Self::from_bytes_unchecked(&[]) };

    /// Converts bytes to a MegPath without checking that it is a valid path.
    ///
    /// Caller must ensure that the path follows the rules described for MEGA file paths.
    const unsafe fn from_bytes_unchecked<'a>(bytes: &'a [u8]) -> &'a Self {
        // SAFETY: the layouts, lifetimes, and mutability are the same. Caller is responsible for
        // enforcing path validity.
        unsafe { std::mem::transmute::<&'a [u8], &'a MegPath>(bytes) }
    }

    /// Converts bytes to a mutable MegPath without checking that it is a valid path.
    ///
    /// Caller must ensure that the path follows the rules described for MEGA file paths.
    unsafe fn from_bytes_unchecked_mut<'a>(bytes: &'a mut [u8]) -> &'a mut Self {
        // SAFETY: the layouts, lifetimes, and mutability are the same. Caller is responsible for
        // enforcing path validity.
        unsafe { std::mem::transmute::<&'a mut [u8], &'a mut MegPath>(bytes) }
    }

    /// Convert bytes to a MegPath.
    pub fn from_bytes(bytes: &[u8]) -> Result<&MegPath, MegPathError> {
        validate_path(bytes)?;
        // SAFETY: we just checked that the path follows the rules.
        Ok(unsafe { Self::from_bytes_unchecked(bytes) })
    }

    /// Convert bytes to a mutable MegPath.
    pub fn from_bytes_mut(bytes: &mut [u8]) -> Result<&mut MegPath, MegPathError> {
        validate_path(bytes)?;
        // SAFETY: we just checked that the path follows the rules.
        Ok(unsafe { Self::from_bytes_unchecked_mut(bytes) })
    }

    /// Convert a string to a MegPath.
    pub fn from_str(str: &str) -> Result<&MegPath, MegPathError> {
        Self::from_bytes(str.as_bytes())
    }

    /// Convert a string to a MegPath.
    pub fn from_str_mut(str: &mut str) -> Result<&mut MegPath, MegPathError> {
        // SAFETY: MegPath enforces that its contents are ASCII-7, so even when mutable it upholds
        // the invariant that the str is valid UTF-8.
        Self::from_bytes_mut(unsafe { str.as_bytes_mut() })
    }

    /// Length of the path in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true if this path is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Get a &str representation of the bytes.
    ///
    /// This always succeeds because 7-bit ascii is always safe to convert to str without further
    /// validation.
    pub fn as_str(&self) -> &str {
        // SAFETY: we require components to be ASCII-7, which is always valid UTF-8
        unsafe { str::from_utf8_unchecked(&self.0) }
    }

    /// Get the bytes of the path.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Get an iterator over the [`Components`] of this [`MegPath`].
    pub fn components(&self) -> Components<'_> {
        Components { path: &self.0 }
    }

    /// Get the file name from this MegPath
    ///
    /// Since trailing slashes are not allowed and we don't allow drive letters, this is always Some
    /// unless the path is empty.
    pub fn file_name(&self) -> Option<&Component> {
        if self.is_empty() {
            None
        } else {
            // Since our paths are restricted, this check is relatively easy.
            let name_start = match self.0.iter().rposition(|&b| is_dir_separator(b)) {
                Some(idx) => idx + 1,
                None => 0,
            };
            let slice = &self.0[name_start..];
            // SAFETY: The MegPath is already validated and either it contained no dir separators or
            // we sliced to only the content after the last dir separator.
            Some(unsafe { Component::from_bytes_unchecked(slice) })
        }
    }

    /// Creates a PathBuf with the components of this [`MegPath`].
    ///
    /// Case is preserved.
    pub fn to_path_buf(&self) -> PathBuf {
        let mut out = PathBuf::with_capacity(self.len());
        for component in self.components() {
            out.push(component.as_str());
        }
        out
    }
}

impl_path_cmp!(@variants MegPath, MegPath);
impl_path_cmp!(@variants MegPath, MegPathBuf);

impl Ord for MegPath {
    fn cmp(&self, other: &Self) -> Ordering {
        self.components().cmp(other.components())
    }
}

impl Hash for MegPath {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let mut first = true;
        for component in self.components() {
            if !first {
                // Normalize all path separators.
                state.write_u8(b'\\');
            }
            first = false;
            component.hash(state);
        }
    }
}

impl AsRef<str> for MegPath {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl ToOwned for MegPath {
    type Owned = MegPathBuf;

    fn to_owned(&self) -> Self::Owned {
        MegPathBuf(self.0.to_owned())
    }

    fn clone_into(&self, target: &mut Self::Owned) {
        self.0.clone_into(&mut target.0)
    }
}

impl fmt::Debug for MegPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for MegPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

/// Components of a [`MegPath`].
pub struct Components<'a> {
    /// Remaining MegPath to return components for.
    path: &'a [u8],
}

impl<'a> Iterator for Components<'a> {
    type Item = &'a Component;

    fn next(&mut self) -> Option<Self::Item> {
        // Since we don't allow trailing slashes, we don't need to worry about the normal 'split'
        // case where you've sliced to the last element but still need to return that empty slice,
        // the way "a/".split("/") would.
        if self.path.is_empty() {
            return None;
        }
        let (next, rest) = split_next_component_unchecked(self.path);
        self.path = rest;
        // SAFETY: Because the input was a valid path, this must be a valid component.
        Some(unsafe { Component::from_bytes_unchecked(next) })
    }
}

/// Checks that the given bytes satisfy the MEGA path rules specified on [`MegPath`].
///
/// If this returns OK then it is safe to cast the bytes to a [`MegPath`].
fn validate_path(bytes: &[u8]) -> Result<(), MegPathError> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut component_start = 0;
    for (idx, &byte) in bytes.iter().enumerate() {
        if !byte.is_ascii() {
            return Err(MegPathError::NotAscii);
        }
        if is_dir_separator(byte) {
            if idx == 0 {
                return Err(MegPathError::LeadingSlash);
            }
            if idx == bytes.len() - 1 {
                return Err(MegPathError::TrailingSlash);
            }
            if idx - component_start == 0 {
                return Err(MegPathError::EmptyComponent);
            }
            // this also implies that the component cannot be '.' or '..'.
            if !is_valid_component_end(bytes[idx - 1]) {
                return Err(MegPathError::InvalidComponent);
            }
            component_start = idx + 1;
        } else if !is_valid_component(byte) {
            return Err(MegPathError::InvalidCharacter);
        }
    }
    Ok(())
}

/// Splits bytes representing a MEGA file path at the next separator '/' or '\', without checking
/// component produced is valid. Returns a tuple of (new component, remaining path)
fn split_next_component_unchecked(path: &[u8]) -> (&[u8], &[u8]) {
    let split_point = path.iter().copied().position(is_dir_separator);
    match split_point {
        // separator + 1 is safe because the separator was found at `separator`, so the next
        // index must be no more than `path.len()`.
        Some(separator) => (&path[..separator], &path[separator + 1..]),
        None => (path, &[]),
    }
}

/// A MEGA file path component. Must be 7-bit ASCII and not contain any slashes or characters which
/// are invalid in windows path names.
#[repr(transparent)]
#[derive(Eq)]
pub struct Component([u8]);

impl Component {
    /// Converts bytes to a Component without checking that it is a valid component name.
    ///
    /// Caller must ensure that the path follows the rules described for MEGA file paths.
    unsafe fn from_bytes_unchecked<'a>(bytes: &'a [u8]) -> &'a Self {
        debug_assert!(!bytes.is_empty(), "Components must not be empty");
        // SAFETY: the layouts, lifetimes, and mutability are the same. Caller is responsible for
        // enforcing path validity.
        unsafe { std::mem::transmute::<&'a [u8], &'a Component>(bytes) }
    }

    /// Length of the component in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Get a &str representation of the bytes.
    ///
    /// This always succeeds because 7-bit ascii is always safe to convert to str without further
    /// validation.
    pub fn as_str(&self) -> &str {
        // SAFETY: we require components to be ASCII-7, which is always valid UTF-8
        unsafe { str::from_utf8_unchecked(&self.0) }
    }
}

impl PartialEq for Component {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl PartialEq<str> for Component {
    fn eq(&self, other: &str) -> bool {
        self.0.eq_ignore_ascii_case(other.as_bytes())
    }
}

impl PartialEq<Component> for str {
    fn eq(&self, other: &Component) -> bool {
        self.as_bytes().eq_ignore_ascii_case(&other.0)
    }
}

impl Hash for Component {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        const BLOCK_SIZE: usize = 64;
        let mut buf = [0u8; BLOCK_SIZE];
        for chunk in self.0.chunks(BLOCK_SIZE) {
            let lower = &mut buf[0..chunk.len()];
            lower.copy_from_slice(chunk);
            lower.make_ascii_lowercase();
            state.write(lower);
        }
    }
}

impl PartialOrd for Component {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Component {
    fn cmp(&self, other: &Self) -> Ordering {
        const BLOCK_SIZE: usize = 64;
        let mut lhs_buf = [0u8; BLOCK_SIZE];
        let mut rhs_buf = [0u8; BLOCK_SIZE];
        let mut lhs_iter = self.0.chunks(BLOCK_SIZE);
        let mut rhs_iter = other.0.chunks(BLOCK_SIZE);
        loop {
            match (lhs_iter.next(), rhs_iter.next()) {
                // Both ran out at the same time without finding any differences, so they're equal.
                (None, None) => return Ordering::Equal,
                // Left ran out first while being equal up to this point, so right is greater.
                (None, Some(_)) => return Ordering::Less,
                // Right ran out first while being equal up to this point, so left is greater.
                (Some(_), None) => return Ordering::Greater,
                (Some(lhs_chunk), Some(rhs_chunk)) => {
                    let lhs_lower = &mut lhs_buf[0..lhs_chunk.len()];
                    let rhs_lower = &mut rhs_buf[0..rhs_chunk.len()];
                    lhs_lower.copy_from_slice(lhs_chunk);
                    rhs_lower.copy_from_slice(rhs_chunk);
                    lhs_lower.make_ascii_lowercase();
                    rhs_lower.make_ascii_lowercase();
                    match (*lhs_lower).cmp(rhs_lower) {
                        // If this block was equal, try the next pair of blocks.
                        // Note that Chunks only differes in length at the end. If there is a lenght
                        // difference the slices won't compare equal. So if we get to Equal, either
                        // the full slices are equal or these blocks are fully equal.
                        Ordering::Equal => {}
                        different => return different,
                    }
                }
            }
        }
    }
}

impl AsRef<str> for Component {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<MegPath> for Component {
    fn as_ref(&self) -> &MegPath {
        // SAFETY: A single path component is always a valid path.
        unsafe { MegPath::from_bytes_unchecked(&self.0) }
    }
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl fmt::Debug for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

/// Error for invalid conversions from str and bytes to [`MegPath`].
#[derive(Debug, Error)]
pub enum MegPathError {
    #[error("Path contained a non-ascii character")]
    NotAscii,
    #[error("Path started with a leading slash")]
    LeadingSlash,
    #[error("Path ended with a trailing slash")]
    TrailingSlash,
    #[error("Path contained an empty path component, likely caused by a double slash, e.g \"//\"")]
    EmptyComponent,
    #[error("Path contained an ascii character which isn't permitted in a path component")]
    InvalidCharacter,
    #[error(
        "Path contained an invalid component, such as '.', '..' or any string ending in '.' or ' '."
    )]
    InvalidComponent,
}

/// Owned container for a [`MegPath`], with the same validation requirements.
#[derive(Clone, Eq)]
pub struct MegPathBuf(Vec<u8>);

impl MegPathBuf {
    /// Creates a new empty [`MegPathBuf`].
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Convert a vector of bytes to a MEGA path.
    pub fn from_vec(bytes: Vec<u8>) -> Result<Self, MegPathError> {
        validate_path(&bytes)?;
        Ok(Self(bytes))
    }

    /// Convert a string into a MEGA path.
    pub fn from_string(string: String) -> Result<Self, MegPathError> {
        Self::from_vec(string.into_bytes())
    }
}

impl_path_cmp!(@variants MegPathBuf, MegPath);
impl_path_cmp!(@variants MegPathBuf, MegPathBuf);

impl Hash for MegPathBuf {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        MegPath::hash(self, state)
    }
}

impl Ord for MegPathBuf {
    fn cmp(&self, other: &Self) -> Ordering {
        self.components().cmp(other.components())
    }
}

impl FromStr for MegPathBuf {
    type Err = MegPathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        MegPath::from_str(s).map(ToOwned::to_owned)
    }
}

impl Deref for MegPathBuf {
    type Target = MegPath;

    fn deref(&self) -> &Self::Target {
        // SAFETY: the buffer enforces that paths remain valid.
        unsafe { MegPath::from_bytes_unchecked(&self.0) }
    }
}

impl DerefMut for MegPathBuf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: the buffer enforces that paths remain valid.
        unsafe { MegPath::from_bytes_unchecked_mut(&mut self.0) }
    }
}

impl AsRef<MegPath> for MegPathBuf {
    fn as_ref(&self) -> &MegPath {
        self
    }
}

impl AsMut<MegPath> for MegPathBuf {
    fn as_mut(&mut self) -> &mut MegPath {
        self
    }
}

impl Borrow<MegPath> for MegPathBuf {
    fn borrow(&self) -> &MegPath {
        self
    }
}

impl BorrowMut<MegPath> for MegPathBuf {
    fn borrow_mut(&mut self) -> &mut MegPath {
        self
    }
}

impl AsRef<str> for MegPathBuf {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for MegPathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for MegPathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

/// Check if the given ascii character is a valid directory separator.
pub(crate) fn is_dir_separator(c: u8) -> bool {
    c == b'/' || c == b'\\'
}

/// Returns true if c is valid as the last character in a path component.
fn is_valid_component_end(c: u8) -> bool {
    is_valid_component(c) && c != b'.' && c != b' '
}

/// Returns true if the character is valid for a path component.
pub(crate) fn is_valid_component(c: u8) -> bool {
    c.is_ascii()
        && c > 31
        && !is_dir_separator(c)
        && c != b'<'
        && c != b'>'
        && c != b':'
        && c != b'"'
        && c != b'|'
        && c != b'?'
        && c != b'*'
}
