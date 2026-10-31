//! Opt-in, IPv4 LAN UPnP AV renderer. No server, catalog scanning or audio
//! engine is started here. One bounded worker owns all network resources.
use crate::upnp_protocol as p;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Preferences {
    pub enabled: bool,
    pub name: String,
    pub id: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub enabled: bool,
    pub requested_enabled: bool,
    pub name: String,
    pub port: Option<u16>,
    pub error: Option<String>,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub session: Option<String>,
    pub state: String,
    pub elapsed: f64,
    pub duration: f64,
    pub volume: u32,
    pub muted: bool,
    pub volume_writable: bool,
    pub volume_available: bool,
    pub error: bool,
    pub play_mode: String,
}
#[derive(Clone, Debug)]
pub struct Media {
    pub id: String,
    pub uri: String,
    pub metadata: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub artwork: Option<String>,
}
pub enum Control {
    Session { id: String, action: Box<Control> },
    Prepare(Media),
    Clear,
    Play,
    Pause,
    Stop,
    Seek(f64),
    Volume(u32),
    Mute(bool),
}
pub trait Backend: Send + Sync + 'static {
    fn snapshot(&self) -> Result<Snapshot, String>;
    fn volume_snapshot(&self) -> Result<Snapshot, String> {
        self.snapshot()
    }
    fn apply(&self, control: Control) -> Result<(), String>;
}

struct Worker {
    stop: Arc<AtomicBool>,
    join: thread::JoinHandle<()>,
    port: u16,
}
impl Worker {
    fn shutdown(self) {
        self.stop.store(true, Ordering::SeqCst);
        if self.join.join().is_err() {
            eprintln!("[UPnP] worker_shutdown=panicked");
        }
    }
}
#[derive(Default)]
struct Lifecycle {
    prefs: Preferences,
    worker: Option<Worker>,
    error: Option<String>,
}
#[derive(Default)]
pub struct State {
    pub operation: Mutex<()>,
    lifecycle: Mutex<Lifecycle>,
    pub snapshot: Arc<Mutex<Snapshot>>,
}
impl State {
    pub fn preferences(&self) -> Result<Preferences, String> {
        let state = self.lifecycle.lock().map_err(|_| "UPNP_BUSY")?;
        let mut prefs = state.prefs.clone();
        if prefs.name.is_empty() {
            prefs.name = "Sonante".into();
        }
        Ok(prefs)
    }
    pub fn status(&self) -> Result<Status, String> {
        let state = self.lifecycle.lock().map_err(|_| "UPNP_BUSY")?;
        Ok(Status {
            requested_enabled: state.prefs.enabled,
            enabled: state
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.stop.load(Ordering::SeqCst)),
            name: if state.prefs.name.is_empty() {
                "Sonante".into()
            } else {
                state.prefs.name.clone()
            },
            port: state.worker.as_ref().map(|w| w.port),
            error: state.error.clone().or_else(|| {
                state
                    .worker
                    .as_ref()
                    .filter(|worker| worker.stop.load(Ordering::SeqCst))
                    .map(|_| "UPNP_NETWORK_UNAVAILABLE".into())
            }),
        })
    }
    pub fn configure(
        &self,
        mut prefs: Preferences,
        backend: Arc<dyn Backend>,
    ) -> Result<Preferences, String> {
        prefs.name = prefs.name.trim().to_string();
        if prefs.name.is_empty()
            || prefs.name.chars().count() > 64
            || prefs.name.chars().any(char::is_control)
        {
            return Err("UPNP_NAME_INVALID".into());
        }
        let mut state = self.lifecycle.lock().map_err(|_| "UPNP_BUSY")?;
        if let Some(worker) = state.worker.take() {
            worker.shutdown();
        }
        if state.prefs.id.len() == 36 {
            prefs.id = state.prefs.id.clone();
        }
        if prefs.id.len() != 36 || !prefs.id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            prefs.id = p::uuid()?;
        }
        state.prefs = prefs.clone();
        state.error = None;
        if prefs.enabled {
            match start(&prefs, backend, self.snapshot.clone()) {
                Ok(worker) => state.worker = Some(worker),
                Err(error) => {
                    state.error = Some(error.clone());
                    return Err(error);
                }
            }
        }
        Ok(prefs)
    }
    pub fn shutdown(&self) {
        if let Ok(mut state) = self.lifecycle.lock() {
            if let Some(worker) = state.worker.take() {
                worker.shutdown();
            }
        }
    }
}

