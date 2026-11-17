use super::*;
static SERIAL: Mutex<()> = Mutex::new(());
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("sonante-media-{}", random().unwrap()));
        std::fs::create_dir(&p).unwrap();
        Self(p.canonicalize().unwrap())
    }
    fn file(&self, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn get(url: &str, method: &str, extra: &str) -> Vec<u8> {
    let rest = url.strip_prefix("http://").unwrap();
    let (address, path) = rest.split_once('/').unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .write_all(
            format!("{method} /{path} HTTP/1.1\r\nHost: {address}\r\n{extra}\r\n").as_bytes(),
        )
        .unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    bytes
}
fn parts(bytes: &[u8]) -> (String, &[u8]) {
    let end = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    (
        String::from_utf8(bytes[..end].to_vec()).unwrap(),
        &bytes[end..],
    )
}
// Force backpressure on our own loopback sockets without depending on the
// machine's TCP autotuning or allocating a large synthetic music fixture.
fn small_socket_buffer(stream: &TcpStream, receive: bool) {
    let size: i32 = 4096;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let option = if receive { libc::SO_RCVBUF } else { libc::SO_SNDBUF };
        assert_eq!(unsafe { libc::setsockopt(stream.as_raw_fd(), libc::SOL_SOCKET,
            option, (&size as *const i32).cast(), std::mem::size_of_val(&size) as _) }, 0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{setsockopt,SOL_SOCKET,SO_RCVBUF,SO_SNDBUF};
        let option = if receive { SO_RCVBUF } else { SO_SNDBUF };
        assert_eq!(unsafe { setsockopt(stream.as_raw_socket() as _, SOL_SOCKET,
            option, (&size as *const i32).cast(), std::mem::size_of_val(&size) as _) }, 0);
    }
}
#[test]
fn long_consumer_pause_preserves_the_complete_container_without_source_failure() {
    let _serial = SERIAL.lock().unwrap();
    let fixture = Fixture::new();
    let payload: Vec<u8> = (0..192*1024+37).map(|n| (n%251) as u8).collect();
    let path = fixture.file("paused-consumer.wav", &payload);
    let server = Server::new().unwrap();
    let lease = server.local(&fixture.0, &path).unwrap();
    let mut client = TcpStream::connect(server.address).unwrap();
    small_socket_buffer(&client, true);
    let until = Instant::now()+Duration::from_secs(2);
    loop {
        if let Some(socket) = server.state.sockets.lock().unwrap().values().next() {
            small_socket_buffer(socket, false);
            break;
        }
        assert!(Instant::now()<until);
        thread::sleep(Duration::from_millis(1));
    }
    client.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
    let rest = lease.url().strip_prefix("http://").unwrap();
    let (address,path) = rest.split_once('/').unwrap();
    client.write_all(format!("GET /{path} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes()).unwrap();
    let until = Instant::now()+Duration::from_secs(2);
    while lease.entry.delivered.load(Ordering::Acquire)<2 {
        assert!(Instant::now()<until);
        thread::sleep(Duration::from_millis(1));
    }
    thread::sleep(Duration::from_secs(3));
    assert!(lease.healthy(), "consumer pause must not become a filesystem failure");
    let mut response = Vec::new();
    client.read_to_end(&mut response).unwrap();
    let (header,body) = parts(&response);
    assert!(header.contains(&format!("Content-Length: {}\r\n",payload.len())));
    assert!(body == payload, "pause must neither truncate nor repeat container bytes: got {}, expected {}", body.len(), payload.len());
}
#[test]
fn pcm_watch_observes_read_failure_and_release_without_retaining_the_file() {
    let _serial = SERIAL.lock().unwrap();
    let fixture = Fixture::new();
    let path = fixture.file("health-fixture.wav", b"own synthetic bytes");
    let server = Server::new().unwrap();
    let lease = server.local(&fixture.0, &path).unwrap();
    let entry = Arc::downgrade(&lease.entry);
    let watch = super::super::liveness::Watch::new(vec![lease.health_signal()]).unwrap();
    assert!(watch.healthy());
    lease.entry.failed.store(true, Ordering::Release);
    assert!(!watch.healthy());
    lease.entry.failed.store(false, Ordering::Release);
    assert!(watch.healthy());
    drop(lease);
    assert!(!watch.healthy());
    assert!(entry.upgrade().is_none(), "the guardian must not keep a file owner alive");
}
#[test]
fn pcm_watch_retires_if_the_container_server_closes_while_its_file_lease_is_retained(){
    let _serial=SERIAL.lock().unwrap();let fixture=Fixture::new();
    let path=fixture.file("server-health-fixture.wav",b"own synthetic bytes");
    let server=Server::new().unwrap();let lease=server.local(&fixture.0,&path).unwrap();
    let watch=super::super::liveness::Watch::new(vec![lease.health_signal()]).unwrap();
    assert!(watch.healthy());drop(server);
    assert!(!watch.healthy());assert!(!lease.healthy());
}
#[test]
fn ranges_match_original_bytes_without_path_or_credential_exposure() {
    let _serial = SERIAL.lock().unwrap();
    let fixture = Fixture::new();
    let payload: Vec<u8> = (0..262147).map(|n| (n % 251) as u8).collect();
    let path = fixture.file("Aspas \" e Música 你好.wav", &payload);
    let server = Server::new().unwrap();
    let lease = server.local(&fixture.0, &path).unwrap();
    assert!(!lease.url().contains("Música"));
    assert!(!lease.url().contains("fixture"));
    let response = get(lease.url(), "GET", "");
    let (header, body) = parts(&response);
    assert!(header.starts_with("HTTP/1.1 200 "));
    assert_eq!(body, payload);
    assert!(header.contains("Content-Type: audio/wav\r\n"));
    for (range, start, end) in [
        ("0-0", 0, 1),
        ("1-65537", 1, 65538),
        ("262145-", 262145, 262147),
        ("-17", 262130, 262147),
        ("9-999999", 9, 262147),
    ] {
        let response = get(lease.url(), "GET", &format!("Range: bytes={range}\r\n"));
        let (header, body) = parts(&response);
        assert!(header.starts_with("HTTP/1.1 206 "));
        assert_eq!(body, &payload[start..end]);
    }
    let response = get(lease.url(), "HEAD", "Range: bytes=3-18\r\n");
    let (header, body) = parts(&response);
    assert!(header.contains("Content-Length: 16\r\n"));
    assert!(body.is_empty());
}
#[test]
fn authentication_retirement_range_errors_and_unsupported_methods_fail_closed() {
    let _serial = SERIAL.lock().unwrap();
    let f = Fixture::new();
    let path = f.file("fixture.flac", b"synthetic-container-bytes");
    let server = Server::new().unwrap();
    let lease = server.local(&f.0, &path).unwrap();
    for method in ["POST", "PUT", "DELETE", "OPTIONS"] {
        assert!(parts(&get(lease.url(), method, ""))
            .0
            .starts_with("HTTP/1.1 404 "));
    }
    let wrong = lease.url().replace(&server.state.secret, &"0".repeat(64));
    assert!(parts(&get(&wrong, "GET", ""))
        .0
        .starts_with("HTTP/1.1 404 "));
    for range in ["0-1,3-4", "x-y", "1--2", "-", "18446744073709551616-"] {
        let response = get(lease.url(), "GET", &format!("Range: bytes={range}\r\n"));
        let status = parts(&response).0;
        assert!(status.starts_with("HTTP/1.1 404 ") || status.starts_with("HTTP/1.1 416 "));
    }
    for range in ["999-", "9-2", "-0"] {
        assert!(parts(&get(
            lease.url(),
            "GET",
            &format!("Range: bytes={range}\r\n")
        ))
        .0
        .starts_with("HTTP/1.1 416 "));
    }
    assert!(parts(&get(
        lease.url(),
        "GET",
        "Range: bytes=0-1\r\nRange: bytes=0-1\r\n"
    ))
    .0
    .starts_with("HTTP/1.1 404 "));
    let url = lease.url().to_owned();
    drop(lease);
    assert!(parts(&get(&url, "GET", "")).0.starts_with("HTTP/1.1 404 "));
}
#[test]
fn separate_requests_use_independent_offsets_and_server_drop_closes_listener() {
    let _serial = SERIAL.lock().unwrap();
    let f = Fixture::new();
    let data: Vec<u8> = (0..100000).map(|n| (n % 241) as u8).collect();
    let path = f.file("fixture.flac", &data);
    let server = Server::new().unwrap();
    let address = server.address;
    let lease = server.local(&f.0, &path).unwrap();
    let mut tasks = Vec::new();
    for start in [9, 101, 2003, 80000] {
        let url = lease.url().to_owned();
        tasks.push((
            start,
            thread::spawn(move || {
                get(
                    &url,
                    "GET",
                    &format!("Range: bytes={start}-{}\r\n", start + 99),
                )
            }),
        ));
    }
    for (start, task) in tasks {
        let response = task.join().unwrap();
        assert_eq!(parts(&response).1, &data[start..start + 100]);
    }
    drop(server);
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err());
    drop(lease);
}
#[test]
fn no_ancestor_or_symlink_escape_and_no_new_path_open_after_admission() {
    let _serial = SERIAL.lock().unwrap();
    let f = Fixture::new();
    let outside = Fixture::new();
    let path = f.file("fixture.wav", b"original");
    let other = outside.file("outside.wav", b"outside");
    let server = Server::new().unwrap();
    assert!(server.local(&f.0, &other).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&other, f.0.join("alias.wav")).unwrap();
        assert!(server.local(&f.0, &f.0.join("alias.wav")).is_err());
        let lease = server.local(&f.0, &path).unwrap();
        std::fs::rename(&path, f.0.join("held-original.wav")).unwrap();
        std::fs::write(&path, b"outside").unwrap();
        assert_eq!(parts(&get(lease.url(), "GET", "")).1, b"original");
    }
    #[cfg(windows)]
    {
        let lease = server.local(&f.0, &path).unwrap();
        assert!(std::fs::rename(&path, f.0.join("held-original.wav")).is_err());
        assert_eq!(parts(&get(lease.url(), "GET", "")).1, b"original");
    }
}
#[test]
fn mutation_empty_media_and_short_requests_do_not_report_successful_data() {
    let _serial = SERIAL.lock().unwrap();
    let f = Fixture::new();
    let empty = f.file("empty.wav", b"");
    let server = Server::new().unwrap();
    let lease = server.local(&f.0, &empty).unwrap();
    let response = get(lease.url(), "GET", "");
    let (header, body) = parts(&response);
    assert!(header.contains("Content-Length: 0\r\n"));
    assert!(body.is_empty());
    assert!(parts(&get(lease.url(), "GET", "Range: bytes=0-\r\n"))
        .0
        .starts_with("HTTP/1.1 416 "));
    #[cfg(unix)]
    {
        std::fs::write(&empty, b"changed").unwrap();
        assert!(parts(&get(lease.url(), "GET", ""))
            .0
            .starts_with("HTTP/1.1 410 "));
        assert!(!lease.healthy());
    }
    assert!(parse("GET /media/no HTTP/1.1\r\n\r\n", &server.state.secret).is_none());
}

#[test]
fn active_header_reader_is_closed_by_retirement_without_waiting_for_its_timeout() {
    let _serial = SERIAL.lock().unwrap();
    let f = Fixture::new();
    let server = Server::new().unwrap();
    let _lease = server
        .local(&f.0, &f.file("fixture.wav", b"synthetic"))
        .unwrap();
    let mut stream = TcpStream::connect(server.address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    stream.write_all(b"GET ").unwrap();
    let until = Instant::now() + Duration::from_secs(1);
    while server.state.sockets.lock().unwrap().is_empty() {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(1));
    }
    let before = Instant::now();
    drop(server);
    assert!(before.elapsed() < Duration::from_millis(500));
    let mut byte = [0u8; 1];
    assert!(matches!(stream.read(&mut byte), Ok(0) | Err(_)));
}
#[test]
fn global_reader_slots_are_bounded_and_released_on_drop() {
    let _serial = SERIAL.lock().unwrap();
    let until = Instant::now() + Duration::from_secs(1);
    while CONNECTIONS.load(Ordering::Acquire) != 0 {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(1));
    }
    let mut permits: Vec<_> = (0..MAX_CONNECTIONS)
        .map(|_| Permit::acquire().unwrap())
        .collect();
    assert!(Permit::acquire().is_none());
    drop(permits.pop());
    assert!(Permit::acquire().is_some());
    drop(permits);
    assert_eq!(CONNECTIONS.load(Ordering::Acquire), 0);
}
#[test]
fn retired_capabilities_do_not_accumulate_and_queue_file_limit_is_enforced() {
    let _serial = SERIAL.lock().unwrap();
    let f = Fixture::new();
    let path = f.file("fixture.wav", b"synthetic");
    let server = Server::new().unwrap();
    let mut leases = Vec::new();
    for _ in 0..MAX_FILES {
        leases.push(server.local(&f.0, &path).unwrap());
    }
    assert!(matches!(server.local(&f.0, &path), Err("queue_limit")));
    let retired = leases.pop().unwrap();
    let old = retired.url().to_owned();
    drop(retired);
    leases.push(server.local(&f.0, &path).unwrap());
    assert_eq!(server.state.files.lock().unwrap().len(), MAX_FILES);
    assert!(parts(&get(&old, "GET", "")).0.starts_with("HTTP/1.1 404 "));
}

#[test]
fn diagnostics_expose_only_coarse_progress_of_the_exact_lease() {
    let _serial = SERIAL.lock().unwrap();
    let f = Fixture::new();
    let path = f.file("diagnostic.wav", b"synthetic-content");
    let server = Server::new().unwrap();
    let lease = server.local(&f.0, &path).unwrap();
    assert_eq!(lease.failure_code(), "mpd_media_not_requested");
    let wrong = lease.url().replace(&server.state.secret, &"0".repeat(64));
    get(&wrong, "GET", "");
    assert_eq!(lease.failure_code(), "mpd_media_not_requested");
    get(lease.url(), "HEAD", "");
    assert_eq!(lease.failure_code(), "mpd_media_request_only");
    get(lease.url(), "GET", "");
    assert_eq!(lease.failure_code(), "mpd_error_after_media_delivery");
    get(lease.url(), "HEAD", "");
    assert_eq!(lease.failure_code(), "mpd_error_after_media_delivery");
    let second = server.local(&f.0, &path).unwrap();
    assert_eq!(second.failure_code(), "mpd_media_not_requested");
    lease.entry.failed.store(true, Ordering::Release);
    assert_eq!(lease.failure_code(), "mpd_media_read_failed");
}
