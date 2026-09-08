// lsp_frame.rs — minimal LSP stdio framing (Content-Length headers).
// the constellation's own wire: one door, many lanes — framing is the door.

use std::io::{BufRead, BufReader, Read, Write};

/// Encode a JSON-RPC payload into LSP stdio framing.
pub fn encode_frame(payload: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 64);
    out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", payload.as_bytes().len()).as_bytes());
    out.extend_from_slice(payload.as_bytes());
    out
}

/// Read one framed JSON-RPC payload from a reader (blocking).
pub fn read_frame<R: Read>(reader: &mut R) -> std::io::Result<String> {
    let mut buf = [0u8; 1];
    let mut header = Vec::new();
    // read until the blank line
    loop {
        reader.read_exact(&mut buf)?;
        header.push(buf[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
        if header.len() > 65536 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "header too long"));
        }
    }
    let head = String::from_utf8_lossy(&header);
    let len = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    String::from_utf8(body).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// A request/response pair for one forwarded method call.
pub fn jsonrpc_request(id: u64, method: &str, params: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
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
}