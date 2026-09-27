//! Provides utility types for working with MEGA file paths.

use std::borrow::{Borrow, BorrowMut};
use std::cmp::Ordering;
use std::fmt;
use std::hash::Hash;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::str::FromStr;

use ain::{AinChar, AinStr, AinString, AinStringFromError, AinValidationError, ain};
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

/// Creates a MegPath from a string literal, with validation checked at compile time.
#[doc(hidden)]
#[macro_export]
macro_rules! _meg_path {
    ($s:literal) => {
        const {
            match $crate::path::MegPath::from_str($s) {
                Ok(path) => path,
                Err(_) => panic!("string literal is not a valid MegPath"),
            }
        }
    };
}

#[doc(inline)]
pub use crate::_meg_path as meg_path;

/// Path length limit for Windows, which we also apply to MEGA paths, in some contexts.
#[cfg(all(
    any(feature = "v1", feature = "v2", feature = "v3"),
    any(feature = "reader", feature = "writer")
))]
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
pub struct MegPath(AinStr);

impl MegPath {
    /// An empty MegPath with no components.
    // SAFETY: Empty slice is a valid path.
    pub const EMPTY: &'static MegPath = unsafe { Self::from_ain_str_unchecked(ain!("")) };

    /// Converts bytes to a MegPath without checking that it is a valid path.
    ///
    /// Caller must ensure that the path follows the rules described for MEGA file paths.
    const unsafe fn from_ain_str_unchecked<'a>(ain: &'a AinStr) -> &'a Self {
        // SAFETY: the layouts, lifetimes, and mutability are the same. Caller is responsible for
        // enforcing path validity.
        let ptr = ain as *const AinStr as *const MegPath;
        unsafe { &*ptr }
    }

    /// Converts bytes to a mutable MegPath without checking that it is a valid path.
    ///
    /// Caller must ensure that the path follows the rules described for MEGA file paths.
    const unsafe fn from_ain_str_unchecked_mut<'a>(ain: &'a mut AinStr) -> &'a mut Self {
        // SAFETY: the layouts, lifetimes, and mutability are the same. Caller is responsible for
        // enforcing path validity.
        let ptr = ain as *mut AinStr as *mut MegPath;
        unsafe { &mut *ptr }
    }

    /// Convert an [`&AinStr`][AinStr] to a MegPath.
    pub const fn from_ain_str(ain: &AinStr) -> Result<&MegPath, MegPathError> {
        if let Err(e) = validate_path(ain) {
            return Err(e);
        }
        // SAFETY: we just checked that the path follows the rules.
        Ok(unsafe { Self::from_ain_str_unchecked(ain) })
    }

    /// Convert an [`&AinStr`][AinStr] to a MegPath.
    pub const fn from_ain_str_mut(ain: &mut AinStr) -> Result<&mut MegPath, MegPathError> {
        if let Err(e) = validate_path(ain) {
            return Err(e);
        }
        // SAFETY: we just checked that the path follows the rules.
        Ok(unsafe { Self::from_ain_str_unchecked_mut(ain) })
    }

    /// Convert bytes to a MegPath.
    pub const fn from_bytes(bytes: &[u8]) -> Result<&MegPath, MegPathError> {
        Self::from_ain_str(match AinStr::from_ascii(bytes) {
            Ok(s) => s,
            Err(_) => return Err(MegPathError::NotAscii),
        })
    }

    /// Convert bytes to a mutable MegPath.
    pub const fn from_bytes_mut(bytes: &mut [u8]) -> Result<&mut MegPath, MegPathError> {
        Self::from_ain_str_mut(match AinStr::from_ascii_mut(bytes) {
            Ok(s) => s,
            Err(_) => return Err(MegPathError::NotAscii),
        })
    }

    /// Convert a string to a MegPath.
    pub const fn from_str(str: &str) -> Result<&MegPath, MegPathError> {
        Self::from_bytes(str.as_bytes())
    }

    /// Convert a string to a MegPath.
    pub const fn from_str_mut(str: &mut str) -> Result<&mut MegPath, MegPathError> {
        // SAFETY: MegPath enforces that its contents are ASCII-7, so even when mutable it upholds
        // the invariant that the str is valid UTF-8.
        Self::from_bytes_mut(unsafe { str.as_bytes_mut() })
    }

    /// Length of the path in bytes.
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true if this path is empty.
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Get a &str representation of the bytes.
    ///
    /// This always succeeds because 7-bit ascii is always safe to convert to str without further
    /// validation.
    pub const fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Get the bytes of the path.
    pub const fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Get this path as an [`&AinStr`][AinStr].
    pub const fn as_ain_str(&self) -> &AinStr {
        &self.0
    }

    /// Get an iterator over the [`Components`] of this [`MegPath`].
    pub const fn components(&self) -> Components<'_> {
        Components { path: &self.0 }
    }

    /// Returns true if the given `other` path is a prefix of this one. Only matches whole path
    /// components.
    pub fn starts_with(&self, other: impl AsRef<MegPath>) -> bool {
        let mut self_components = self.components();
        let mut other_components = other.as_ref().components();
        loop {
            match (self_components.next(), other_components.next()) {
                // If we ran out of other components before or at the same time as self, then it is
                // is a prefix.
                (Some(_), None) | (None, None) => return true,
                // If other has more components than self, then it can't be a prefix of self.
                (None, Some(_)) => return false,
                // If we ever find a pair of components in the same position that don't match
                // between the two, then other can't be a prefix of self.
                (Some(sc), Some(oc)) if sc != oc => return false,
                // If both components are equal, we continue iterating.
                (Some(_), Some(_)) => {}
            }
        }
    }

    /// Create a new [`MegPathBuf`] by joining this path with another.
    pub fn join(&self, other: impl AsRef<MegPath>) -> MegPathBuf {
        let mut res = self.to_owned();
        res.push(other);
        res
    }

    /// Get the file name from this MegPath
    ///
    /// Since trailing slashes are not allowed and we don't allow drive letters, this is always Some
    /// unless the path is empty.
    pub fn file_name(&self) -> Option<&AinStr> {
        self.components().next_back()
    }

    /// Gets a slice corresponding to the parent of this meg path, or None if the path is empty.
    pub fn parent(&self) -> Option<&MegPath> {
        let mut components = self.components();
        components.next_back().map(|_| components.as_meg_path())
    }

    /// Extracts the extension (without the leading dot) of [`self.file_name`], if possible.
    ///
    /// The extension is:
    ///
    /// * [`None`], if there is no file name;
    /// * [`None`], if there is no embedded `.`;
    /// * [`None`], if the file name begins with `.` and has no other `.`s within;
    /// * Otherwise, the portion of the file name after the final `.`
    ///
    /// [`self.file_name`]: MegPath::file_name
    ///
    /// # Examples
    ///
    /// ```
    /// # use ain::ain;
    /// # use petro_meg::path::meg_path;
    /// assert_eq!(ain!("rs"), meg_path!("foo.rs").extension().unwrap());
    /// assert_eq!(ain!("gz"), meg_path!("foo.tar.gz").extension().unwrap());
    /// ```
    pub fn extension(&self) -> Option<&AinStr> {
        // Copied from std::path::Path::extension.
        self.file_name()
            .map(stdutil::rsplit_file_at_dot)
            .and_then(|(before, after)| before.and(after))
    }

    /// Extracts the stem (non-extension) portion of [`self.file_name`].
    ///
    /// [`self.file_name`]: MegPath::file_name
    ///
    /// The stem is:
    ///
    /// * [`None`], if there is no file name;
    /// * The entire file name if there is no embedded `.`;
    /// * The entire file name if the file name begins with `.` and has no other `.`s within;
    /// * Otherwise, the portion of the file name before the final `.`
    ///
    /// # Examples
    ///
    /// ```
    /// # use ain::ain;
    /// # use petro_meg::path::meg_path;
    /// assert_eq!(ain!("foo"), meg_path!("foo.rs").file_stem().unwrap());
    /// assert_eq!(ain!("foo.tar"), meg_path!("foo.tar.gz").file_stem().unwrap());
    /// ```
    ///
    /// # See Also
    /// This method is similar to [`MegPath::file_prefix`], which extracts the portion of the file
    /// name before the *first* `.`
    pub fn file_stem(&self) -> Option<&AinStr> {
        // Copied from std::path::Path::file_stem.
        self.file_name()
            .map(stdutil::rsplit_file_at_dot)
            .and_then(|(before, after)| before.or(after))
    }

    /// Extracts the prefix of [`self.file_name`].
    ///
    /// The prefix is:
    ///
    /// * [`None`], if there is no file name;
    /// * The entire file name if there is no embedded `.`;
    /// * The portion of the file name before the first non-beginning `.`;
    /// * The entire file name if the file name begins with `.` and has no other `.`s within;
    /// * The portion of the file name before the second `.` if the file name begins with `.`
    ///
    /// [`self.file_name`]: MegPath::file_name
    ///
    /// # Examples
    ///
    /// ```
    /// # use ain::ain;
    /// # use petro_meg::path::meg_path;
    /// assert_eq!(ain!("foo"), meg_path!("foo.rs").file_prefix().unwrap());
    /// assert_eq!(ain!("foo"), meg_path!("foo.tar.gz").file_prefix().unwrap());
    /// assert_eq!(ain!(".config"), meg_path!(".config").file_prefix().unwrap());
    /// assert_eq!(ain!(".config"), meg_path!(".config.toml").file_prefix().unwrap());
    /// ```
    ///
    /// # See Also
    /// This method is similar to [`MegPath::file_stem`], which extracts the portion of the file
    /// name before the *last* `.`
    pub fn file_prefix(&self) -> Option<&AinStr> {
        // Copied from std::path::Path::file_prefix.
        self.file_name()
            .map(stdutil::split_file_at_dot)
            .and_then(|(before, _after)| Some(before))
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

    /// Makes this MegPath into a normalized path by converting to ASCII uppercase and switching any
    /// '/' separators for '\\'.
    pub const fn make_normalized(&mut self) {
        let mut ain = self.0.as_mut_slice();
        // Manual loop to allow this to be const-fn
        while let [ch, rest @ ..] = ain {
            *ch = to_normalized(*ch);
            ain = rest;
        }
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
        for component in self.components() {
            component.hash(state);
        }
    }
}

