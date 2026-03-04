use std::fmt;
use std::io::{self, BufRead, Read, Write};

use aes::cipher::{BlockDecryptMut as _, BlockEncryptMut as _, KeyIvInit as _};

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type Aes129CbcEnc = cbc::Encryptor<aes::Aes128>;

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
    pub fn new(key: [u8; 16], iv: [u8; 16]) -> Self {
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

/// 128 bit AES block size.
const BLOCK_SIZE: usize = 16;

/// Rounds the given value up to the next multiple of the block size. Panics on overflow.
pub(crate) fn round_up_to_block(n: u64) -> u64 {
    n.checked_next_multiple_of(BLOCK_SIZE as u64)
        .expect("Rounding up to a multiple of the block size overflowed")
}

/// Round the number down to the next smaller multiple of the block size.
fn round_down_to_block(n: usize) -> usize {
    n - (n % BLOCK_SIZE)
}

/// Default buffer size for the decrypting reader.
const DEFAULT_BUFFER_SIZE: usize = BLOCK_SIZE * 256;

/// Reader which wraps another reader to make it perform AES 128 CBC decryption on a stream.
///
/// The underlying stream must be a multiple of the block size (128 bits/16 bytes), otherwise an
/// `UnexpectedEof` error will be produced when the end of the stream is reached outside of a
pub(crate) struct DecryptingReader<R: ?Sized> {
    /// Decoder used to perform actual decryption.
    dec: Aes128CbcDec,
    /// Buffer for a fixed number of blocks. Len will always be a multiple of BLOCK_SIZE.
    buf: Box<[u8]>,
    /// Position within the buffer that as been filled up to. Always <= decrypted.
    pos: usize,
    /// Position within the buffer that has been decrypted up to. Always <= filled and will always
    /// be a multiple of the block size.
    ///
    /// Decrypted will always be the largest multiple of 128 bits which is less than or equal to
    /// filled.
    decrypted: usize,
    /// Position within the buffer that is currently filled with valid data. Always <= buf.len().
    filled: usize,
    /// Inner reader that provides the source data.
    ///
    /// Placing this last allows it to be unsized, so you can unsize from DecryptingReader<T> ->
    /// DecryptingReader<dyn Read>.
    inner: R,
}

impl<R> DecryptingReader<R> {
    /// Create a reader which decrypts from the given inner reader using the specified key.
    pub(crate) fn new(inner: R, key: &Key) -> Self {
        Self::new_with_capacity(inner, key, DEFAULT_BUFFER_SIZE)
    }

    /// Create a reader which decrypts from the given inner reader using the specified key and
    /// buffer size. Buffer size will be rounded up to the next multiple of the block size.
    ///
    /// Panics
    pub(crate) fn new_with_capacity(inner: R, key: &Key, capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "must have positive capacity in order to decrypt"
        );
        let capacity = round_up_to_block(capacity as u64);
        assert!(
            capacity <= usize::MAX as u64,
            "Next block size above capacity exceeds uszie max"
        );
        let buf = Box::new_zeroed_slice(capacity as usize);
        // SAFETY: 0 is always a valid value for u8.
        let buf = unsafe { buf.assume_init() };
        Self {
            dec: Aes128CbcDec::new((&key.key).into(), (&key.iv).into()),
            buf,
            pos: 0,
            decrypted: 0,
            filled: 0,
            inner,
        }
    }

    /// Unwraps the inner reader. The buffer is discarded, so any data that has been read into the
    /// decryption buffer will be lost.
    pub(crate) fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: ?Sized> DecryptingReader<R> {
    /// Shifts down any bytes in the buffer which have been filled but not decrypted.
    ///
    /// Because of the invariant that `decrypted` is the largest multiple of `BLOCK_SIZE` less than
    /// or equal to `filled`, this will move at most 15 bytes.
    fn shift(&mut self) {
        let remaining_filled = self.filled - self.decrypted;
        self.buf.copy_within(self.decrypted..self.filled, 0);
        self.pos = 0;
        self.decrypted = 0;
        self.filled = remaining_filled;
    }
}

impl<R: ?Sized + Read> Read for DecryptingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Since we always need to decrypt, we can't use the trick from BufReader of bypassing the
        // internal buffer for large reads.
        let mut rem = self.fill_buf()?;
        let nread = rem.read(buf)?;
        self.consume(nread);
        Ok(nread)
    }
}

