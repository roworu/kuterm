//! json-rpc messages framed like lsp: a `Content-Length` header, an empty line, the json

use serde_json_lenient::Value;

// a plugin sending more than this in one message is cut off
const MAX_MESSAGE: usize = 16 << 20;

/// `message` with its header, ready to write
pub fn encode(message: &Value) -> Vec<u8> {
    let body = serde_json_lenient::to_string(message).unwrap_or_default();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// messages read so far, split as their headers say
#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
}

/// what reading a stream gave
#[derive(Debug, PartialEq)]
pub enum Decoded {
    Message(Value),
    /// a message that is no json, or a header that makes no sense, with why
    Broken(String),
}

impl Decoder {
    /// add read bytes and take every message they complete
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Decoded> {
        self.buffer.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(end) = find(&self.buffer, b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&self.buffer[..end]).into_owned();
            let length = header.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            });
            let Some(length) = length.filter(|length| *length <= MAX_MESSAGE) else {
                // nothing tells where the next message starts, so everything read goes
                self.buffer.clear();
                out.push(Decoded::Broken(format!("bad header {header:?}")));
                break;
            };
            let start = end + 4;
            if self.buffer.len() < start + length {
                break;
            }
            let body: Vec<u8> = self.buffer.drain(..start + length).skip(start).collect();
            out.push(match serde_json_lenient::from_slice(&body) {
                Ok(value) => Decoded::Message(value),
                Err(error) => Decoded::Broken(error.to_string()),
            });
        }
        out
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use serde_json_lenient::json;

    use super::{Decoded, Decoder, encode};

    #[test]
    fn messages_round_trip_whole_and_in_pieces() {
        let one = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"});
        let two = json!({"jsonrpc": "2.0", "method": "event", "params": {"text": "ü"}});
        let mut bytes = encode(&one);
        bytes.extend(encode(&two));
        assert!(String::from_utf8_lossy(&bytes).starts_with("Content-Length: "));
        let mut decoder = Decoder::default();
        assert_eq!(
            decoder.feed(&bytes),
            [Decoded::Message(one.clone()), Decoded::Message(two.clone())]
        );
        // a byte at a time, split inside the header and the multibyte body
        let mut got = Vec::new();
        for byte in &bytes {
            got.extend(decoder.feed(std::slice::from_ref(byte)));
        }
        assert_eq!(got, [Decoded::Message(one), Decoded::Message(two)]);
    }

    #[test]
    fn broken_input_is_told_and_skipped() {
        let mut decoder = Decoder::default();
        let got = decoder.feed(b"Content-Length: 3\r\n\r\n{x}");
        assert!(matches!(got[..], [Decoded::Broken(_)]));
        let got = decoder.feed(b"Nonsense\r\n\r\n");
        assert!(matches!(got[..], [Decoded::Broken(_)]));
        // after both, a good message reads fine
        let good = json!({"id": 2});
        assert_eq!(decoder.feed(&encode(&good)), [Decoded::Message(good)]);
        // other headers are allowed next to the length
        let got = decoder.feed(b"Content-Type: x\r\ncontent-length: 2\r\n\r\n{}");
        assert_eq!(got, [Decoded::Message(json!({}))]);
    }
}