impl AsRef<MegPath> for MegPath {
    fn as_ref(&self) -> &MegPath {
        self
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
    path: &'a AinStr,
}

impl<'a> Components<'a> {
    /// Gets the remainder of the path as a MegPath.
    pub fn as_meg_path(&self) -> &'a MegPath {
        // SAFETY: we start with a valid MegPath and always remove whole path components, never
        // leaving separators, so we can be sure that any such trimmed path remains valid.
        unsafe { MegPath::from_ain_str_unchecked(self.path) }
    }
}

impl<'a> Iterator for Components<'a> {
    type Item = &'a AinStr;

    fn next(&mut self) -> Option<Self::Item> {
        // Since we don't allow trailing slashes, we don't need to worry about the normal 'split'
        // case where you've sliced to the last element but still need to return that empty slice,
        // the way "a/".split("/") would.
        if self.path.is_empty() {
            return None;
        }
        let next;
        match self.path.find(is_dir_separator) {
            Some(idx) => {
                next = &self.path[..idx];
                // idx must be less than len.
                self.path = &self.path[idx + 1..];
            }
            None => {
                next = self.path;
                self.path = AinStr::empty();
            }
        }
        Some(next)
    }
}

impl<'a> DoubleEndedIterator for Components<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.path.is_empty() {
            return None;
        }
        let next;
        match self.path.rfind(is_dir_separator) {
            Some(idx) => {
                // idx must be less than len.
                next = &self.path[idx + 1..];
                self.path = &self.path[..idx];
            }
            None => {
                next = self.path;
                self.path = AinStr::empty();
            }
        }
        Some(next)
    }
}