impl<R: ?Sized + Read> BufRead for DecryptingReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        // If we've reached the end of the decrypted portion of our internal buffer, then we need to
        // fetch more data from the reader and decrypt it.

        // Branch using `>=` instead of the more correct `==` to tell the compiler that the pos..cap
        // slice is always valid.
        // Not sure of the logic of this, its copied from the standard library's BufReader approach.
        if self.pos >= self.decrypted {
            debug_assert!(self.pos == self.decrypted);
            self.shift();

            // Unlike BufReader, which can accept any length of read output from the underlying
            // reader, we have to read multiples of the block size. Read to fill the remaining space
            // in the buffer until we have at least one block filled.
            while let pending_decrypt = self.filled - self.decrypted
                && pending_decrypt < BLOCK_SIZE
            {
                match self.inner.read(&mut self.buf[self.filled..]) {
                    // The inner reader hit EOF and we are at a block boundary, so we can also EOF
                    // safely without leaving encrypted data unread.
                    // We know at this point that pos == decrypted still.
                    Ok(0) if pending_decrypt == 0 => {
                        return Ok(&self.buf[self.pos..self.decrypted]);
                    }
                    // The inner reader hit EOF and we are not on a block boundary and have not yet
                    // read at least one full block of data. The EOF is therefore unexpected.
                    Ok(0) => {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "The end of the underlying byte stream was not at an AES-128 Block \
                            Boundary",
                        ));
                    }
                    Ok(n) => {
                        self.filled += n;
                    }
                    // It should be safe to error here, as our pos, decrypted, and filled are all
                    // valid still. When `fill_buf` is used to implement `read`, this is safe,
                    // because even though bytes may have been read from the underlying reader, they
                    // have not been read from the buffered reader yet, so I think that still
                    // satisfies the API constraint placed on the read method.
                    Err(e) => return Err(e),
                }
            }

            // Find how many blocks we have to decrypt by truncating filled to a multiple of the
            // block size.
            while let next_block_end = self.decrypted + BLOCK_SIZE
                && next_block_end <= self.filled
            {
                let block = &mut self.buf[self.decrypted..next_block_end];
                self.dec.decrypt_block_mut(block.into());
                self.decrypted = next_block_end;
            }
        }
        Ok(&self.buf[self.pos..self.decrypted])
    }

    fn consume(&mut self, amt: usize) {
        self.pos = std::cmp::min(self.pos + amt, self.decrypted);
    }
}

/// Writer which encrypts bytes as they are written.
///
/// This acts as a type of BufWriter, but with encryption applied when flushing.
///
/// Because it uses CBC encryption, calling Flush is actually not sufficient to flush the data. You
/// must call `pad_block` before flushing for the last time to ensure that all data is written.
///
/// If this writer is dropped with unencrypted data still in the buffer, it will panic.
pub(crate) struct EncryptingWriter<W: ?Sized + Write> {
    /// Encrypter to use to encrypt the file contents.
    enc: Aes129CbcEnc,
    /// Buffer used for encryption. Any complete blocks in the buffer are encrypted. Any incompletel
    /// blocks, which cannot be flushed, are unencrypted.
    buf: Vec<u8>,
    /// True if the inner writer panics. Used to avoid a double panic or an attempt to flush during
    /// unwinding.
    panicked: bool,
    /// Inner writer. Placed last to allow unsized.
    inner: W,
}

impl<W: Write> EncryptingWriter<W> {
    pub(crate) fn new(inner: W, key: &Key) -> Self {
        Self::new_with_capacity(inner, key, DEFAULT_BUFFER_SIZE)
    }

    pub(crate) fn new_with_capacity(inner: W, key: &Key, capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "must have positive capacity in order to encrypt"
        );
        let capacity = round_up_to_block(capacity as u64);
        assert!(
            capacity <= usize::MAX as u64,
            "Next block size above capacity exceeds uszie max"
        );
        Self {
            enc: Aes129CbcEnc::new((&key.key).into(), (&key.iv).into()),
            buf: Vec::with_capacity(capacity as usize),
            panicked: false,
            inner,
        }
    }
}

