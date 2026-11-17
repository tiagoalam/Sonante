//! Metrics-only loopback request. Absolute per-I/O deadline and byte budgets
//! include the handshake, fragmented frames and ping traffic. No PCM transport.
use serde_json::Value;
use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    time::{Duration, Instant},
};
use tungstenite::Message;
const MAX_FRAME: usize = 16 * 1024;
const MAX_READ: usize = 64 * 1024;
const MAX_WRITE: usize = 8 * 1024;
#[cfg(not(test))]
const BUDGET: Duration = Duration::from_secs(2);
#[cfg(test)]
const BUDGET: Duration = Duration::from_millis(200);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    Connect,
    Ownership,
    Timeout,
    Transport,
    Protocol,
    Limit,
}
struct Bounded {
    stream: TcpStream,
    deadline: Instant,
    read: usize,
    written: usize,
}
impl Bounded {
    fn remaining(&self) -> io::Result<Duration> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err(io::ErrorKind::TimedOut.into())
        } else {
            Ok(left)
        }
    }
}
impl Read for Bounded {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let left = self.remaining()?;
        let limit = MAX_READ.saturating_sub(self.read).min(out.len());
        if limit == 0 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        let n = self.stream.read(&mut out[..limit])?;
        self.read += n;
        self.remaining()?;
        Ok(n)
    }
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let left = self.remaining()?;
        let limit = MAX_WRITE.saturating_sub(self.written).min(bytes.len());
        if limit == 0 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.stream.set_write_timeout(Some(left))?;
        let n = self.stream.write(&bytes[..limit])?;
        self.written += n;
        self.remaining()?;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        let left = self.remaining()?;
        self.stream.set_write_timeout(Some(left))?;
        self.stream.flush()?;
        self.remaining()?;
        Ok(())
    }
}
fn classify(error: tungstenite::Error) -> Error {
    match error {
        tungstenite::Error::Io(e) => match e.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Error::Timeout,
            io::ErrorKind::InvalidData => Error::Limit,
            _ => Error::Transport,
        },
        tungstenite::Error::Capacity(_) => Error::Limit,
        _ => Error::Protocol,
    }
}
pub(crate) fn request(
    port: u16,
    command: Value,
    verify: impl FnOnce(&TcpStream) -> Result<(), ()>,
) -> Result<Value, Error> {
    let encoded = command.to_string();
    if encoded.len() > 4096 || port == 0 {
        return Err(Error::Limit);
    }
    let deadline = Instant::now() + BUDGET;
    let stream = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
        BUDGET.min(Duration::from_millis(500)),
    )
    .map_err(|_| Error::Connect)?;
    // Own process/tuple proof BEFORE handshake or any private binding/token bytes.
    verify(&stream).map_err(|_| Error::Ownership)?;
    stream.set_nodelay(true).map_err(|_| Error::Transport)?;
    let stream = Bounded {
        stream,
        deadline,
        read: 0,
        written: 0,
    };
    let limits = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    let (mut socket, _) = tungstenite::client::client_with_config(
        format!("ws://127.0.0.1:{port}/"),
        stream,
        Some(limits),
    )
    .map_err(|e| match e {
        tungstenite::HandshakeError::Failure(e) => classify(e),
        _ => Error::Timeout,
    })?;
    socket
        .send(Message::Text(encoded.into()))
        .map_err(classify)?;
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                if text.len() > MAX_FRAME {
                    return Err(Error::Limit);
                }
                let value = serde_json::from_str(&text).map_err(|_| Error::Protocol);
                let _ = socket.close(None);
                return value;
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            Ok(_) => return Err(Error::Protocol),
            Err(e) => return Err(classify(e)),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::TcpListener,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };
    #[test]
    fn peer_ownership_is_verified_before_any_handshake_or_binding_is_sent() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            bytes
        });
        assert_eq!(
            request(
                port,
                serde_json::json!({"SonanteAnalyzer":{"nonce":"PRIVATE"}}),
                |_| Err(())
            ),
            Err(Error::Ownership)
        );
        assert!(peer.join().unwrap().is_empty());
    }
    #[test]
    fn actual_bounded_websocket_roundtrip_uses_only_one_owned_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let verified = Arc::new(AtomicBool::new(false));
        let proof = verified.clone();
        let peer = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            let text = socket.read().unwrap().into_text().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap(),
                serde_json::json!({"SonanteAnalyzer":{"action":"read"}})
            );
            socket.send(Message::Text("{\"own\":true}".into())).unwrap();
        });
        let value = request(
            port,
            serde_json::json!({"SonanteAnalyzer":{"action":"read"}}),
            move |_| {
                proof.store(true, Ordering::Release);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(value, serde_json::json!({"own":true}));
        assert!(verified.load(Ordering::Acquire));
        peer.join().unwrap();
    }
    #[test]
    fn a_slow_fragment_cannot_extend_the_physical_request_by_resetting_socket_timeouts() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            let _ = socket.read().unwrap();
            socket.get_mut().write_all(&[0x81, 100]).unwrap();
            for _ in 0..100 {
                if socket.get_mut().write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let at = Instant::now();
        let error =
            request(port, serde_json::json!({"SonanteAnalyzer":{}}), |_| Ok(())).unwrap_err();
        assert!(matches!(error, Error::Timeout | Error::Transport));
        assert!(at.elapsed() < Duration::from_secs(1));
        peer.join().unwrap();
    }
    #[test]
    fn declared_large_metrics_frame_is_refused_before_payload_allocation() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            let _ = socket.read().unwrap();
            let mut bytes = vec![0x81, 127];
            bytes.extend((10u64 * 1024 * 1024).to_be_bytes());
            let _ = socket.get_mut().write_all(&bytes);
        });
        assert_eq!(
            request(port, serde_json::json!({"SonanteAnalyzer":{}}), |_| Ok(())),
            Err(Error::Limit)
        );
        peer.join().unwrap();
    }
}
