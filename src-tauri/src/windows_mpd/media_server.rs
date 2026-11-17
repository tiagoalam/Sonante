//! Private container-byte transport, not PCM/DSP or an input source resolver.
//! MPD receives a random session capability, never a filesystem path. Local
//! content is served from the exact admitted handle; there is no path reopen.
use crate::windows_file_info::FileIdentity;
use super::protocol::Result;
#[path="media_server_transfer.rs"]
mod transfer;
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, Weak,
    },
    thread,
    time::{Duration, Instant},
};
const MAX_CONNECTIONS: usize = 8;
const MAX_FILES: usize = 1000;
static CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn acquire() -> Option<Self> {
        CONNECTIONS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_CONNECTIONS).then_some(n + 1)
            })
            .ok()
            .map(|_| Self)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        CONNECTIONS.fetch_sub(1, Ordering::AcqRel);
    }
}
fn random() -> Result<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| "random_failed")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
struct Entry {
    identity: Option<FileIdentity>,
    file: crate::windows_io::retire::Owned<File>,
    len: u64,
    modified: std::time::SystemTime,
    live: Arc<AtomicBool>,
    // Only coarse progress; no URL, path, payload, counter or peer is exposed.
    delivered: AtomicUsize,
    failed: Arc<AtomicBool>,
    server_stop:Arc<AtomicBool>,
    mime: &'static str,
}
impl Entry {
    fn valid(&self) -> bool {
        self.live.load(Ordering::Acquire) && !self.failed.load(Ordering::Acquire) && !self.server_stop.load(Ordering::Acquire)
    }
    fn unchanged(&self) -> bool {
        self.file.metadata().is_ok_and(|m| {
            m.is_file() && m.len() == self.len && m.modified().ok() == Some(self.modified)
                &&(!self.file.deferred()||self.identity.is_some()&&crate::windows_file_info::content_identity(&self.file)==self.identity)
        })
    }
}
pub struct Lease {
    entry: Arc<Entry>,
    url: String,
}
impl Lease {
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn healthy(&self) -> bool {
        self.entry.valid()
    }
    pub(crate) fn health_signal(&self)->super::liveness::Signal{super::liveness::Signal::local(self.entry.live.clone(),self.entry.failed.clone(),self.entry.server_stop.clone())}
    pub fn failure_code(&self) -> &'static str {
        if self.entry.failed.load(Ordering::Acquire) { return "mpd_media_read_failed"; }
        match self.entry.delivered.load(Ordering::Acquire) {
            0 => "mpd_media_not_requested",
            1 => "mpd_media_request_only",
            2 => "mpd_media_payload_not_sent",
            _ => "mpd_error_after_media_delivery",
        }
    }
    pub fn identity(&self) -> Option<FileIdentity> {
        self.entry.identity
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.entry.live.store(false, Ordering::Release);
    }
}
struct State {
    stop: Arc<AtomicBool>,
    secret: String,
    files: Mutex<HashMap<String, Weak<Entry>>>,
    sockets: Mutex<HashMap<u64, TcpStream>>,
    serial: AtomicU64,
}
pub struct Server {
    state: Arc<State>,
    address: SocketAddr,
    done: std::sync::mpsc::Receiver<()>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn new() -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| "windows_media_unavailable")?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "windows_media_unavailable")?;
        let address = listener
            .local_addr()
            .map_err(|_| "windows_media_unavailable")?;
        let state = Arc::new(State {
            stop: Arc::new(AtomicBool::new(false)),
            secret: random()?,
            files: Mutex::new(HashMap::new()),
            sockets: Mutex::new(HashMap::new()),
            serial: AtomicU64::new(1),
        });
        let shared = state.clone();
        let (done_tx, done) = std::sync::mpsc::channel();
        let worker = thread::Builder::new()
            .name("sonante-media-accept".into())
            .spawn(move || {
                struct Exit(Arc<AtomicBool>);
                impl Drop for Exit{fn drop(&mut self){self.0.store(true,Ordering::Release);}}
                let _exit=Exit(shared.stop.clone());
                while !shared.stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, peer)) => {
                            if !peer.ip().is_loopback() {
                                continue;
                            }
                            // Accepted-socket inheritance differs by platform.
                            // Use bounded blocking operations explicitly.
                            if stream.set_nonblocking(false).is_err(){shared.stop.store(true,Ordering::Release);break;}
                            let Some(permit) = Permit::acquire() else {
                                continue;
                            };
                            let Ok(copy) = stream.try_clone() else {
                                continue;
                            };
                            let id = shared.serial.fetch_add(1, Ordering::AcqRel);
                            let Ok(mut sockets) = shared.sockets.lock() else {
                                break;
                            };
                            if shared.stop.load(Ordering::Acquire) {
                                break;
                            }
                            sockets.insert(id, copy);
                            drop(sockets);
                            let shared = shared.clone();
                            let cleanup = shared.clone();
                            if thread::Builder::new()
                                .name("sonante-media-read".into())
                                .spawn(move || {
                                    let _permit = permit;
                                    let _ = serve(stream, &shared);
                                    if let Ok(mut sockets) = shared.sockets.lock() {
                                        sockets.remove(&id);
                                    }
                                })
                                .is_err()
                            {
                                cleanup.stop.store(true, Ordering::Release);
                                if let Ok(mut sockets) = cleanup.sockets.lock() {
                                    sockets.remove(&id);
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => {
                            shared.stop.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
                let _ = done_tx.send(());
            })
            .map_err(|_| "worker_failed")?;
        Ok(Self {
            state,
            address,
            done,
            worker: Some(worker),
        })
    }
    pub fn local(&self, root: &Path, path: &Path) -> Result<Lease> {
        if self.state.stop.load(Ordering::Acquire) {
            return Err("windows_media_unavailable");
        }
        let owned_root=root.to_path_buf();let owned_path=path.to_path_buf();
        let (file,len,modified,identity)=crate::windows_io::read(crate::windows_io::READ_TIMEOUT,move|job|{
            if !job.current(){return Err("file_unavailable");}
            let close=if crate::windows_file_info::network_root(&owned_root){Some(crate::windows_io::retire::reserve_file().map_err(|e|if e==crate::windows_io::Error::Busy{"engine_busy"}else{"worker_failed"})?)}else{None};
            let(file,len)=crate::windows_file_info::with_bound_file_while(&owned_root,&owned_path,||job.current(),|file,len|Ok((file,len))).map_err(|_|"file_unavailable")?;
            if !job.current(){return Err("file_unavailable");}
            let modified=file.metadata().and_then(|m|m.modified()).map_err(|_|"file_unavailable")?;
            if !job.current(){return Err("file_unavailable");}
            let identity=crate::windows_file_info::content_identity(&file);
            if !job.current(){return Err("file_unavailable");}
            Ok((crate::windows_io::retire::Owned::new(file,close),len,modified,identity))
        }).map_err(|e|if e==crate::windows_io::Error::Busy{"engine_busy"}else{"file_unavailable"})??;
        if self.state.stop.load(Ordering::Acquire){return Err("windows_media_unavailable");}
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let (extension, mime) = match extension.as_str() {
            "flac" => ("flac", "audio/flac"),
            "wav" => ("wav", "audio/wav"),
            "mp3" => ("mp3", "audio/mpeg"),
            "ogg" => ("ogg", "audio/ogg"),
            "opus" => ("opus", "audio/ogg"),
            "m4a" => ("m4a", "audio/mp4"),
            "aac" => ("aac", "audio/aac"),
            "aif" | "aiff" => ("aiff", "audio/aiff"),
            "wv" => ("wv", "audio/wavpack"),
            _ => return Err("unsupported_file"),
        };
        let key = format!("{}.{}", random()?, extension);
        let entry = Arc::new(Entry {
            identity,
            file,
            len,
            modified,
            live: Arc::new(AtomicBool::new(true)),
            delivered: AtomicUsize::new(0),
            failed: Arc::new(AtomicBool::new(false)),
            server_stop:self.state.stop.clone(),
            mime,
        });
        let mut files = self
            .state
            .files
            .lock()
            .map_err(|_| "windows_media_unavailable")?;
        files.retain(|_, v| v.upgrade().is_some_and(|e| e.live.load(Ordering::Acquire)));
        if files.len() >= MAX_FILES {
            return Err("queue_limit");
        }
        if files.contains_key(&key) {
            return Err("random_failed");
        }
        files.insert(key.clone(), Arc::downgrade(&entry));
        Ok(Lease {
            entry,
            url: format!(
                "http://{}/media/{}/{}",
                self.address, self.state.secret, key
            ),
        })
    }
    pub fn healthy(&self) -> bool {
        !self.state.stop.load(Ordering::Acquire)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
        if let Ok(sockets) = self.state.sockets.lock() {
            for stream in sockets.values() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        // OS disk I/O cannot be cancelled here. At most eight physical readers retain
        // their permits/handles until return; no UI/control thread joins those reads.
        if self.done.recv_timeout(Duration::from_millis(100)).is_ok() {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}
struct Request {
    key: String,
    range: Option<String>,
    head: bool,
}
fn parse(request: &str, secret: &str) -> Option<Request> {
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
    let key = target
        .strip_prefix("/media/")?
        .strip_prefix(secret)?
        .strip_prefix('/')?;
    let (hash, extension) = key.split_once('.')?;
    if hash.len() != 64
        || !hash.bytes().all(|b| b.is_ascii_hexdigit())
        || !matches!(
            extension,
            "flac" | "wav" | "mp3" | "ogg" | "opus" | "m4a" | "aac" | "aiff" | "wv"
        )
    {
        return None;
    }
    let mut range = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        let value = value.trim();
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.chars().any(char::is_control)
        {
            return None;
        }
        if name.eq_ignore_ascii_case("range") {
            let value = value.strip_prefix("bytes=")?;
            let (start, end) = value.split_once('-')?;
            if range.is_some()
                || value.len() > 48
                || (start.is_empty() && end.is_empty())
                || !start.bytes().all(|b| b.is_ascii_digit())
                || !end.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            range = Some(value.into());
        }
        if name.eq_ignore_ascii_case("transfer-encoding")
            || (name.eq_ignore_ascii_case("content-length") && value != "0")
        {
            return None;
        }
    }
    Some(Request {
        key: key.into(),
        range,
        head: method == "HEAD",
    })
}
fn selected_range(value: Option<&str>, len: u64) -> std::result::Result<(u64, u64, bool), ()> {
    let Some(value) = value else {
        return Ok((0, len, false));
    };
    if len == 0 {
        return Err(());
    }
    let (start, end) = value.split_once('-').ok_or(())?;
    if start.is_empty() {
        let n = end.parse::<u64>().map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        let count = n.min(len);
        return Ok((len - count, count, true));
    }
    let start = start.parse::<u64>().map_err(|_| ())?;
    if start >= len {
        return Err(());
    }
    let end = if end.is_empty() {
        len - 1
    } else {
        end.parse::<u64>().map_err(|_| ())?.min(len - 1)
    };
    if end < start {
        return Err(());
    }
    Ok((start, end - start + 1, true))
}
fn response(stream: &mut TcpStream, status: &str, extra: &str) -> std::io::Result<()> {
    stream.write_all(
        format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: 0\r\n{extra}\r\n")
            .as_bytes(),
    )
}
#[cfg(windows)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buffer, offset)
}
#[cfg(not(windows))]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buffer, offset)
}
fn serve(mut stream: TcpStream, state: &State) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut request = Vec::new();
    loop {
        if state.stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Ok(());
        }
        let mut chunk = [0; 1024];
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        request.extend_from_slice(&chunk[..n]);
        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            if end + 4 > 8192 {
                return response(&mut stream, "431 Request Header Fields Too Large", "");
            }
            request.truncate(end + 4);
            break;
        }
        if request.len() >= 8192 {
            return response(&mut stream, "431 Request Header Fields Too Large", "");
        }
    }
    let request = std::str::from_utf8(&request)
        .ok()
        .and_then(|r| parse(r, &state.secret));
    let Some(request) = request else {
        return response(&mut stream, "404 Not Found", "");
    };
    let entry = state
        .files
        .lock()
        .ok()
        .and_then(|m| m.get(&request.key).and_then(Weak::upgrade))
        .filter(|e| e.valid());
    let Some(entry) = entry else {
        return response(&mut stream, "404 Not Found", "");
    };
    if state.stop.load(Ordering::Acquire) {
        return response(&mut stream, "410 Gone", "");
    }
    // The physical reader keeps its original permit through an OS stall.
    // This resource-free fault signal and exact socket duplicate let a paused
    // control/UI route retire it without joining that filesystem operation.
    let fault=entry.failed.clone();let socket=stream.try_clone()?;
    let watch=crate::windows_io::watch::Watch::start(Duration::from_secs(3),move||{
        fault.store(true,Ordering::Release);let _=socket.shutdown(Shutdown::Both);
    }).map_err(|_|{entry.failed.store(true,Ordering::Release);std::io::Error::other("media_watch_unavailable")})?;
    entry.delivered.fetch_max(1, Ordering::AcqRel);
    if !entry.unchanged() {
        entry.failed.store(true, Ordering::Release);
        return response(&mut stream, "410 Gone", "");
    }
    if !watch.park()||!entry.valid(){entry.failed.store(true,Ordering::Release);return Ok(());}
    let Ok((start, count, partial)) = selected_range(request.range.as_deref(), entry.len) else {
        return response(
            &mut stream,
            "416 Range Not Satisfiable",
            &format!("Content-Range: bytes */{}\r\n", entry.len),
        );
    };
    let status = if partial {
        "206 Partial Content"
    } else {
        "200 OK"
    };
    let mut header=format!("HTTP/1.1 {status}\r\nConnection: close\r\nAccept-Ranges: bytes\r\nContent-Type: {}\r\nContent-Length: {count}\r\n",entry.mime);
    if partial {
        header.push_str(&format!(
            "Content-Range: bytes {start}-{}/{}\r\n",
            start + count - 1,
            entry.len
        ));
    }
    header.push_str("\r\n");
    stream.write_all(header.as_bytes())?;
    if !request.head { entry.delivered.fetch_max(2, Ordering::AcqRel); }
    if request.head {
        return Ok(());
    }
    transfer::copy(start,count,||{
        if state.stop.load(Ordering::Acquire)||!entry.valid(){return false;}
        if watch.current(){true}else{entry.failed.store(true,Ordering::Release);false}
    },||{
        if !watch.pulse(){return false;}
        let unchanged=entry.unchanged();watch.park()&&unchanged
    },|buffer,offset|{
        if !watch.pulse(){return Err(std::io::Error::from(std::io::ErrorKind::TimedOut));}
        let read=read_at(&entry.file,buffer,offset);
        if !watch.park(){return Err(std::io::Error::from(std::io::ErrorKind::TimedOut));}read
    },|bytes|{
        transfer::write_current(bytes,||!state.stop.load(Ordering::Acquire)&&entry.valid()&&watch.current(),|bytes|stream.write(bytes))?;
        entry.delivered.fetch_max(3,Ordering::AcqRel);Ok(())
    },||entry.failed.store(true,Ordering::Release))
}

#[cfg(test)]
#[path = "media_server_tests.rs"]
mod tests;