fn interfaces() -> Result<Vec<Ipv4Addr>, String> {
    let mut head = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err("UPNP_NETWORK_UNAVAILABLE".into());
    }
    let mut result = Vec::new();
    let mut item = head;
    while !item.is_null() {
        unsafe {
            let i = &*item;
            if !i.ifa_addr.is_null()
                && (*i.ifa_addr).sa_family as i32 == libc::AF_INET
                && i.ifa_flags & libc::IFF_UP as u32 != 0
                && i.ifa_flags & libc::IFF_MULTICAST as u32 != 0
            {
                let a = &*(i.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(a.sin_addr.s_addr.to_ne_bytes());
                if !ip.is_loopback() && !ip.is_unspecified() {
                    result.push(ip);
                }
            }
            item = i.ifa_next;
        }
    }
    unsafe { libc::freeifaddrs(head) };
    result.sort();
    result.dedup();
    if result.is_empty() {
        return Err("UPNP_NETWORK_UNAVAILABLE".into());
    }
    Ok(result)
}
fn lan(ip: IpAddr) -> bool {
    matches!(ip,IpAddr::V4(ip) if ip.is_private()||ip.is_link_local()||ip.is_loopback())
}
fn start(
    prefs: &Preferences,
    backend: Arc<dyn Backend>,
    snapshot: Arc<Mutex<Snapshot>>,
) -> Result<Worker, String> {
    let addresses = interfaces()?;
    let listener =
        TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|_| "UPNP_HTTP_BIND_FAILED")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "UPNP_HTTP_BIND_FAILED")?;
    let port = listener
        .local_addr()
        .map_err(|_| "UPNP_HTTP_BIND_FAILED")?
        .port();
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .map_err(|_| "UPNP_DISCOVERY_FAILED")?;
    socket
        .set_reuse_address(true)
        .map_err(|_| "UPNP_DISCOVERY_FAILED")?;
    socket
        .bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 1900)).into())
        .map_err(|_| "UPNP_DISCOVERY_FAILED")?;
    let mut joined = Vec::new();
    for ip in addresses {
        if socket
            .join_multicast_v4(&Ipv4Addr::new(239, 255, 255, 250), &ip)
            .is_ok()
        {
            joined.push(ip);
        } else {
            eprintln!("[UPnP] interface_membership=unavailable");
        }
    }
    if joined.is_empty() {
        return Err("UPNP_DISCOVERY_FAILED".into());
    }
    let addresses = joined;
    socket
        .set_multicast_ttl_v4(2)
        .map_err(|_| "UPNP_DISCOVERY_FAILED")?;
    socket
        .set_nonblocking(true)
        .map_err(|_| "UPNP_DISCOVERY_FAILED")?;
    let socket: UdpSocket = socket.into();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let prefs = prefs.clone();
    let join = thread::Builder::new()
        .name("sonante-upnp".into())
        .spawn(move || {
            run(
                listener, socket, addresses, port, prefs, backend, snapshot, stopping,
            )
        })
        .map_err(|_| "UPNP_THREAD_FAILED")?;
    Ok(Worker { stop, join, port })
}
fn identities(id: &str) -> Vec<(String, String)> {
    let uuid = format!("uuid:{id}");
    ["upnp:rootdevice", p::RENDERER, p::AVT, p::RCS, p::CM]
        .into_iter()
        .map(|kind| (kind.to_string(), format!("{uuid}::{kind}")))
        .chain(std::iter::once((uuid.clone(), uuid.clone())))
        .collect()
}
fn advertise(socket: &UdpSocket, addresses: &[Ipv4Addr], port: u16, id: &str, alive: bool) {
    let multicast = SocketAddr::from((Ipv4Addr::new(239, 255, 255, 250), 1900));
    let reference = socket2::SockRef::from(socket);
    for ip in addresses {
        if reference.set_multicast_if_v4(ip).is_err() {
            continue;
        }
        for (kind, usn) in identities(id) {
            let detail = if alive {
                format!("CACHE-CONTROL: max-age=1800\r\nLOCATION: http://{ip}:{port}/device.xml\r\nSERVER: Linux/1.0 UPnP/1.0 Sonante/{}\r\n",env!("CARGO_PKG_VERSION"))
            } else {
                String::new()
            };
            let message=format!("NOTIFY * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nNT: {kind}\r\nNTS: ssdp:{}\r\nUSN: {usn}\r\n{detail}\r\n",if alive{"alive"}else{"byebye"});
            if socket.send_to(message.as_bytes(), multicast).is_err() {
                eprintln!("[UPnP] discovery_notice=unavailable");
            }
        }
    }
}

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: String,
}
fn read_request(stream: &mut TcpStream) -> Result<Request, ()> {
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|_| ())?;
    stream
        .set_write_timeout(Some(Duration::from_millis(250)))
        .map_err(|_| ())?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    let header_end;
    loop {
        if Instant::now() > deadline || bytes.len() > 8192 {
            return Err(());
        }
        let mut chunk = [0; 1024];
        let n = stream.read(&mut chunk).map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            header_end = end + 4;
            if header_end > 8192 {
                return Err(());
            }
            break;
        }
    }
    let head = std::str::from_utf8(&bytes[..header_end]).map_err(|_| ())?;
    let mut lines = head.split("\r\n");
    let parts: Vec<_> = lines.next().ok_or(())?.split(' ').collect();
    if parts.len() != 3
        || parts[2] != "HTTP/1.1"
        || !parts[1].starts_with('/')
        || parts[1].len() > 128
    {
        return Err(());
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();
    let mut headers = BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (key, value) = line.split_once(':').ok_or(())?;
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.chars().any(|c| c.is_control() && c != '\t')
        {
            return Err(());
        }
        if headers
            .insert(key.to_ascii_lowercase(), value.trim().to_string())
            .is_some()
        {
            return Err(());
        }
    }
    if headers.contains_key("transfer-encoding") || headers.contains_key("expect") {
        return Err(());
    }
    let size = headers
        .get("content-length")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|_| ())?
        .unwrap_or(0);
    if size > 65536 {
        return Err(());
    }
    while bytes.len() < header_end + size {
        if Instant::now() > deadline {
            return Err(());
        }
        let mut chunk = [0; 4096];
        let remaining = (header_end + size - bytes.len()).min(chunk.len());
        let n = stream.read(&mut chunk[..remaining]).map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    if bytes.len() != header_end + size {
        return Err(());
    }
    let body = String::from_utf8(bytes[header_end..].to_vec()).map_err(|_| ())?;
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}
fn response(stream: &mut TcpStream, code: u16, body: &str, headers: &str) {
    let status = match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        412 => "Precondition Failed",
        _ => "Internal Server Error",
    };
    let head=format!("HTTP/1.1 {code} {status}\r\nContent-Type: text/xml; charset=\"utf-8\"\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",body.len());
    if stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(body.as_bytes()))
        .is_err()
    {
        eprintln!("[UPnP] response=disconnected");
    }
}
struct Subscription {
    sid: String,
    peer: IpAddr,
    callback: SocketAddr,
    path: String,
    service: String,
    expires: Instant,
    sequence: u32,
    last: String,
}
struct Renderer {
    backend: Arc<dyn Backend>,
    media: Option<Media>,
    subscriptions: Vec<Subscription>,
}
impl Renderer {
    fn action(
        &mut self,
        service: &str,
        action: &str,
        body: &str,
    ) -> Result<Vec<(&'static str, String)>, u16> {
        let args = p::leaves(body, 65536)?;
        let arg = |key: &str| args.get(key).map(String::as_str).ok_or(402u16);
        if service != p::CM && arg("InstanceID")? != "0" {
            return Err(718);
        }
        if service == p::CM {
            return match action {
                "GetProtocolInfo" => Ok(vec![
                    ("Source", String::new()),
                    ("Sink", p::PROTOCOLS.into()),
                ]),
                "GetCurrentConnectionIDs" => Ok(vec![("ConnectionIDs", "0".into())]),
                "GetCurrentConnectionInfo" => {
                    if arg("ConnectionID")? != "0" {
                        return Err(706);
                    }
                    Ok(vec![
                        ("RcsID", "0".into()),
                        ("AVTransportID", "0".into()),
                        ("ProtocolInfo", "http-get:*:*:*".into()),
                        ("PeerConnectionManager", String::new()),
                        ("PeerConnectionID", "-1".into()),
                        ("Direction", "Input".into()),
                        ("Status", "OK".into()),
                    ])
                }
                _ => Err(401),
            };
        }
        if service == p::AVT && action == "SetAVTransportURI" {
            let raw = arg("CurrentURI")?;
            if raw.is_empty() {
                if let Some(media) = &self.media {
                    self.backend
                        .apply(Control::Session {
                            id: media.id.clone(),
                            action: Box::new(Control::Clear),
                        })
                        .map_err(|_| 501u16)?;
                }
                self.media = None;
                return Ok(vec![]);
            }
            let uri = p::media_url(raw)?;
            let metadata = arg("CurrentURIMetaData")?.to_string();
            let tags = if metadata.trim().is_empty() {
                BTreeMap::new()
            } else {
                p::metadata(&metadata)?
            };
            let text = |key: &str, default: &str| {
                tags.get(key)
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(default)
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(512)
                    .collect::<String>()
            };
            let media = Media {
                id: p::uuid().map_err(|_| 501u16)?,
                uri,
                metadata,
                title: text("title", "UPnP"),
                artist: text("artist", ""),
                album: text("album", ""),
                artwork: tags
                    .get("albumArtURI")
                    .and_then(|uri| p::media_url(uri).ok()),
            };
            self.backend
                .apply(Control::Prepare(media.clone()))
                .map_err(|_| 501u16)?;
            self.media = Some(media);
            return Ok(vec![]);
        }
        if service == p::AVT && action == "GetDeviceCapabilities" {
            return Ok(vec![
                ("PlayMedia", "NETWORK".into()),
                ("RecMedia", "NOT_IMPLEMENTED".into()),
                ("RecQualityModes", "NOT_IMPLEMENTED".into()),
            ]);
        }
        if service == p::RCS && action == "ListPresets" {
            return Ok(vec![("CurrentPresetNameList", "FactoryDefaults".into())]);
        }
        let snapshot = if service == p::RCS {
            self.backend.volume_snapshot()
        } else {
            self.backend.snapshot()
        }
        .map_err(|_| 501u16)?;
        let media = self
            .media
            .as_ref()
            .filter(|media| snapshot.session.as_deref() == Some(media.id.as_str()));
        let state = if media.is_none() {
            "NO_MEDIA_PRESENT"
        } else {
            match snapshot.state.as_str() {
                "play" => "PLAYING",
                "pause" => "PAUSED_PLAYBACK",
                _ => "STOPPED",
            }
        };
        let uri = media.map(|m| m.uri.clone()).unwrap_or_default();
        let metadata = media.map(|m| m.metadata.clone()).unwrap_or_default();
        if service == p::RCS {
            if !matches!(action, "ListPresets" | "SelectPreset") && arg("Channel")? != "Master" {
                return Err(402);
            }
            return match action {
                "ListPresets" => Ok(vec![("CurrentPresetNameList", "FactoryDefaults".into())]),
                "SelectPreset" if arg("PresetName")? == "FactoryDefaults" => Ok(vec![]),
                "GetVolume" if snapshot.volume_available => {
                    Ok(vec![("CurrentVolume", snapshot.volume.to_string())])
                }
                "GetMute" if snapshot.volume_available => Ok(vec![(
                    "CurrentMute",
                    if snapshot.muted { "1" } else { "0" }.into(),
                )]),
                "SetVolume" if snapshot.volume_writable && media.is_some() => {
                    let value = arg("DesiredVolume")?.parse::<u32>().map_err(|_| 402u16)?;
                    if value > 100 {
                        return Err(402);
                    }
                    self.backend
                        .apply(Control::Session {
                            id: media.ok_or(701u16)?.id.clone(),
                            action: Box::new(Control::Volume(value)),
                        })
                        .map_err(|_| 501u16)?;
                    Ok(vec![])
                }
                "SetMute" if snapshot.volume_writable && media.is_some() => {
                    let value = match arg("DesiredMute")? {
                        "1" | "true" => true,
                        "0" | "false" => false,
                        _ => return Err(402),
                    };
                    self.backend
                        .apply(Control::Session {
                            id: media.ok_or(701u16)?.id.clone(),
                            action: Box::new(Control::Mute(value)),
                        })
                        .map_err(|_| 501u16)?;
                    Ok(vec![])
                }
                "GetVolume" | "GetMute" | "SetVolume" | "SetMute" => Err(501),
                _ => Err(401),
            };
        }
        if service != p::AVT {
            return Err(401);
        }
        match action {
            "GetTransportInfo" => Ok(vec![
                ("CurrentTransportState", state.into()),
                (
                    "CurrentTransportStatus",
                    if snapshot.error {
                        "ERROR_OCCURRED"
                    } else {
                        "OK"
                    }
                    .into(),
                ),
                ("CurrentSpeed", "1".into()),
            ]),
            "GetPositionInfo" => Ok(vec![
                ("Track", if media.is_some() { "1" } else { "0" }.into()),
                (
                    "TrackDuration",
                    p::timestamp(if media.is_some() {
                        snapshot.duration
                    } else {
                        0.0
                    }),
                ),
                ("TrackMetaData", metadata),
                ("TrackURI", uri),
                (
                    "RelTime",
                    p::timestamp(if media.is_some() {
                        snapshot.elapsed
                    } else {
                        0.0
                    }),
                ),
                (
                    "AbsTime",
                    p::timestamp(if media.is_some() {
                        snapshot.elapsed
                    } else {
                        0.0
                    }),
                ),
                ("RelCount", "2147483647".into()),
                ("AbsCount", "2147483647".into()),
            ]),
            "GetMediaInfo" => Ok(vec![
                ("NrTracks", if media.is_some() { "1" } else { "0" }.into()),
                (
                    "MediaDuration",
                    p::timestamp(if media.is_some() {
                        snapshot.duration
                    } else {
                        0.0
                    }),
                ),
                ("CurrentURI", uri),
                ("CurrentURIMetaData", metadata),
                ("NextURI", String::new()),
                ("NextURIMetaData", String::new()),
                ("PlayMedium", "NETWORK".into()),
                ("RecordMedium", "NOT_IMPLEMENTED".into()),
                ("WriteStatus", "NOT_IMPLEMENTED".into()),
            ]),
            "GetDeviceCapabilities" => Ok(vec![
                ("PlayMedia", "NETWORK".into()),
                ("RecMedia", "NOT_IMPLEMENTED".into()),
                ("RecQualityModes", "NOT_IMPLEMENTED".into()),
            ]),
            "GetTransportSettings" => Ok(vec![
                (
                    "PlayMode",
                    if snapshot.play_mode.is_empty() {
                        "NORMAL".into()
                    } else {
                        snapshot.play_mode.clone()
                    },
                ),
                ("RecQualityMode", "NOT_IMPLEMENTED".into()),
            ]),
            "GetCurrentTransportActions" => Ok(vec![(
                "Actions",
                if media.is_some() {
                    "Play,Pause,Stop,Seek"
                } else {
                    ""
                }
                .into(),
            )]),
            "Play" | "Pause" | "Stop" | "Seek" => {
                if media.is_none() {
                    return Err(701);
                }
                let command = match action {
                    "Play" => {
                        if arg("Speed")? != "1" {
                            return Err(717);
                        }
                        Control::Play
                    }
                    "Pause" => Control::Pause,
                    "Stop" => Control::Stop,
                    _ => {
                        if !matches!(arg("Unit")?, "REL_TIME" | "ABS_TIME") {
                            return Err(710);
                        }
                        Control::Seek(p::time(arg("Target")?)?)
                    }
                };
                self.backend
                    .apply(Control::Session {
                        id: media.ok_or(701u16)?.id.clone(),
                        action: Box::new(command),
                    })
                    .map_err(|_| 501u16)?;
                Ok(vec![])
            }
            _ => Err(401),
        }
    }
    fn subscribe(&mut self, request: &Request, peer: IpAddr) -> Result<String, ()> {
        let service = match request.path.as_str() {
            "/event/avtransport" => p::AVT,
            "/event/rendering" => p::RCS,
            "/event/connection" => p::CM,
            _ => return Err(()),
        };
        self.subscriptions.retain(|s| s.expires > Instant::now());
        let h = &request.headers;
        if let Some(sid) = h.get("sid") {
            if h.contains_key("callback") || h.contains_key("nt") {
                return Err(());
            }
            let index = self
                .subscriptions
                .iter()
                .position(|s| &s.sid == sid && s.peer == peer && s.service == service)
                .ok_or(())?;
            if request.method == "UNSUBSCRIBE" {
                self.subscriptions.remove(index);
                return Ok(String::new());
            }
            self.subscriptions[index].expires = Instant::now() + Duration::from_secs(300);
            return Ok(format!("SID: {sid}\r\nTIMEOUT: Second-300\r\n"));
        }
        if request.method != "SUBSCRIBE"
            || h.get("nt").map(String::as_str) != Some("upnp:event")
            || self.subscriptions.len() >= 8
        {
            return Err(());
        }
        let raw = h.get("callback").ok_or(())?;
        let raw = raw
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .ok_or(())?;
        let url = reqwest::Url::parse(raw).map_err(|_| ())?;
        // A renderer must not become a callback proxy into another machine.
        if url.scheme() != "http"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(());
        }
        let ip = url
            .host_str()
            .ok_or(())?
            .parse::<IpAddr>()
            .map_err(|_| ())?;
        if ip != peer {
            return Err(());
        }
        let callback = SocketAddr::new(ip, url.port_or_known_default().ok_or(())?);
        let path = if let Some(query) = url.query() {
            format!("{}?{query}", url.path())
        } else {
            url.path().into()
        };
        if path.len() > 1024 || path.chars().any(char::is_control) {
            return Err(());
        }
        let sid = format!("uuid:{}", p::uuid().map_err(|_| ())?);
        self.subscriptions.push(Subscription {
            sid: sid.clone(),
            peer,
            callback,
            path,
            service: service.into(),
            expires: Instant::now() + Duration::from_secs(300),
            sequence: 0,
            last: String::new(),
        });
        Ok(format!("SID: {sid}\r\nTIMEOUT: Second-300\r\n"))
    }
    fn notify(&mut self, snapshot: &Snapshot) {
        self.subscriptions.retain(|s| s.expires > Instant::now());
        for subscription in &mut self.subscriptions {
            let remote = self
                .media
                .as_ref()
                .filter(|m| snapshot.session.as_deref() == Some(m.id.as_str()));
            let state = if remote.is_none() {
                "NO_MEDIA_PRESENT"
            } else {
                match snapshot.state.as_str() {
                    "play" => "PLAYING",
                    "pause" => "PAUSED_PLAYBACK",
                    _ => "STOPPED",
                }
            };
            let variables = if subscription.service == p::CM {
                format!("<e:property><SourceProtocolInfo></SourceProtocolInfo></e:property><e:property><SinkProtocolInfo>{}</SinkProtocolInfo></e:property><e:property><CurrentConnectionIDs>0</CurrentConnectionIDs></e:property>",p::escape(p::PROTOCOLS))
            } else {
                let avt = subscription.service == p::AVT;
                let changes = if avt {
                    format!("<TransportState val=\"{state}\"/><TransportStatus val=\"{}\"/><CurrentPlayMode val=\"{}\"/><CurrentTrack val=\"{}\"/><CurrentTrackDuration val=\"{}\"/><CurrentTrackURI val=\"{}\"/><CurrentTrackMetaData val=\"{}\"/><AVTransportURI val=\"{}\"/><CurrentTransportActions val=\"{}\"/>",if snapshot.error { "ERROR_OCCURRED" } else { "OK" },p::escape(if snapshot.play_mode.is_empty(){"NORMAL"}else{&snapshot.play_mode}),usize::from(remote.is_some()),p::timestamp(if remote.is_some(){snapshot.duration}else{0.0}),p::escape(remote.map(|m|m.uri.as_str()).unwrap_or("")),p::escape(remote.map(|m|m.metadata.as_str()).unwrap_or("")),p::escape(remote.map(|m|m.uri.as_str()).unwrap_or("")),if remote.is_some(){"Play,Pause,Stop,Seek"}else{""})
                } else if !snapshot.volume_available {
                    String::new()
                } else {
                    format!("<Volume channel=\"Master\" val=\"{}\"/><Mute channel=\"Master\" val=\"{}\"/>",snapshot.volume,usize::from(snapshot.muted))
                };
                let event=format!("<Event xmlns=\"urn:schemas-upnp-org:metadata-1-0/{}\"><InstanceID val=\"0\">{changes}</InstanceID></Event>",if avt{"AVT/"}else{"RCS/"});
                format!(
                    "<e:property><LastChange>{}</LastChange></e:property>",
                    p::escape(&event)
                )
            };
            if variables == subscription.last {
                continue;
            }
            let body=format!("<?xml version=\"1.0\"?><e:propertyset xmlns:e=\"urn:schemas-upnp-org:event-1-0\">{variables}</e:propertyset>");
            let request=format!("NOTIFY {} HTTP/1.1\r\nHOST: {}\r\nCONTENT-TYPE: text/xml; charset=\"utf-8\"\r\nNT: upnp:event\r\nNTS: upnp:propchange\r\nSID: {}\r\nSEQ: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",subscription.path,subscription.callback,subscription.sid,subscription.sequence,body.len());
            let sent =
                TcpStream::connect_timeout(&subscription.callback, Duration::from_millis(250))
                    .and_then(|mut stream| {
                        use std::io::BufRead;
                        stream.set_write_timeout(Some(Duration::from_millis(250)))?;
                        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
                        stream.write_all(request.as_bytes())?;
                        let mut line = String::new();
                        std::io::BufReader::new(stream.take(1024)).read_line(&mut line)?;
                        if line.starts_with("HTTP/1.1 200 ") || line.starts_with("HTTP/1.0 200 ") {
                            Ok(())
                        } else {
                            Err(std::io::Error::other("event refused"))
                        }
                    });
            if sent.is_ok() {
                subscription.last = variables;
                subscription.sequence = if subscription.sequence == u32::MAX {
                    1
                } else {
                    subscription.sequence + 1
                };
            } else {
                subscription.expires = Instant::now();
                eprintln!("[UPnP] event_callback=unavailable");
            }
        }
    }
}

