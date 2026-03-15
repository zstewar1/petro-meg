use std::io::{self, Write};

/// Counts the number of bytes written.
pub(super) struct CountingWriter<W> {
    inner: W,
    /// A mark that can be used to count bytes since a specific point.
    mark: u64,
    /// Counts the total number of bytes written.
    written: u64,
}

impl<W> CountingWriter<W> {
    /// Create a new zeroed counting writer.
    pub(super) fn new(inner: W) -> Self {
        Self {
            inner,
            mark: 0,
            written: 0,
        }
    }

    /// Move the mark to the current position.
    pub(super) fn move_mark(&mut self) {
        self.mark = self.written;
    }

    /// Get the number of bytes written since the mark was moved.
    pub(super) fn written_since_mark(&mut self) -> u64 {
        self.written - self.mark
    }

    /// Get the total bytes written.
    pub(super) fn total_written(&self) -> u64 {
        self.written
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
