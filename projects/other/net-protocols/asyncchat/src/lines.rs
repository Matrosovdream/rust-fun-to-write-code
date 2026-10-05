//! A bounded, cancel-safe line reader.
//!
//! Why not tokio's `read_line`? Two reasons:
//! - It's unbounded: a client that never sends `\n` grows the buffer forever.
//! - It's not cancel-safe: if it loses a `select!` race halfway through a
//!   line, the bytes it already read are lost.

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    Text(String),
    TooLong,
}

/// Splits a byte stream into lines of at most `max` bytes (not counting the
/// `\n`; a `\r` before it is dropped).
///
/// All progress lives in the struct, not in the future: the partial line in
/// `line`, and whether we are throwing away an over-long one in `too_long`.
/// That is what makes [`next_line`](Self::next_line) cancel-safe.
pub struct LineReader<R> {
    inner: BufReader<R>,
    line: Vec<u8>,
    too_long: bool,
    max: usize,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub fn new(reader: R, max: usize) -> Self {
        LineReader {
            inner: BufReader::new(reader),
            line: Vec::new(),
            too_long: false,
            max,
        }
    }

    /// Returns the next line, or `None` at EOF.
    ///
    /// Cancel safety: the only `.await` is `fill_buf`, which consumes
    /// nothing. Bytes leave the `BufReader` (`consume`) in the same
    /// synchronous step that records them in `self`. So if a `select!`
    /// drops this future at the `.await`, nothing is lost: the next call
    /// picks up exactly where this one stopped.
    pub async fn next_line(&mut self) -> io::Result<Option<Line>> {
        loop {
            let buf = self.inner.fill_buf().await?;
            if buf.is_empty() {
                // EOF. A last line without `\n` still counts.
                if self.line.is_empty() && !self.too_long {
                    return Ok(None);
                }
                return Ok(Some(self.finish()));
            }
            let newline = buf.iter().position(|&b| b == b'\n');
            let chunk = &buf[..newline.unwrap_or(buf.len())];
            // Past the limit we stop storing bytes but keep reading up to
            // the `\n`: memory stays bounded, and the next line starts clean.
            if self.line.len() + chunk.len() > self.max {
                self.too_long = true;
                self.line.clear();
            } else if !self.too_long {
                self.line.extend_from_slice(chunk);
            }
            let used = chunk.len() + usize::from(newline.is_some());
            self.inner.consume(used);
            if newline.is_some() {
                return Ok(Some(self.finish()));
            }
        }
    }

    fn finish(&mut self) -> Line {
        let line = std::mem::take(&mut self.line);
        if std::mem::take(&mut self.too_long) {
            return Line::TooLong;
        }
        let line = line.strip_suffix(b"\r").unwrap_or(&line);
        // Invalid UTF-8 is replaced with U+FFFD rather than rejected.
        Line::Text(String::from_utf8_lossy(line).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    fn text(s: &str) -> Option<Line> {
        Some(Line::Text(s.to_string()))
    }

    #[tokio::test]
    async fn splits_lines_and_bounds_them() {
        let long = "x".repeat(20);
        let input = format!("hi\r\nplain\n{long}\nafter\n\nno newline");
        // A 4-byte buffer makes the reader see the input in small pieces.
        let reader = tokio::io::BufReader::with_capacity(4, input.as_bytes());
        let mut lines = LineReader::new(reader, 10);
        assert_eq!(lines.next_line().await.unwrap(), text("hi"));
        assert_eq!(lines.next_line().await.unwrap(), text("plain"));
        assert_eq!(lines.next_line().await.unwrap(), Some(Line::TooLong));
        assert_eq!(lines.next_line().await.unwrap(), text("after"));
        assert_eq!(lines.next_line().await.unwrap(), text(""));
        assert_eq!(lines.next_line().await.unwrap(), text("no newline"));
        assert_eq!(lines.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn the_limit_counts_every_byte_before_the_newline() {
        // A `\r` counts too, as in linechat: 9 bytes + `\r` fits in 10, 10 + `\r` doesn't.
        let input = b"0123456789\n012345678\r\n0123456789\r\n\xff\xfe\n";
        let mut lines = LineReader::new(&input[..], 10);
        assert_eq!(lines.next_line().await.unwrap(), text("0123456789"));
        assert_eq!(lines.next_line().await.unwrap(), text("012345678"));
        assert_eq!(lines.next_line().await.unwrap(), Some(Line::TooLong));
        assert_eq!(lines.next_line().await.unwrap(), text("\u{fffd}\u{fffd}"));
    }

    #[tokio::test]
    async fn a_cancelled_read_loses_nothing() {
        let (mut client, server) = tokio::io::duplex(64);
        let mut lines = LineReader::new(server, 100);

        client.write_all(b"hel").await.unwrap();
        // Simulate losing a select! race: the read is dropped mid-line.
        let cancelled = tokio::time::timeout(Duration::from_millis(20), lines.next_line()).await;
        assert!(cancelled.is_err());

        client.write_all(b"lo\n").await.unwrap();
        assert_eq!(lines.next_line().await.unwrap(), text("hello"));
    }
}
