//! Minimal binary WebSocket framing (P1.S2 carrier) — hand-rolled sha1 + base64
//! + RFC 6455 frames, zero crates beyond tokio. Adapted from taut's trial `ws.rs`
//! (which is text-only) for **binary** frames: glade carries CBOR, not text.
//! Enough to talk to a browser/Node WebSocket: the upgrade handshake, masked
//! client->server and unmasked server->client binary frames; close surfaced;
//! ping/pong skipped (trusted localhost).
//!
//! An upgrade whose `Origin` is not a loopback origin is refused before the
//! handshake ([`accept`]).

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex;

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let ml = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for i in 0..5 {
        out[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

pub fn accept_key(key: &str) -> String {
    b64_encode(&sha1(format!("{}258EAFA5-E914-47DA-95CA-C5AB0DC85B11", key).as_bytes()))
}

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut s = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        s.push(B64[((n >> 18) & 63) as usize] as char);
        s.push(B64[((n >> 12) & 63) as usize] as char);
        s.push(if chunk.len() > 1 { B64[((n >> 6) & 63) as usize] as char } else { '=' });
        s.push(if chunk.len() > 2 { B64[(n & 63) as usize] as char } else { '=' });
    }
    s
}

pub struct WsReader {
    inner: OwnedReadHalf,
}

#[derive(Clone)]
pub struct WsWriter {
    inner: Arc<Mutex<OwnedWriteHalf>>,
    mask: bool, // clients MUST mask; servers MUST NOT
}

pub enum Msg {
    Binary(Vec<u8>),
    Close,
}

/// The answer to a refused upgrade: no `101`, and the connection closed.
const FORBIDDEN: &[u8] =
    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// Every value of the header `name` in an HTTP head, the name matched without
/// regard to case.
fn header_values<'a>(head: &'a str, name: &str) -> Vec<&'a str> {
    head.lines()
        .filter_map(|line| {
            let (field, value) = line.split_once(':')?;
            field
                .trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim())
        })
        .collect()
}

/// Whether an upgrade request's `Origin` values admit it (plan Step 4.3's
/// question 7 (a), owner 2026-09-24). A browser lets any page open a
/// websocket to 127.0.0.1, and only the `Origin` it sends says whose page it
/// is. No `Origin` is a client that is not a browser. One `Origin` is
/// admitted if it is a loopback origin as a browser writes it: `http` or
/// `https`, a host that is exactly `localhost`, `127.0.0.1` or `[::1]`, and at
/// most a port. Anything else is refused: `null`, another host, a lookalike
/// such as `localhost.evil.example`, a path, a malformed value, two `Origin`s.
fn origin_allowed(origins: &[&str]) -> bool {
    match origins {
        [] => true,
        [origin] => loopback_origin(origin),
        _ => false,
    }
}

fn loopback_origin(origin: &str) -> bool {
    let Some(authority) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    ["localhost", "127.0.0.1", "[::1]"]
        .into_iter()
        .any(|host| match authority.strip_prefix(host) {
            Some("") => true,
            Some(rest) => rest.strip_prefix(':').is_some_and(port),
            None => false,
        })
}

/// A port as an origin writes it: one to five digits, at most 65535.
fn port(digits: &str) -> bool {
    let all_digits = digits.bytes().all(|b| b.is_ascii_digit());
    (1..=5).contains(&digits.len()) && all_digits && digits.parse::<u16>().is_ok()
}

/// Server side: perform the HTTP upgrade handshake. A request whose `Origin`
/// is not loopback is answered `403` and refused before the upgrade
/// ([`origin_allowed`]).
pub async fn accept(stream: tokio::net::TcpStream) -> std::io::Result<(WsReader, WsWriter)> {
    let (mut read, mut write) = stream.into_split();
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if read.read(&mut byte).await? == 0 {
            return Err(eof());
        }
        buf.push(byte[0]);
    }
    let headers = String::from_utf8_lossy(&buf);
    if !origin_allowed(&header_values(&headers, "origin")) {
        write.write_all(FORBIDDEN).await?;
        let refused = "websocket upgrade refused: its Origin is not loopback";
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            refused,
        ));
    }
    let key = headers
        .lines()
        .find_map(|l| {
            let (name, val) = l.split_once(':')?;
            name.trim().eq_ignore_ascii_case("sec-websocket-key").then(|| val.trim().to_string())
        })
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no ws key"))?;
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(&key)
    );
    write.write_all(resp.as_bytes()).await?;
    Ok((WsReader { inner: read }, WsWriter { inner: Arc::new(Mutex::new(write)), mask: false }))
}

/// Client side: send the upgrade request (frames will be masked).
pub async fn connect(host: &str, port: u16) -> std::io::Result<(WsReader, WsWriter)> {
    let stream = tokio::net::TcpStream::connect((host, port)).await?;
    let (mut read, mut write) = stream.into_split();
    let key = b64_encode(&[0u8; 16]);
    let req = format!(
        "GET / HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    write.write_all(req.as_bytes()).await?;
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if read.read(&mut byte).await? == 0 {
            return Err(eof());
        }
        buf.push(byte[0]);
    }
    if !String::from_utf8_lossy(&buf).contains("101") {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "no 101 upgrade"));
    }
    Ok((WsReader { inner: read }, WsWriter { inner: Arc::new(Mutex::new(write)), mask: true }))
}

fn eof() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof in ws")
}

