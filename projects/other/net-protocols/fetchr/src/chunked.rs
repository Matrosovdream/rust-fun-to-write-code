//! `Transfer-Encoding: chunked` (RFC 9112 §7.1), decoded by a `Read`
//! adapter. The server sends the body in pieces, each prefixed with its
//! size in hex, so it can start sending before it knows the total length:
//!
//! ```text
//! 5;note=x\r\n      chunk size in hex; `;extensions` are allowed and ignored
//! hello\r\n         exactly 5 bytes of data, then CRLF
//! 7\r\n
//! , world\r\n
//! 0\r\n             a zero-size chunk ends the body…
//! Expires: 0\r\n    …followed by optional trailer fields…
//! \r\n              …and an empty line
//! ```

use std::io::{self, BufRead, Read};

use crate::{invalid_data, read_line};

/// Wraps the connection's reader. Whoever reads from it sees only the
/// decoded body bytes and then EOF, so it can go straight into `io::copy`.
pub struct ChunkedReader<R> {
    inner: R,
    /// Bytes still to come in the current chunk.
    left: u64,
    done: bool,
}

impl<R: BufRead> ChunkedReader<R> {
    pub fn new(inner: R) -> Self {
        ChunkedReader {
            inner,
            left: 0,
            done: false,
        }
    }
}

impl<R: BufRead> Read for ChunkedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            self.left = parse_size(&read_line(&mut self.inner)?)?;
            if self.left == 0 {
                // The last chunk. We don't use trailers; skip to the empty line.
                while !read_line(&mut self.inner)?.is_empty() {}
                self.done = true;
                return Ok(0);
            }
        }
        // Never read past the end of this chunk. Compare as u64 first so a
        // huge chunk size can't overflow `usize` on a 32-bit machine.
        let max = self.left.min(buf.len() as u64) as usize;
        let n = self.inner.read(&mut buf[..max])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.left -= n as u64;
        if self.left == 0 && !read_line(&mut self.inner)?.is_empty() {
            return Err(invalid_data("chunk data is longer than its size"));
        }
        Ok(n)
    }
}

fn parse_size(line: &str) -> io::Result<u64> {
    let hex = line.split(';').next().unwrap_or_default().trim();
    if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid_data(format!("bad chunk size line {line:?}")));
    }
    u64::from_str_radix(hex, 16).map_err(|_| invalid_data("chunk size overflows u64"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    fn decode(raw: &[u8]) -> io::Result<Vec<u8>> {
        let mut body = Vec::new();
        ChunkedReader::new(raw).read_to_end(&mut body)?;
        Ok(body)
    }

    #[test]
    fn decodes_chunks_with_extensions_and_trailers() {
        let raw = b"5;note=x\r\nhello\r\n7\r\n, world\r\n0\r\nExpires: 0\r\n\r\n";
        assert_eq!(decode(raw).unwrap(), b"hello, world");
        assert_eq!(decode(b"A\nabcdefghij\n0\n\n").unwrap(), b"abcdefghij"); // bare LF, upper-case hex
        assert_eq!(decode(b"0\r\n\r\n").unwrap(), b"");
    }

    #[test]
    fn stops_at_the_end_and_leaves_the_rest_unread() {
        let mut raw: &[u8] = b"3\r\nabc\r\n0\r\n\r\nNEXT";
        let mut body = Vec::new();
        ChunkedReader::new(&mut raw).read_to_end(&mut body).unwrap();
        assert_eq!((body.as_slice(), raw), (&b"abc"[..], &b"NEXT"[..]));
    }

    #[test]
    fn works_one_byte_at_a_time() {
        // A 1-byte BufReader makes every inner read return a single byte,
        // like a slow network.
        let raw: &[u8] = b"5\r\nhello\r\n2\r\n!!\r\n0\r\n\r\n";
        let mut body = Vec::new();
        ChunkedReader::new(BufReader::with_capacity(1, raw))
            .read_to_end(&mut body)
            .unwrap();
        assert_eq!(body, b"hello!!");
    }

    #[test]
    fn rejects_malformed_and_truncated_input() {
        let kind = |raw: &[u8]| decode(raw).unwrap_err().kind();
        assert_eq!(kind(b"zz\r\nhello\r\n"), io::ErrorKind::InvalidData);
        assert_eq!(
            kind(b"+5\r\nhello\r\n0\r\n\r\n"),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            kind(b"3\r\nabcdef\r\n0\r\n\r\n"),
            io::ErrorKind::InvalidData
        );
        assert_eq!(kind(b"11111111111111111\r\n"), io::ErrorKind::InvalidData);
        assert_eq!(kind(b"5\r\nhel"), io::ErrorKind::UnexpectedEof);
        assert_eq!(kind(b"5\r\nhello\r\n"), io::ErrorKind::UnexpectedEof);
    }
}
