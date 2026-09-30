//! Minimal binary WebSocket client carrier — the client half of the node's
//! `ws.rs` (P1.S2), ported verbatim in behavior: the RFC 6455 upgrade request,
//! masked client->server binary frames, and reading unmasked server->client
//! binary frames (ping/pong skipped, close surfaced). Glade carries CBOR, not
//! text, so frames are binary (opcode 0x2). Zero crates beyond tokio — the same
//! trusted-localhost posture the node uses.

use std::sync::Arc;

use glade_wire::frame::frame_len;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex;

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
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
}

pub enum Msg {
    Binary(Vec<u8>),
    Close,
}

fn eof() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof in ws")
}

/// Client side: perform the HTTP upgrade handshake (frames will be masked).
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
    Ok((WsReader { inner: read }, WsWriter { inner: Arc::new(Mutex::new(write)) }))
}

impl WsReader {
    /// Read one data message (binary/text payload), skipping ping/pong. A
    /// frame whose header claims more than glade-wire's `MAX_FRAME_BYTES` is
    /// refused before its payload, as `InvalidData`, as the node refuses one
    /// (F15, TautCheckedDecode.md CD-G3 item 3).
    pub async fn read(&mut self) -> std::io::Result<Msg> {
        loop {
            let mut h = [0u8; 2];
            self.inner.read_exact(&mut h).await?;
            let opcode = h[0] & 0x0f;
            let masked = h[1] & 0x80 != 0;
            let mut len = u64::from(h[1] & 0x7f);
            if len == 126 {
                let mut e = [0u8; 2];
                self.inner.read_exact(&mut e).await?;
                len = u16::from_be_bytes(e).into();
            } else if len == 127 {
                let mut e = [0u8; 8];
                self.inner.read_exact(&mut e).await?;
                len = u64::from_be_bytes(e);
            }
            let len = frame_len(len)?;
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
                0x1 | 0x2 => return Ok(Msg::Binary(payload)),
                0x8 => return Ok(Msg::Close),
                _ => continue, // ping/pong/continuation: skip
            }
        }
    }
}

impl WsWriter {
    /// Send one masked binary frame (opcode 0x2) — clients MUST mask.
    pub async fn send_binary(&self, payload: &[u8]) -> std::io::Result<()> {
        let n = payload.len();
        let mut frame = vec![0x82u8]; // FIN + binary
        if n < 126 {
            frame.push(0x80 | n as u8);
        } else if n < 65536 {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(n as u64).to_be_bytes());
        }
        let key = [0x37u8, 0xfa, 0x21, 0x3d];
        frame.extend_from_slice(&key);
        for (i, b) in payload.iter().enumerate() {
            frame.push(b ^ key[i % 4]);
        }
        self.inner.lock().await.write_all(&frame).await
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use glade_wire::frame::MAX_FRAME_BYTES;
    use std::io;
    use std::time::Duration;
    use tokio::net::TcpListener;

    /// A node on loopback that answers the upgrade, writes `bytes` and holds
    /// the socket open until the client goes.
    async fn serving(bytes: Vec<u8>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(socket.read_u8().await.unwrap());
            }
            socket
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n")
                .await
                .unwrap();
            let _ = socket.write_all(&bytes).await;
            let _ = socket.read(&mut [0; 1]).await;
        });
        port
    }

    /// An unmasked binary frame's header, as the node writes it, claiming
    /// `len` bytes.
    fn header(len: u64) -> Vec<u8> {
        [&[0x82, 127][..], &len.to_be_bytes()].concat()
    }

    /// F15 in client-rs (TautCheckedDecode.md CD-G3 item 3): a header that
    /// claims more than `MAX_FRAME_BYTES`, by one or up to all of `u64`, is
    /// refused as `InvalidData` before its payload, where the reader
    /// allocated what it claimed and waited for it. A frame of exactly the
    /// limit is read whole.
    #[tokio::test]
    async fn a_header_over_the_frame_limit_is_refused_before_its_payload() {
        for claimed in [MAX_FRAME_BYTES as u64 + 1, u64::MAX] {
            let port = serving(header(claimed)).await;
            let (mut reader, _writer) = connect("127.0.0.1", port).await.unwrap();
            let read = tokio::time::timeout(Duration::from_secs(5), reader.read()).await;
            let Ok(Err(refused)) = read else {
                panic!("a header claiming {claimed} bytes was not refused before its payload");
            };
            assert_eq!(refused.kind(), io::ErrorKind::InvalidData, "{claimed}");
            let said = format!("bad frame: {claimed} bytes, over the limit of {MAX_FRAME_BYTES}");
            assert_eq!(refused.to_string(), said);
        }
        let at_limit = [header(MAX_FRAME_BYTES as u64), vec![7; MAX_FRAME_BYTES]].concat();
        let port = serving(at_limit).await;
        let (mut reader, _writer) = connect("127.0.0.1", port).await.unwrap();
        let Msg::Binary(frame) = reader.read().await.unwrap() else {
            panic!("the frame at the limit was not read");
        };
        assert_eq!(frame.len(), MAX_FRAME_BYTES);
    }
}
