//! Discard owned engine stdout/stderr through a fixed buffer. No payload is
//! retained or logged. Authored MPD Direct errors yield fixed diagnostic hints
//! only. A permit belongs to the physical reader until its final return.
use super::protocol::Result;
#[path="stdio_hint.rs"]
mod hints;
use std::{
    io::{self, Read},
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
const LIMIT: usize = 4; // Includes old/candidate and any reader awaiting physical EOF.
static READERS: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn acquire() -> Result<Self> {
        READERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < LIMIT).then_some(n + 1)
            })
            .map(|_| Self)
            .map_err(|_| "engine_busy")
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        READERS.fetch_sub(1, Ordering::AcqRel);
    }
}
#[cfg(test)]
fn discard(reader: &mut impl Read) -> io::Result<()> {discard_observed(reader,|_|{})}
fn discard_observed(reader:&mut impl Read,mut observed:impl FnMut(hints::Hint))->io::Result<()>{
    let mut buffer = [0u8; 4096];
    let mut scan=hints::Scan::default();
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => scan.push(&buffer[..n],&mut observed),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}
pub(super) struct Drain {
    failed: Arc<AtomicBool>,
    pending:Arc<AtomicU8>,
    done: mpsc::Receiver<()>,
    worker: Option<JoinHandle<()>>,
}
impl Drain {
    pub(super) fn start(mut reader: impl Read + Send + 'static) -> Result<Self> {
        let permit = Permit::acquire()?;
        let failed = Arc::new(AtomicBool::new(false));
        let fault = failed.clone();
        let pending=Arc::new(AtomicU8::new(0));let observed=pending.clone();
        let (finished, done) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("sonante-mpd-stdio".into())
            .spawn(move || {
                // The physical pipe reader never waits for diagnostic locks or
                // logs. Only a seven-bit observation crosses to its owner.
                if discard_observed(&mut reader,|hint|{observed.fetch_or(hint.bit(),Ordering::Release);}).is_err() {
                    fault.store(true, Ordering::Release);
                }
                // Close the exact read handle and release physical capacity before
                // signalling. A caller timeout never releases these resources early.
                drop(reader);
                drop(permit);
                let _ = finished.send(());
            })
            .map_err(|_| "engine_stdio_failed")?;
        Ok(Self {
            failed,
            pending,
            done,
            worker: Some(worker),
        })
    }
    pub(super) fn failure_signal(&self)->Arc<AtomicBool>{self.failed.clone()}
    fn take_hints(&self)->u8{self.pending.swap(0,Ordering::AcqRel)}
    fn publish(&self){
        let observed=self.take_hints();
        for hint in hints::ALL{if observed&hint.bit()!=0{crate::local_diagnostics::record(crate::local_diagnostics::Stage::QueueApply,Duration::ZERO,Some(hint.code()));}}
    }
    pub(super) fn check(&self) -> Result<()> {
        self.publish();
        if self.failed.load(Ordering::Acquire) {
            Err("engine_stdio_failed")
        } else {
            Ok(())
        }
    }
}
impl Drop for Drain {
    fn drop(&mut self) {
        // The owner terminates/closes its Job before this field is dropped; the
        // parent writer was closed immediately after CreateProcessW. EOF thus
        // releases the reader. Never wait indefinitely for an OS read/thread.
        let returned = self.done.recv_timeout(Duration::from_millis(100)).is_ok();
        if let Some(worker) = self.worker.take() {
            if worker.is_finished() {
                if worker.join().is_err() {
                    eprintln!("Windows MPD: stdio_worker_failed");
                }
            } else if !returned {
                eprintln!("Windows MPD: stdio_cleanup_pending");
            }
        }
        self.publish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Condvar, Mutex};
    static SERIAL: Mutex<()> = Mutex::new(());
    #[test]
    fn physical_pipe_reader_only_publishes_bits_and_owner_consumes_them_once(){
        let _serial=SERIAL.lock().unwrap();
        let line=b"exception: Failed to open \"Sonante Windows Direct\" (wasapi); Exclusive mode not allowed\n";
        let output=Drain::start(std::io::Cursor::new(line.to_vec())).unwrap();
        output.done.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(output.pending.load(Ordering::Acquire),hints::Hint::Exclusive.bit());
        assert_eq!(output.take_hints(),hints::Hint::Exclusive.bit());assert_eq!(output.take_hints(),0);output.check().unwrap();
    }
    #[test]
    fn real_reader_emits_only_fixed_authored_hints_and_preserves_terminal_read_failures(){
        let _serial=SERIAL.lock().unwrap();
        let line=b"2026-11-05T12:13:14 exception: Failed to open \"Sonante Windows Direct\" (wasapi); Unable to initialize audio client: PRIVATE_URL_TOKEN (0x8889000a)\n";
        struct Fragmented{bytes:Vec<u8>,at:usize,calls:usize}
        impl Read for Fragmented{fn read(&mut self,out:&mut[u8])->io::Result<usize>{self.calls+=1;if self.calls==1{return Err(io::ErrorKind::Interrupted.into());}if self.at==self.bytes.len(){return Err(io::ErrorKind::PermissionDenied.into());}let n=out.len().min(7).min(self.bytes.len()-self.at);out[..n].copy_from_slice(&self.bytes[self.at..self.at+n]);self.at+=n;Ok(n)}}
        let mut source=Fragmented{bytes:line.repeat(100),at:0,calls:0};let mut codes=Vec::new();
        let error=discard_observed(&mut source,|hint|codes.push(hint.code())).unwrap_err();
        assert_eq!(error.kind(),io::ErrorKind::PermissionDenied);assert_eq!(source.at,line.len()*100);assert_eq!(codes,["WIN_DIRECT_STDIO_DEVICE_BUSY"]);assert!(!codes.join("").contains("PRIVATE"));
    }
    #[test]
    fn discards_large_fragmented_output_with_a_fixed_buffer_and_retries_interrupts() {
        let _serial = SERIAL.lock().unwrap();
        struct Output {
            remaining: usize,
            calls: usize,
            largest: usize,
        }
        impl Read for Output {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                self.calls += 1;
                self.largest = self.largest.max(b.len());
                if self.calls == 1 {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                let n = self.remaining.min(b.len()).min(317);
                b[..n].fill(0xa5);
                self.remaining -= n;
                Ok(n)
            }
        }
        let mut source = Output {
            remaining: 2 * 1024 * 1024 + 17,
            calls: 0,
            largest: 0,
        };
        discard(&mut source).unwrap();
        assert_eq!(source.remaining, 0);
        assert_eq!(source.largest, 4096);
        assert!(source.calls > 6000);
    }
    #[test]
    fn benign_eof_and_broken_pipe_are_distinct_from_a_real_read_failure() {
        let _serial = SERIAL.lock().unwrap();
        struct Error(io::ErrorKind);
        impl Read for Error {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(self.0.into())
            }
        }
        assert!(discard(&mut &b""[..]).is_ok());
        assert!(discard(&mut Error(io::ErrorKind::BrokenPipe)).is_ok());
        assert!(discard(&mut Error(io::ErrorKind::PermissionDenied)).is_err());
        let output = Drain::start(Error(io::ErrorKind::PermissionDenied)).unwrap();
        output.done.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(output.check(), Err("engine_stdio_failed"));
    }
    #[test]
    fn reader_capacity_survives_owner_drop_until_the_physical_reader_returns() {
        let _serial = SERIAL.lock().unwrap();
        struct Block(Arc<(Mutex<bool>, Condvar)>);
        impl Read for Block {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                let (lock, signal) = &*self.0;
                let mut ready = lock.lock().unwrap();
                while !*ready {
                    ready = signal.wait(ready).unwrap();
                }
                Ok(0)
            }
        }
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let drains: Vec<_> = (0..LIMIT)
            .map(|_| Drain::start(Block(gate.clone())).unwrap())
            .collect();
        assert!(matches!(Drain::start(&b""[..]), Err("engine_busy")));
        drop(drains);
        assert_eq!(READERS.load(Ordering::Acquire), LIMIT);
        assert!(matches!(Drain::start(&b""[..]), Err("engine_busy")));
        let (lock, signal) = &*gate;
        *lock.lock().unwrap() = true;
        signal.notify_all();
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while READERS.load(Ordering::Acquire) != 0 && std::time::Instant::now() < until {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(READERS.load(Ordering::Acquire), 0);
        let output = Drain::start(&b""[..]).unwrap();
        output.done.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(output.check().is_ok());
    }
    #[test]
    fn real_anonymous_pipe_drains_a_burst_and_finishes_after_writer_close() {
        use std::io::Write;
        let _serial = SERIAL.lock().unwrap();
        let (reader, mut writer) = std::io::pipe().unwrap();
        let output = Drain::start(reader).unwrap();
        for _ in 0..257 {
            writer.write_all(&[0xa5; 8192]).unwrap();
        }
        assert!(matches!(
            output.done.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(writer);
        output.done.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(output.check().is_ok());
        assert_eq!(READERS.load(Ordering::Acquire), 0);
    }
}
