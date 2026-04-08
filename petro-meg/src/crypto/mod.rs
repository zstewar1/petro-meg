//! Provides the [`Key`] type for dealing with encrypted MEGA files.

use std::fmt;

#[cfg(feature = "reader")]
pub mod reader;
#[cfg(feature = "writer")]
pub mod writer;

/// AES 128 Key and Initial Vector used to decrypt encrypted V3 files.
#[derive(Clone)]
pub struct Key {
    /// 16 bit AES key.
    key: [u8; 16],
    /// Initial vector used for decryption.
    iv: [u8; 16],
}

impl Key {
    /// Create a new key from the key bytes and initial vector.
    pub const fn new(key: [u8; 16], iv: [u8; 16]) -> Self {
        Self { key, iv }
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Key")
            .field("key", HexDebug::from(&self.key))
            .field("iv", HexDebug::from(&self.iv))
            .finish()
    }
}

/// Debug Prints a slice of bytes as hex.
#[derive(Copy, Clone)]
#[repr(transparent)]
struct HexDebug([u8; 16]);

impl HexDebug {
    fn from(value: &[u8; 16]) -> &Self {
        // SAFETY: both are Copy, non-Drop and have the same repr.
        unsafe { std::mem::transmute(value) }
    }
}

impl fmt::Debug for HexDebug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for val in self.0 {
            write!(f, "{val:02X}")?;
        }
        Ok(())
    }
}