/// Checks that the given bytes satisfy the MEGA path rules specified on [`MegPath`].
///
/// If this returns OK then it is safe to cast the bytes to a [`MegPath`].
const fn validate_path(ain: &AinStr) -> Result<(), MegPathError> {
    if ain.is_empty() {
        return Ok(());
    }
    let mut ain = ain.as_slice();
    // Pre-check that the first and last characters are not dir separators and that the end of the
    // path is allowed at the end of a path component.
    match ain {
        [ch, ..] if is_dir_separator(*ch) => return Err(MegPathError::LeadingSlash),
        [.., ch] if is_dir_separator(*ch) => return Err(MegPathError::TrailingSlash),
        [.., ch] if !is_valid_component_end(*ch) => return Err(MegPathError::InvalidComponent),
        _ => {}
    }
    loop {
        match ain {
            // If we ever encounter a character that's not a valid component nor a valid dir
            // separator, that's an invalid character.
            [ch, ..] if !is_valid_component(*ch) && !is_dir_separator(*ch) => {
                return Err(MegPathError::InvalidCharacter);
            }
            // Any time there are two sequential dir separators, that's an empty component.
            [d1, d2, ..] if is_dir_separator(*d1) && is_dir_separator(*d2) => {
                return Err(MegPathError::EmptyComponent);
            }
            // If the character just before a dir separator is not valid as a component end, that's
            // an invalid component. This also blocks components consisting entirely of dots, such
            // as '.' or '..'.
            [c, d, ..] if is_dir_separator(*d) && !is_valid_component_end(*c) => {
                return Err(MegPathError::InvalidComponent);
            }
            // Advance by one character if the current character doesn't break any rules.
            [_, rest @ ..] => ain = rest,
            [] => break,
        }
    }
    Ok(())
}