fn service_path(path: &str) -> Option<&'static str> {
    match path {
        "/control/avtransport" => Some(p::AVT),
        "/control/rendering" => Some(p::RCS),
        "/control/connection" => Some(p::CM),
        _ => None,
    }
}
fn handle(
    stream: &mut TcpStream,
    peer: IpAddr,
    renderer: &mut Renderer,
    prefs: &Preferences,
    stop: &AtomicBool,
) {
    let Ok(request) = read_request(stream) else {
        response(stream, 400, "", "");
        return;
    };
    if stop.load(Ordering::SeqCst) {
        return;
    }
    match request.method.as_str() {
        "GET" => {
            let document = if request.path == "/device.xml" {
                Some(p::device(&prefs.name, &prefs.id))
            } else {
                p::scpd(&request.path).map(str::to_string)
            };
            if let Some(xml) = document {
                response(stream, 200, &xml, "");
            } else {
                response(stream, 404, "", "");
            }
        }
        "POST" => {
            let Some(service) = service_path(&request.path) else {
                response(stream, 404, "", "");
                return;
            };
            let action = request
                .headers
                .get("soapaction")
                .map(|s| s.trim_matches('"'))
                .and_then(|s| s.split_once('#'))
                .filter(|(s, _)| *s == service)
                .map(|(_, action)| action);
            let Some(action) = action.filter(|s| {
                !s.is_empty() && s.len() < 64 && s.bytes().all(|b| b.is_ascii_alphanumeric())
            }) else {
                response(stream, 500, &p::fault(401), "");
                return;
            };
            if !p::soap_valid(&request.body, service, action) {
                response(stream, 500, &p::fault(402), "");
                return;
            }
            match renderer.action(service, action, &request.body) {
                Ok(fields) => response(stream, 200, &p::success(service, action, &fields), ""),
                Err(code) => {
                    eprintln!("[UPnP] control_result=refused code={code}");
                    response(stream, 500, &p::fault(code), "");
                }
            }
        }
        "SUBSCRIBE" | "UNSUBSCRIBE" => match renderer.subscribe(&request, peer) {
            Ok(headers) => response(stream, 200, "", &headers),
            Err(_) => response(stream, 412, "", ""),
        },
        _ => response(stream, 400, "", ""),
    }
}
fn run(
    listener: TcpListener,
    socket: UdpSocket,
    addresses: Vec<Ipv4Addr>,
    port: u16,
    prefs: Preferences,
    backend: Arc<dyn Backend>,
    snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
) {
    let mut renderer = Renderer {
        backend,
        media: None,
        subscriptions: Vec::new(),
    };
    advertise(&socket, &addresses, port, &prefs.id, true);
    let mut advertised = Instant::now();
    let mut notified = Instant::now();
    let mut budget = (Instant::now(), 0u32);
    let mut pending: Vec<(Instant, SocketAddr, String)> = Vec::new();
    let mut buffer = [0u8; 4096];
    while !stop.load(Ordering::SeqCst) {
        if advertised.elapsed() > Duration::from_secs(600) {
            advertise(&socket, &addresses, port, &prefs.id, true);
            advertised = Instant::now();
        }
        if budget.0.elapsed() > Duration::from_secs(1) {
            budget = (Instant::now(), 0);
        }
        if let Ok((n, peer)) = socket.recv_from(&mut buffer) {
            if lan(peer.ip()) && budget.1 < 16 && pending.len() < 32 {
                budget.1 += 1;
                if let Ok(message) = std::str::from_utf8(&buffer[..n]) {
                    let mut lines = message.split("\r\n");
                    if lines.next() == Some("M-SEARCH * HTTP/1.1") {
                        let headers: BTreeMap<_, _> = lines
                            .filter_map(|line| line.split_once(':'))
                            .map(|(k, v)| (k.to_ascii_lowercase(), v.trim()))
                            .collect();
                        let mx = headers
                            .get("mx")
                            .and_then(|s| s.parse::<u32>().ok())
                            .filter(|n| *n > 0 && *n <= 5);
                        if headers.get("man") == Some(&"\"ssdp:discover\"") && mx.is_some() {
                            let target = headers.get("st").copied().unwrap_or("");
                            let local = (|| {
                                let route = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
                                route.connect(peer).ok()?;
                                Some(route.local_addr().ok()?.ip())
                            })();
                            if let Some(local) = local {
                                for (kind, usn) in identities(&prefs.id) {
                                    if target == "ssdp:all" || target == kind {
                                        let reply=format!("HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nEXT:\r\nLOCATION: http://{local}:{port}/device.xml\r\nSERVER: Linux/1.0 UPnP/1.0 Sonante/{}\r\nST: {kind}\r\nUSN: {usn}\r\n\r\n",env!("CARGO_PKG_VERSION"));
                                        pending.push((
                                            Instant::now()
                                                + Duration::from_millis(
                                                    (budget.1 as u64 * 73)
                                                        % ((mx.unwrap_or(1) as u64) * 1000),
                                                ),
                                            peer,
                                            reply,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        let now = Instant::now();
        pending.retain(|(when, peer, reply)| {
            if *when <= now {
                let _ = socket.send_to(reply.as_bytes(), peer);
                false
            } else {
                true
            }
        });
        match listener.accept() {
            Ok((mut stream, peer)) if lan(peer.ip()) => {
                handle(&mut stream, peer.ip(), &mut renderer, &prefs, &stop)
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => {
                eprintln!("[UPnP] listener=unavailable");
                break;
            }
        }
        if notified.elapsed() > Duration::from_millis(250) {
            let current = snapshot.lock().ok().map(|state| state.clone());
            if let Some(current) = current {
                renderer.notify(&current);
            }
            notified = Instant::now();
        }
        // Sleep in the kernel until network input or the next short event tick.
        // Disable discovery polling after its per-second budget is consumed.
        use std::os::fd::AsRawFd;
        let mut fds = [
            libc::pollfd {
                fd: listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if budget.1 < 16 {
                    socket.as_raw_fd()
                } else {
                    -1
                },
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        unsafe {
            libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 100);
        }
    }
    stop.store(true, Ordering::SeqCst);
    advertise(&socket, &addresses, port, &prefs.id, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake(Mutex<(Snapshot, Vec<String>)>);
    impl Backend for Fake {
        fn snapshot(&self) -> Result<Snapshot, String> {
            Ok(self.0.lock().unwrap().0.clone())
        }
        fn apply(&self, control: Control) -> Result<(), String> {
            if let Control::Session { action, .. } = control {
                return self.apply(*action);
            }
            let mut state = self.0.lock().unwrap();
            match control {
                Control::Prepare(media) => {
                    state.0.session = Some(media.id);
                    state.0.state = "stop".into();
                    state.1.push("prepare".into());
                }
                Control::Play => {
                    state.0.state = "play".into();
                    state.1.push("play".into());
                }
                _ => state.1.push("control".into()),
            }
            Ok(())
        }
    }
    fn body(args: &[(&str, &str)]) -> String {
        format!(
            "<Envelope><Body><Action>{}</Action></Body></Envelope>",
            args.iter()
                .map(|(k, v)| format!("<{k}>{}</{k}>", p::escape(v)))
                .collect::<String>()
        )
    }
    #[test]
    fn upnp_prepare_does_not_play_and_stale_remote_cannot_control_local_queue() {
        let backend = Arc::new(Fake(Mutex::new((Snapshot::default(), vec![]))));
        let mut r = Renderer {
            backend: backend.clone(),
            media: None,
            subscriptions: vec![],
        };
        let args = body(&[
            ("InstanceID", "0"),
            ("CurrentURI", "http://192.168.1.2/music.flac"),
            ("CurrentURIMetaData", ""),
        ]);
        r.action(p::AVT, "SetAVTransportURI", &args).unwrap();
        assert_eq!(backend.0.lock().unwrap().1, vec!["prepare"]);
        r.action(
            p::AVT,
            "Play",
            &body(&[("InstanceID", "0"), ("Speed", "1")]),
        )
        .unwrap();
        assert_eq!(backend.0.lock().unwrap().0.state, "play");
        backend.0.lock().unwrap().0.session = None;
        assert_eq!(
            r.action(p::AVT, "Pause", &body(&[("InstanceID", "0")])),
            Err(701)
        );
        assert!(r
            .action(p::AVT, "GetMediaInfo", &body(&[("InstanceID", "0")]))
            .unwrap()
            .iter()
            .any(|(k, v)| *k == "CurrentURI" && v.is_empty()));
    }
    #[test]
    fn upnp_faults_do_not_report_success_or_reveal_input() {
        let backend = Arc::new(Fake(Mutex::new((Snapshot::default(), vec![]))));
        let mut r = Renderer {
            backend,
            media: None,
            subscriptions: vec![],
        };
        assert_eq!(
            r.action(
                p::AVT,
                "SetAVTransportURI",
                &body(&[
                    ("InstanceID", "0"),
                    ("CurrentURI", "file:///secret"),
                    ("CurrentURIMetaData", "")
                ])
            ),
            Err(716)
        );
        assert_eq!(
            r.action(p::AVT, "GetMediaInfo", &body(&[("InstanceID", "1")])),
            Err(718)
        );
        assert_eq!(
            r.action(
                p::RCS,
                "SetVolume",
                &body(&[
                    ("InstanceID", "0"),
                    ("Channel", "Master"),
                    ("DesiredVolume", "100")
                ])
            ),
            Err(501)
        );
    }
    fn exchange(raw: String) -> String {
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = server.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, peer) = server.accept().unwrap();
            let backend = Arc::new(Fake(Mutex::new((Snapshot::default(), vec![]))));
            let mut renderer = Renderer {
                backend,
                media: None,
                subscriptions: vec![],
            };
            handle(
                &mut stream,
                peer.ip(),
                &mut renderer,
                &Preferences {
                    enabled: true,
                    name: "Fixture".into(),
                    id: "fixture".into(),
                },
                &AtomicBool::new(false),
            );
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        client.write_all(raw.as_bytes()).unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        worker.join().unwrap();
        response
    }
    #[test]
    fn upnp_http_descriptions_and_dispatch_use_real_bounded_transport() {
        assert!(
            exchange("GET /device.xml HTTP/1.1\r\nHost: fixture\r\n\r\n".into())
                .contains("<friendlyName>Fixture</friendlyName>")
        );
        let body=format!("<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:GetProtocolInfo xmlns:u=\"{}\"/></s:Body></s:Envelope>",p::CM);
        let request=format!("POST /control/connection HTTP/1.1\r\nHost: fixture\r\nSOAPAction: \"{}#GetProtocolInfo\"\r\nContent-Length: {}\r\n\r\n{}",p::CM,body.len(),body);
        assert!(exchange(request.clone()).contains("<u:GetProtocolInfoResponse"));
        assert!(
            exchange(request.replace("#GetProtocolInfo", "#GetMediaInfo"))
                .contains("<errorCode>402</errorCode>")
        );
        assert!(exchange(
            "POST /control/connection HTTP/1.1\r\nContent-Length: 65537\r\n\r\n".into()
        )
        .starts_with("HTTP/1.1 400"));
        assert!(exchange(
            "GET /device.xml HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n".into()
        )
        .starts_with("HTTP/1.1 400"));
    }
    #[test]
    fn upnp_subscription_callback_must_be_the_requesting_peer() {
        let backend = Arc::new(Fake(Mutex::new((Snapshot::default(), vec![]))));
        let mut r = Renderer {
            backend,
            media: None,
            subscriptions: vec![],
        };
        let mut request = Request {
            method: "SUBSCRIBE".into(),
            path: "/event/avtransport".into(),
            headers: BTreeMap::from([
                ("nt".into(), "upnp:event".into()),
                ("callback".into(), "<http://192.168.1.9:123/events>".into()),
            ]),
            body: String::new(),
        };
        let peer = "192.168.1.2".parse().unwrap();
        assert!(r.subscribe(&request, peer).is_err());
        request
            .headers
            .insert("callback".into(), "<http://192.168.1.2:123/events>".into());
        assert!(r.subscribe(&request, peer).is_ok());
        assert_eq!(r.subscriptions.len(), 1);
    }
    #[test]
    fn upnp_event_callbacks_confirm_initial_and_changed_state_with_ordered_sequences() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let mut received = Vec::new();
            for _ in 0..2 {
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "owned event callback was not delivered"
                            );
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("owned callback: {error}"),
                    }
                };
                let request = read_request(&mut stream).unwrap();
                assert_eq!(request.method, "NOTIFY");
                received.push((request.headers["seq"].clone(), request.body));
                response(&mut stream, 200, "", "");
            }
            received
        });
        let backend = Arc::new(Fake(Mutex::new((Snapshot::default(), vec![]))));
        let mut renderer = Renderer {
            backend: backend.clone(),
            media: None,
            subscriptions: vec![],
        };
        renderer
            .action(
                p::AVT,
                "SetAVTransportURI",
                &body(&[
                    ("InstanceID", "0"),
                    ("CurrentURI", "http://192.168.1.2/music.flac"),
                    ("CurrentURIMetaData", ""),
                ]),
            )
            .unwrap();
        renderer
            .subscribe(
                &Request {
                    method: "SUBSCRIBE".into(),
                    path: "/event/avtransport".into(),
                    headers: BTreeMap::from([
                        ("nt".into(), "upnp:event".into()),
                        ("callback".into(), format!("<http://{address}/callback>")),
                    ]),
                    body: String::new(),
                },
                address.ip(),
            )
            .unwrap();
        let mut state = backend.snapshot().unwrap();
        renderer.notify(&state);
        state.state = "play".into();
        state.error = true;
        renderer.notify(&state);
        renderer.notify(&state); // unchanged state must not generate another notification
        assert_eq!(renderer.subscriptions[0].sequence, 2);
        let received = server.join().unwrap();
        assert_eq!(received[0].0, "0");
        assert_eq!(received[1].0, "1");
        assert!(received[0].1.contains("STOPPED"));
        assert!(received[1].1.contains("ERROR_OCCURRED"));
    }

    #[test]
    fn upnp_default_and_disabled_configuration_open_no_worker_and_retain_identity() {
        let state = State::default();
        assert!(!state.status().unwrap().enabled);
        let backend = Arc::new(Fake(Mutex::new((Snapshot::default(), vec![]))));
        let first = state
            .configure(
                Preferences {
                    enabled: false,
                    name: "Fixture".into(),
                    id: String::new(),
                },
                backend.clone(),
            )
            .unwrap();
        assert!(!state.status().unwrap().enabled);
        assert!(state.lifecycle.lock().unwrap().worker.is_none());
        assert_eq!(first.id.len(), 36);
        let second = state
            .configure(
                Preferences {
                    enabled: false,
                    name: "Renamed".into(),
                    id: String::new(),
                },
                backend.clone(),
            )
            .unwrap();
        assert_eq!(first.id, second.id);
        assert!(state
            .configure(
                Preferences {
                    enabled: true,
                    name: "bad\nname".into(),
                    id: String::new()
                },
                backend
            )
            .is_err());
        assert_eq!(state.status().unwrap().name, "Renamed");
        state.shutdown();
    }
    #[test]
    fn upnp_static_and_position_queries_do_not_probe_volume() {
        use std::sync::atomic::AtomicUsize;
        #[derive(Default)]
        struct Reads {
            transport: AtomicUsize,
            volume: AtomicUsize,
        }
        impl Backend for Reads {
            fn snapshot(&self) -> Result<Snapshot, String> {
                self.transport.fetch_add(1, Ordering::SeqCst);
                Ok(Snapshot::default())
            }
            fn volume_snapshot(&self) -> Result<Snapshot, String> {
                self.volume.fetch_add(1, Ordering::SeqCst);
                Ok(Snapshot {
                    volume: 42,
                    volume_available: true,
                    ..Snapshot::default()
                })
            }
            fn apply(&self, _: Control) -> Result<(), String> {
                Err("unexpected mutation".into())
            }
        }
        let reads = Arc::new(Reads::default());
        let mut renderer = Renderer {
            backend: reads.clone(),
            media: None,
            subscriptions: vec![],
        };
        renderer
            .action(
                p::AVT,
                "GetDeviceCapabilities",
                &body(&[("InstanceID", "0")]),
            )
            .unwrap();
        renderer
            .action(p::RCS, "ListPresets", &body(&[("InstanceID", "0")]))
            .unwrap();
        assert_eq!(reads.transport.load(Ordering::SeqCst), 0);
        assert_eq!(reads.volume.load(Ordering::SeqCst), 0);
        renderer
            .action(p::AVT, "GetPositionInfo", &body(&[("InstanceID", "0")]))
            .unwrap();
        assert_eq!(reads.transport.load(Ordering::SeqCst), 1);
        assert_eq!(reads.volume.load(Ordering::SeqCst), 0);
        let result = renderer
            .action(
                p::RCS,
                "GetVolume",
                &body(&[("InstanceID", "0"), ("Channel", "Master")]),
            )
            .unwrap();
        assert_eq!(result, vec![("CurrentVolume", "42".into())]);
        assert_eq!(reads.volume.load(Ordering::SeqCst), 1);
    }
}
