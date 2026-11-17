//! Loopback-only media bridge. MPD receives no server address or credentials.
use super::{host::Binding as Connection, UNAVAILABLE};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
};
pub fn bind() -> Result<(std::net::TcpListener, u16), String> {
    let listener =
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|_| UNAVAILABLE)?;
    listener.set_nonblocking(true).map_err(|_| UNAVAILABLE)?;
    let port = listener.local_addr().map_err(|_| UNAVAILABLE)?.port();
    Ok((listener, port))
}
// One process-wide physical async stream budget survives reconnects. A retiring
// connection cancels each async socket/body future; this does not claim to cancel
// platform DNS or other in-flight OS operations internal to the HTTP runtime.
fn slots() -> &'static Arc<Semaphore> {
    static SLOTS: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(6)))
}
pub(super) fn start(listener: TcpListener, connection: Weak<Connection>, mut stop: tokio::sync::watch::Receiver<bool>) {
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                _ = stop.wait_for(|stopped| *stopped) => break,
                result = listener.accept() => result
            };
            let Ok((stream, peer)) = accepted else { break };
            if !peer.ip().is_loopback() { continue; }
            let Some(connection) = connection.upgrade().filter(|c| c.active()) else { break };
            let Ok(slot) = slots().clone().try_acquire_owned() else { continue };
            let mut stop = connection.stop.subscribe();
            tokio::spawn(async move {
                let _slot = slot;
                tokio::select! {
                    biased;
                    _ = stop.wait_for(|stopped| *stopped) => {},
                    _ = serve(stream, &connection) => {}
                }
            });
        }
    });
}
fn range(value: &str) -> bool {
    let Some(value) = value.strip_prefix("bytes=") else {
        return false;
    };
    if value.len() > 48 {
        return false;
    }
    let Some((start, end)) = value.split_once('-') else {
        return false;
    };
    (!start.is_empty() || !end.is_empty())
        && start.bytes().all(|b| b.is_ascii_digit())
        && end.bytes().all(|b| b.is_ascii_digit())
}
fn parse<'a>(request: &'a str, secret: &str) -> Option<(&'a str, Option<&'a str>, bool)> {
    let mut lines = request.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?;
    let target = first.next()?;
    let version = first.next()?;
    if !["GET", "HEAD"].contains(&method)
        || !["HTTP/1.0", "HTTP/1.1"].contains(&version)
        || first.next().is_some()
    {
        return None;
    }
    let song = target
        .strip_prefix("/plex/")?
        .strip_prefix(secret)?
        .strip_prefix('/')?;
    if song.len()!=48 || !song.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)) {
        return None;
    }
    let mut selected = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("range") {
            let value = value.trim();
            if selected.is_some() || !range(value) {
                return None;
            }
            selected = Some(value);
        }
    }
    Some((song, selected, method == "HEAD"))
}
async fn reject(stream: &mut TcpStream, status: &str) -> Result<(), String> {
    stream
        .write_all(
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .map_err(|_| UNAVAILABLE.into())
}
async fn serve(mut stream: TcpStream, connection: &Connection) -> Result<(), String> {
    let mut buffer = Vec::new();
    let request = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let mut chunk = [0u8; 1024];
            let count = stream.read(&mut chunk).await.map_err(|_| UNAVAILABLE)?;
            if count == 0 {
                return Err(UNAVAILABLE.to_string());
            }
            buffer.extend_from_slice(&chunk[..count]);
            if let Some(end) = buffer.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                if end + 4 > 8192 {
                    return Err(UNAVAILABLE.to_string());
                }
                buffer.truncate(end + 4);
                break;
            }
            if buffer.len() >= 8192 {
                return Err(UNAVAILABLE.to_string());
            }
        }
        String::from_utf8(buffer).map_err(|_| UNAVAILABLE.to_string())
    })
    .await
    .map_err(|_| UNAVAILABLE)??;
    let Some((song, range, head)) = parse(&request, &connection.secret) else {
        return reject(&mut stream, "404 Not Found").await;
    };
    if !connection.active() {
        return reject(&mut stream, "410 Gone").await;
    }
    let Some(part)=connection.part(song)? else{return reject(&mut stream,"404 Not Found").await;};
    let mut response=match connection.client.media(&connection.stream_http,&part.source,range).await {
        Ok(response)=>response,Err(_)=>return reject(&mut stream,"502 Bad Gateway").await,
    };
    if !connection.active() {
        return reject(&mut stream, "410 Gone").await;
    }
    let status = match response.status().as_u16() {
        200 => "200 OK",
        206 => "206 Partial Content",
        416 => "416 Range Not Satisfiable",
        _ => return reject(&mut stream, "502 Bad Gateway").await,
    };
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream");
    if content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("html")
    {
        return reject(&mut stream, "502 Bad Gateway").await;
    }
    let mut header = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
    for name in [
        reqwest::header::CONTENT_TYPE,
        reqwest::header::CONTENT_RANGE,
        reqwest::header::ACCEPT_RANGES,
    ] {
        if let Some(value) = response
            .headers()
            .get(&name)
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.len() <= 192 && !v.chars().any(char::is_control))
        {
            header.push_str(&format!("{}: {}\r\n", name.as_str(), value));
        }
    }
    if let Some(length) = response.content_length() {
        header.push_str(&format!("Content-Length: {length}\r\n"));
    }
    header.push_str("\r\n");
    tokio::time::timeout(Duration::from_secs(20), stream.write_all(header.as_bytes()))
        .await
        .map_err(|_| UNAVAILABLE)?
        .map_err(|_| UNAVAILABLE)?;
    if !head {
        while let Some(chunk) = response.chunk().await.map_err(|_| UNAVAILABLE)? {
            if !connection.active() {
                return Err(UNAVAILABLE.into());
            }
            tokio::time::timeout(Duration::from_secs(20), stream.write_all(&chunk))
                .await
                .map_err(|_| UNAVAILABLE)?
                .map_err(|_| UNAVAILABLE)?;
        }
    }
    Ok(())
}
