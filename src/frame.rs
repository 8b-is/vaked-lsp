// frame.rs — minimal LSP stdio framing (Content-Length headers).
// the constellation's own wire: one door, many lanes — framing is the door.
//
// Hot-path rules (the IPC is the door, the door must be cheap):
//   * encode: zero heap formatting — the length is written digit by digit
//   * read:   chunked (8 KiB) reads, one reusable carry buffer per stream;
//             never one syscall per byte, never a String for the header

use std::io::Read;

const HDR: &[u8] = b"Content-Length: ";
const SEP: &[u8] = b"\r\n\r\n";

/// Index of the first byte of the header/body separator, if present.
pub fn header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == SEP)
}

/// Local resource policy for all framed readers, not protocol-wide limits.
pub const MAX_HEADER_BYTES: usize = 65536;
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Parse one complete decimal length field, rejecting ambiguity and overflow.
/// Applying the shared limits here also protects the async sub-server reader.
pub fn content_length(header: &[u8]) -> Option<usize> {
    if header.len() > MAX_HEADER_BYTES {
        return None;
    }
    let mut length = None;
    for line in header.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(colon) = line.iter().position(|&b| b == b':') else { continue };
        if !line[..colon].eq_ignore_ascii_case(b"Content-Length") {
            continue;
        }
        if length.is_some() {
            return None;
        }
        let value = line[colon + 1..].trim_ascii();
        if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let mut n = 0usize;
        for digit in value {
            n = n.checked_mul(10)?.checked_add((digit - b'0') as usize)?;
        }
        if n > MAX_BODY_BYTES {
            return None;
        }
        length = Some(n);
    }
    length
}

/// Encode a JSON-RPC payload into LSP stdio framing (one allocation).
pub fn encode_frame(payload: &str) -> Vec<u8> {
    encode_frame_bytes(payload.as_bytes())
}

/// Encode raw bytes — skip the String round-trip when the caller already
/// serialized to a byte buffer (serde_json::to_vec).
pub fn encode_frame_bytes(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 64);
    out.extend_from_slice(HDR);
    let mut n = payload.len();
    if n == 0 {
        out.push(b'0');
    } else {
        let mut digits = [0u8; 20];
        let mut i = digits.len();
        while n > 0 {
            i -= 1;
            digits[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        out.extend_from_slice(&digits[i..]);
    }
    out.extend_from_slice(SEP);
    out.extend_from_slice(payload);
    out
}

/// A stateful framed reader. One carry buffer is reused across frames, so
/// the stdio hot path allocates only the body (once per message) — and a
/// body arriving together with the next frame's header is never lost.
pub struct FrameReader<R> {
    reader: R,
    buf: Vec<u8>,
}

impl<R: Read> FrameReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buf: Vec::with_capacity(8192),
        }
    }

    /// Pull more bytes from the underlying stream into the carry buffer.
    fn fill(&mut self) -> std::io::Result<usize> {
        let mut chunk = [0u8; 8192];
        let n = self.reader.read(&mut chunk)?;
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(n)
    }

    /// Extract the next framed body as raw bytes.
    fn next_frame(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            if let Some(pos) = header_end(&self.buf) {
                let len = content_length(&self.buf[..pos]).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "missing Content-Length")
                })?;
                let body_start = pos + SEP.len();
                while self.buf.len() < body_start + len {
                    if self.fill()? == 0 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "eof in body",
                        ));
                    }
                }
                let body = self.buf[body_start..body_start + len].to_vec();
                self.buf.drain(..body_start + len);
                return Ok(body);
            }
            if self.buf.len() > MAX_HEADER_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "header too long",
                ));
            }
            if self.fill()? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "eof in header",
                ));
            }
        }
    }

    /// Read one framed payload as UTF-8.
    pub fn read_frame(&mut self) -> std::io::Result<String> {
        String::from_utf8(self.next_frame()?)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Read one framed payload as raw bytes (binary-safe lanes).
    pub fn read_frame_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        self.next_frame()
    }
}

/// One-shot framed read for tests and non-loop callers.
pub fn read_frame<R: Read>(reader: &mut R) -> std::io::Result<String> {
    FrameReader::new(&mut *reader).read_frame()
}

/// A request/response pair for one forwarded method call.
pub fn jsonrpc_request(id: u64, method: &str, params: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// A notification — no id, no response expected.
pub fn jsonrpc_notify(method: &str, params: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let payload = r#"{"jsonrpc":"2.0","id":1,"method":"textDocument/hover","params":{}}"#;
        let framed = encode_frame(payload);
        let mut cursor = std::io::Cursor::new(framed);
        let back = read_frame(&mut cursor).unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn framing_is_length_preserving_with_utf8() {
        // non-ascii inside a frame must survive (the céllar of the garden writes hungarian)
        let payload = "{\"text\":\"kölbeu · φ · ⟦⟧\"}";
        let framed = encode_frame(payload);
        let mut cursor = std::io::Cursor::new(framed);
        let back = read_frame(&mut cursor).unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn encode_is_exact() {
        assert_eq!(encode_frame("{}"), b"Content-Length: 2\r\n\r\n{}");
        assert_eq!(encode_frame(""), b"Content-Length: 0\r\n\r\n");
    }

    #[test]
    fn two_frames_one_buffer_survive() {
        // a single read may deliver several frames — none may be lost
        let a = r#"{"id":1}"#;
        let b = r#"{"id":2,"text":"kölbeu"}"#;
        let mut wire = encode_frame(a);
        wire.extend_from_slice(&encode_frame(b));
        let mut reader = FrameReader::new(std::io::Cursor::new(wire));
        assert_eq!(reader.read_frame().unwrap(), a);
        assert_eq!(reader.read_frame().unwrap(), b);
    }

    #[test]
    fn content_length_scans_raw_header() {
        assert_eq!(content_length(b"Content-Length: 42\r\nX: 1\r\n"), Some(42));
        assert_eq!(content_length(b"x\r\n\r\n"), None);
    }
    #[test]
    fn rejects_invalid_lengths() {
        for header in [
            "Content-Length: 18446744073709551615999",
            "Content-Length: 2garbage",
            "X-Content-Length: 2",
            "Content-Length: 2\r\nContent-Length: 3",
            "Content-Length: -2",
            "Content-Length: 16777217",
        ] {
            assert_eq!(content_length(header.as_bytes()), None, "{header}");
        }
        assert_eq!(content_length(b"content-length: 2"), Some(2));
    }

    #[test]
    fn excessive_frames_fail_before_body_read() {
        for header in ["Content-Length: 18446744073709551615999".to_owned(),
                       "Content-Length: 16777217".to_owned(),
                       format!("X: {}\r\nContent-Length: 0", "a".repeat(65536))] {
            let bytes = format!("{header}\r\n\r\n").into_bytes();
            let err = FrameReader::new(std::io::Cursor::new(bytes)).read_frame().unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        }
    }

}
