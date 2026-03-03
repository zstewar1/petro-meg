use std::io::{self, BufRead, Read};

use aes::cipher::{BlockDecryptMut as _, KeyIvInit as _};

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// AES 128 Key and Initial Vector used to decrypt encrypted V3 files.
#[derive(Debug, Clone)]
pub struct Key {
    /// 16 bit AES key.
    key: [u8; 16],
    /// Initial vector used for decryption.
    iv: [u8; 16],
}

/// 128 bit AES block size.
const BLOCK_SIZE: usize = 16;

/// Rounds the given value up to the next multiple of the block size. Panics on overflow.
pub(crate) fn round_up_to_block(n: u64) -> u64 {
    n.checked_next_multiple_of(BLOCK_SIZE as u64)
        .expect("Rounding up to a multiple of the block size overflowed")
}

/// Default buffer size for the decrypting reader.
const DEFAULT_BUFFER_SIZE: usize = BLOCK_SIZE * 256;

/// Reader which wraps another reader to make it perform AES 128 CBC decryption on a stream.
///
/// The underlying stream must be a multiple of the block size (128 bits/16 bytes), otherwise an
/// `UnexpectedEof` error will be produced when the end of the stream is reached outside of a
pub struct DecryptingReader<R: ?Sized> {
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
        let capacity = round_up_to_block(capacity as u64);
        assert!(capacity <= usize::MAX as u64, "Next block size above capacity exceeds uszie max");
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