/// Error for invalid conversions from str and bytes to [`MegPath`].
#[derive(Debug, Error, PartialEq, Eq)]
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

impl From<AinValidationError> for MegPathError {
    fn from(_: AinValidationError) -> Self {
        Self::NotAscii
    }
}

impl<T> From<AinStringFromError<T>> for MegPathError {
    fn from(_: AinStringFromError<T>) -> Self {
        Self::NotAscii
    }
}

/// Owned container for a [`MegPath`], with the same validation requirements.
#[derive(Default, Clone, Eq)]
pub struct MegPathBuf(AinString);

impl MegPathBuf {
    /// Creates a new empty [`MegPathBuf`].
    pub const fn new() -> Self {
        Self(AinString::new())
    }

    /// Convert an [`AinString`] to a MEGA path.
    pub fn from_ain_string(ain: AinString) -> Result<Self, MegPathError> {
        if let Err(e) = validate_path(ain.as_ain_str()) {
            return Err(e);
        }
        Ok(Self(ain))
    }

    /// Convert a vector of bytes to a MEGA path.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, MegPathError> {
        Self::from_ain_string(AinString::from_ascii(bytes)?)
    }

    /// Convert a string into a MEGA path.
    pub fn from_string(string: String) -> Result<Self, MegPathError> {
        Self::from_bytes(string.into_bytes())
    }

    /// Coerces to a &MegPath slice.
    pub fn as_meg_path(&self) -> &MegPath {
        &self
    }

    /// Extracts the underlying bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0.into_bytes()
    }

    /// Extracts the contents as a string.
    pub fn into_string(self) -> String {
        self.0.into_string()
    }

    /// Extracts the contents as an AinString.
    pub fn into_ain_string(self) -> AinString {
        self.0
    }

    /// Push another MegPath to the end of this one.
    pub fn push(&mut self, rest: impl AsRef<MegPath>) {
        let rest = rest.as_ref();
        // If rest is an empty path, don't push anything, since that would create a path with a
        // trailing slash, which we specifically don't allow.
        if rest.is_empty() {
            return;
        }
        // If rest is not empty, then it must not start or end with a slash, so all we have to do is
        // insert a slash and then copy the rest of that path into our buffer.
        self.0.reserve(1 + rest.len());
        self.0.push(ain!('\\'));
        self.0.push_str(rest.as_ain_str());
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
        unsafe { MegPath::from_ain_str_unchecked(&self.0) }
    }
}

impl DerefMut for MegPathBuf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: the buffer enforces that paths remain valid.
        unsafe { MegPath::from_ain_str_unchecked_mut(&mut self.0) }
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

impl Borrow<MegPath> for &MegPathBuf {
    fn borrow(&self) -> &MegPath {
        self
    }
}

impl BorrowMut<MegPath> for MegPathBuf {
    fn borrow_mut(&mut self) -> &mut MegPath {
        self
    }
}

impl Borrow<MegPath> for &mut MegPathBuf {
    fn borrow(&self) -> &MegPath {
        self
    }
}