impl<W: ?Sized + Write> EncryptingWriter<W> {
    /// Gets the number of bytes that can be written without reallocating.
    fn available(&self) -> usize {
        self.buf.capacity() - self.buf.len()
    }

    /// Get the number of bytes in the buffer that are already encrypted.
    fn encrypted(&self) -> usize {
        round_down_to_block(self.buf.len())
    }

    /// Pad out the buffer to a full multiple of the block size.
    pub fn pad(&mut self) {
        let encrypted = self.encrypted();
        if encrypted == self.buf.len() {
            // If all data is already blocked for encryption, no need to pad.
            return;
        }
        let block_end = encrypted + BLOCK_SIZE;
        let needed_fill = block_end - self.buf.len();
        for _ in 0..needed_fill {
            self.buf.push(needed_fill as u8);
        }
    }

    /// Encrypt any complete blocks
    fn flush_buf(&mut self) -> io::Result<()> {
        /// Helper struct to ensure the buffer is updated after all the writes
        /// are complete. It tracks the number of written bytes and drains them
        /// all from the front of the buffer when dropped.
        struct BufGuard<'a> {
            buffer: &'a mut Vec<u8>,
            /// Number of bytes in the buffer that are encrypted.
            encrypted: usize,
            written: usize,
        }

        impl<'a> BufGuard<'a> {
            fn new(buffer: &'a mut Vec<u8>, encrypted: usize) -> Self {
                Self {
                    buffer,
                    encrypted,
                    written: 0,
                }
            }

            /// The unwritten part of the buffer
            fn remaining(&self) -> &[u8] {
                &self.buffer[self.written..self.encrypted]
            }

            /// Flag some bytes as removed from the front of the buffer
            fn consume(&mut self, amt: usize) {
                self.written += amt;
            }

            /// true if all of the bytes have been written
            fn done(&self) -> bool {
                self.written >= self.encrypted
            }
        }

        impl Drop for BufGuard<'_> {
            fn drop(&mut self) {
                if self.written > 0 {
                    self.buffer.drain(..self.written);
                }
            }
        }

        let encrypted = self.encrypted();
        let mut guard = BufGuard::new(&mut self.buf, encrypted);
        while !guard.done() {
            self.panicked = true;
            let r = self.inner.write(guard.remaining());
            self.panicked = false;

            match r {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write the buffered data",
                    ));
                }
                Ok(n) => guard.consume(n),
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl<W: ?Sized + Write> Write for EncryptingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.available() == 0 {
            self.flush_buf()?;
        }
        // Unlike BufWriter, which can avoid copying large inputs into its internal buffer in some
        // cases, since we have to encrypt the data, there is no case where we want to skip the
        // internal buffer.

        // Remember where we were encrypted up to so we can start encryption from there without
        // double-encrypting anything.
        let mut already_encrypted = self.encrypted();
        // Copy as many bytes as possible into the buffer.
        let to_write = buf.len().min(self.available());
        self.buf.extend_from_slice(&buf[..to_write]);

        // If there are any newly-complete blocks available, encrypt them.
        while let next_block_end = already_encrypted + BLOCK_SIZE
            && next_block_end <= self.buf.len()
        {
            let block = &mut self.buf[already_encrypted..next_block_end];
            self.enc.encrypt_block_mut(block.into());
            already_encrypted = next_block_end;
        }
        Ok(to_write)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_buf().and_then(|_| self.inner.flush())
    }
}

impl<W: ?Sized + Write> Drop for EncryptingWriter<W> {
    fn drop(&mut self) {
        // Don't panic if the inner writer panicked.
        if !self.panicked {
            // dtors should not panic, so we ignore a failed flush
            let r = self.flush_buf();
            if r.is_ok() && !self.buf.is_empty() {
                // If we flushed fully and there was unencrypted data left, then we panic because
                // you're supposed to pad to the block size.
                panic!(
                    "EncryptingWriter had {} unencrypted bytes left when dropped. Be sure to pad \
                    to the block length and flush before dropping to prevent data loss!",
                    self.buf.len()
                )
            }
        }
    }
}