impl WsReader {
    /// Read one data message (binary/text payload), skipping ping/pong.
    pub async fn read(&mut self) -> std::io::Result<Msg> {
        loop {
            let mut h = [0u8; 2];
            self.inner.read_exact(&mut h).await?;
            let opcode = h[0] & 0x0f;
            let masked = h[1] & 0x80 != 0;
            let mut len = (h[1] & 0x7f) as usize;
            if len == 126 {
                let mut e = [0u8; 2];
                self.inner.read_exact(&mut e).await?;
                len = u16::from_be_bytes(e) as usize;
            } else if len == 127 {
                let mut e = [0u8; 8];
                self.inner.read_exact(&mut e).await?;
                len = u64::from_be_bytes(e) as usize;
            }
            let mut mask = [0u8; 4];
            if masked {
                self.inner.read_exact(&mut mask).await?;
            }
            let mut payload = vec![0u8; len];
            self.inner.read_exact(&mut payload).await?;
            if masked {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= mask[i % 4];
                }
            }
            match opcode {
                0x1 | 0x2 => return Ok(Msg::Binary(payload)), // text or binary -> bytes
                0x8 => return Ok(Msg::Close),
                _ => continue, // ping/pong/continuation: skip
            }
        }
    }
}

impl WsWriter {
    /// Send one binary frame (opcode 0x2).
    pub async fn send_binary(&self, payload: &[u8]) -> std::io::Result<()> {
        let n = payload.len();
        let mask_bit = if self.mask { 0x80 } else { 0 };
        let mut frame = vec![0x82u8]; // FIN + binary
        if n < 126 {
            frame.push(mask_bit | n as u8);
        } else if n < 65536 {
            frame.push(mask_bit | 126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        } else {
            frame.push(mask_bit | 127);
            frame.extend_from_slice(&(n as u64).to_be_bytes());
        }
        if self.mask {
            let key = [0x37u8, 0xfa, 0x21, 0x3d];
            frame.extend_from_slice(&key);
            for (i, b) in payload.iter().enumerate() {
                frame.push(b ^ key[i % 4]);
            }
        } else {
            frame.extend_from_slice(payload);
        }
        self.inner.lock().await.write_all(&frame).await
    }
}

// The upgrade's `Origin` check (plan Step 4.3's question 7 (a), owner
// 2026-09-24). A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;

    /// Send `accept` an upgrade request with `extra` header lines, written by
    /// hand over a loopback socket. Returns the status line it answered and
    /// whether it made a session.
    async fn upgrade_with(extra: &[&str]) -> (String, bool) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let node = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            accept(stream).await.is_ok()
        });
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let key = b64_encode(&[0u8; 16]);
        let mut request = format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
        );
        for line in extra {
            request.push_str(line);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        client.write_all(request.as_bytes()).await.unwrap();
        // The node's side ends here, and the connection closes with it.
        let session = node.await.unwrap();
        let mut answer = Vec::new();
        client.read_to_end(&mut answer).await.unwrap();
        let status = String::from_utf8_lossy(&answer)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        (status, session)
    }

    /// A request whose `Origin` is not a loopback origin is answered `403`
    /// before the upgrade, with no `101`, and makes no session: `null`,
    /// another site, lookalikes of the three loopback hosts, other schemes, a
    /// path, a malformed or empty value, and two `Origin` headers. The header
    /// name matches without regard to case. It proves the node's answer to
    /// each header, not what a browser sends.
    #[tokio::test]
    async fn a_non_loopback_origin_is_refused_before_the_upgrade() {
        let refused: &[&[&str]] = &[
            &["Origin: http://evil.example"],
            &["Origin: null"],
            &["Origin: http://localhost.evil.example"],
            &["Origin: http://127.0.0.1.evil.example"],
            &["Origin: http://[::1].evil.example:5173"],
            &["Origin: http://localhost@evil.example"],
            &["Origin: http://evil.example:5173@localhost"],
            &["Origin: http://127.0.0.2:5173"],
            &["Origin: ws://localhost:5173"],
            &["Origin: localhost:5173"],
            &["Origin: http://localhost:5173/"],
            &["Origin: http://localhost:"],
            &["Origin: http://localhost:99999"],
            &["Origin: http://localhost:+5173"],
            &["Origin:"],
            &["ORIGIN: http://evil.example"],
            &["origin: http://evil.example"],
            &[
                "Origin: http://localhost:5173",
                "Origin: http://evil.example",
            ],
        ];
        let mut wrong = Vec::new();
        for lines in refused {
            let (status, session) = upgrade_with(lines).await;
            if status != "HTTP/1.1 403 Forbidden" || session {
                wrong.push(format!("{lines:?}: {status}; session made: {session}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "not refused before the upgrade:\n{}",
            wrong.join("\n")
        );
    }

    /// The clients the check must keep (plan Step 4.3's `Origin` table): no
    /// `Origin` (client-rs, Node's WebSocket, this file's `connect`), the
    /// Gyld desk in dev and built, a second desk on a moved port, and
    /// glade/demo; and each loopback host, `https`, and the header name in
    /// lower case. Each is upgraded. It passed before the check and after: it
    /// guards the check's scope.
    #[tokio::test]
    async fn no_origin_or_a_loopback_origin_is_upgraded() {
        let upgraded: &[&[&str]] = &[
            &[],
            &["Origin: http://localhost:5173"],
            &["Origin: http://127.0.0.1:8080"],
            &["Origin: http://localhost:5180"],
            &["Origin: http://localhost:5175"],
            &["origin: http://[::1]:5173"],
            &["Origin: https://localhost"],
            &["Origin: http://127.0.0.1"],
        ];
        let mut wrong = Vec::new();
        for lines in upgraded {
            let (status, session) = upgrade_with(lines).await;
            if status != "HTTP/1.1 101 Switching Protocols" || !session {
                wrong.push(format!("{lines:?}: {status}; session made: {session}"));
            }
        }
        assert!(wrong.is_empty(), "not upgraded:\n{}", wrong.join("\n"));
    }
}