impl BorrowMut<MegPath> for &mut MegPathBuf {
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
pub(crate) const fn is_dir_separator(ch: AinChar) -> bool {
    let b = ch.to_u8();
    b == b'/' || b == b'\\'
}

/// Returns true if c is valid as the last character in a path component.
const fn is_valid_component_end(ch: AinChar) -> bool {
    is_valid_component(ch) && {
        let b = ch.to_u8();
        b != b'.' && b != b' '
    }
}

/// Returns true if the character is valid for a path component.
pub(crate) const fn is_valid_component(ch: AinChar) -> bool {
    let b = ch.to_u8();
    b > 31
        && !is_dir_separator(ch)
        && b != b'<'
        && b != b'>'
        && b != b':'
        && b != b'"'
        && b != b'|'
        && b != b'?'
        && b != b'*'
}

/// Return true if the characters are all valid within MEGA file paths (though not necessarily in
/// their current positions).
#[cfg(all(feature = "dynamic_version", feature = "reader"))]
pub(crate) fn is_valid_path_chars(chars: &[u8]) -> bool {
    chars.iter().all(|&b| {
        AinChar::from_u8(b).map_or(false, |ch| is_dir_separator(ch) || is_valid_component(ch))
    })
}

/// Normalizes an ascii byte for the MEGA format by converting to uppercase and replacing '/' with
/// '\\'.
pub(crate) const fn to_normalized(ch: AinChar) -> AinChar {
    if ch.to_u8() == b'/' {
        ain!('\\')
    } else {
        ch.to_uppercase()
    }
}

/// Utility functions which have been borrowed from the Rust standard libary, e.g. std::path under
/// the terms of the MIT License and modified to work on MegPaths and components.
mod stdutil {
    use ain::{AinChar, AinStr, ain};

    /// basic workhorse for splitting stem and extension
    pub(super) fn rsplit_file_at_dot(file: &AinStr) -> (Option<&AinStr>, Option<&AinStr>) {
        // This check from 'std' is skipped, because MegPath does not allow relative components.
        // if file == ain!("..") {
        //     return (Some(file), None);
        // }

        let mut iter = file.rsplitn(2, |ch: AinChar| ch.to_u8() == b'.');
        let after = iter.next();
        let before = iter.next();
        if before == Some(ain!("")) {
            (Some(file), None)
        } else {
            (before, after)
        }
    }

    pub(super) fn split_file_at_dot(file: &AinStr) -> (&AinStr, Option<&AinStr>) {
        // This check from 'std' is skipped, because MegPath does not allow relative components.
        // if file == ain!("..") {
        //     return (file, None);
        // }

        let i = match file.as_bytes()[1..].iter().position(|b| *b == b'.') {
            Some(i) => i + 1,
            None => return (file, None),
        };
        let before = &file[..i];
        let after = &file[i + 1..];
        (before, Some(after))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::Hasher;

    #[derive(PartialEq, Eq, Debug, Default)]
    struct MockHash {
        written: Vec<u8>,
    }

    impl Hasher for MockHash {
        fn write(&mut self, bytes: &[u8]) {
            self.written.extend_from_slice(bytes);
        }

        fn finish(&self) -> u64 {
            todo!("Not needed")
        }
    }

    #[test]
    fn hashes_by_component() {
        let same1 = meg_path!("a/path/with/Components");
        let same2 = meg_path!("A\\path/with\\COMPONENTS");
        let different1 = meg_path!("Apath/withCOMPONENTS");

        let mut h1 = MockHash::default();
        same1.hash(&mut h1);
        let mut h2 = MockHash::default();
        same2.hash(&mut h2);
        let mut h3 = MockHash::default();
        different1.hash(&mut h3);
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn validation_succeeds() {
        const CASES: &[&AinStr] = &[
            ain!(""),
            ain!("A/b"),
            ain!("A\\b"),
            ain!("has/.leading"),
            ain!("has/.leading.ext"),
            ain!("has/some.ext"),
        ];
        for case in CASES {
            assert!(
                validate_path(case).is_ok(),
                "{case:?} unexpectedly fails validation"
            );
        }
    }

    #[test]
    fn validation_fails() {
        use MegPathError::*;
        const CASES: &[(&AinStr, MegPathError)] = &[
            (ain!("."), InvalidComponent),
            (ain!(".."), InvalidComponent),
            (ain!("/"), LeadingSlash),
            (ain!("/leading"), LeadingSlash),
            (ain!("/leading/with/multiple"), LeadingSlash),
            (ain!("trailing/with\\multiple\\"), TrailingSlash),
            (ain!("consecutive/\\slashes"), EmptyComponent),
            (ain!("dot\\in\\filename."), InvalidComponent),
            (ain!("dot/in/mid./filename"), InvalidComponent),
            (ain!("relative/../component"), InvalidComponent),
            (ain!("invalid\x03/characters"), InvalidCharacter),
        ];
        for (input, expected) in CASES {
            match validate_path(input) {
                Ok(()) => panic!("{input:?} unexpectedly passes validation"),
                Err(e) => assert_eq!(
                    e, *expected,
                    "{input:?} produced unexpected error {e:?} instead of {expected:?}"
                ),
            }
        }
    }
}
