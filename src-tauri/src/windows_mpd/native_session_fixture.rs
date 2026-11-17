//! Test-only paced synthetic HTTP WAV. This never accepts a path or remote host.
//! The native Session qualifier uses it to keep a File sink active without
//! replacing the product's source, decoder, PCM, analyzer or controller code.
use std::{io::{Read,Write},net::{Shutdown,TcpListener,TcpStream},sync::{Arc,Mutex,mpsc,atomic::{AtomicBool,Ordering}},thread::{self,JoinHandle},time::{Duration,Instant}};
type Result<T>=std::result::Result<T,&'static str>;
pub(super) fn sample(frame:usize,channel:usize)->i16{(((frame as u32).wrapping_mul(109).wrapping_add(channel as u32*9973)%16001)as i32-8000)as i16}
pub(super) fn wav(frames:usize)->Vec<u8>{
    assert!((1024..=4_800_017).contains(&frames));let bytes=(frames*4)as u32;let mut out=Vec::with_capacity(44+bytes as usize);
    out.extend(b"RIFF");out.extend((36+bytes).to_le_bytes());out.extend(b"WAVEfmt ");out.extend(16u32.to_le_bytes());out.extend(1u16.to_le_bytes());out.extend(2u16.to_le_bytes());out.extend(48000u32.to_le_bytes());out.extend(192000u32.to_le_bytes());out.extend(4u16.to_le_bytes());out.extend(16u16.to_le_bytes());out.extend(b"data");out.extend(bytes.to_le_bytes());
    for i in 0..frames{for c in 0..2{out.extend(sample(i,c).to_le_bytes());}}out
}
pub(super) struct Source{address:std::net::SocketAddr,stop:Arc<AtomicBool>,stream:Arc<Mutex<Option<TcpStream>>>,done:mpsc::Receiver<Result<()>>,worker:Option<JoinHandle<()>>}
impl Source{
    pub(super) fn new(frames:usize)->Result<Self>{
        let body=wav(frames);let listener=TcpListener::bind("127.0.0.1:0").map_err(|_|"fixture_bind")?;
        let address=listener.local_addr().map_err(|_|"fixture_address")?;listener.set_nonblocking(true).map_err(|_|"fixture_socket")?;
        let stop=Arc::new(AtomicBool::new(false));let stream=Arc::new(Mutex::new(None));let(worker_stop,worker_stream)=(stop.clone(),stream.clone());let(tx,done)=mpsc::sync_channel(1);
        let worker=thread::Builder::new().name("sonante-own-paced-wav".into()).spawn(move||{
            let result=(||{
                let end=Instant::now()+Duration::from_secs(12);
                let(mut socket,peer)=loop{
                    if worker_stop.load(Ordering::Acquire){return Ok(());}
                    match listener.accept(){Ok(pair)=>break pair,Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>{if Instant::now()>=end{return Err("fixture_accept_timeout");}thread::sleep(Duration::from_millis(5));},Err(_)=>return Err("fixture_accept"),}
                };
                if !peer.ip().is_loopback(){return Err("fixture_peer");}
                socket.set_read_timeout(Some(Duration::from_secs(2))).map_err(|_|"fixture_socket")?;socket.set_write_timeout(Some(Duration::from_millis(500))).map_err(|_|"fixture_socket")?;
                *worker_stream.lock().map_err(|_|"fixture_socket")?=Some(socket.try_clone().map_err(|_|"fixture_socket")?);
                let mut request=Vec::new();let mut chunk=[0u8;1024];
                while !request.windows(4).any(|w|w==b"\r\n\r\n"){
                    if worker_stop.load(Ordering::Acquire){return Ok(());}
                    if request.len()>=32768{return Err("fixture_request_limit");}
                    let n=socket.read(&mut chunk).map_err(|_|"fixture_request")?;if n==0{return Err("fixture_request_eof");}request.extend_from_slice(&chunk[..n]);
                }
                if !request.starts_with(b"GET /own.wav HTTP/1.")||request.windows(7).any(|w|w.eq_ignore_ascii_case(b"range: ")){return Err("fixture_request_scope");}
                // MPD marks any Accept-Ranges header seekable, even "none".
                // Omit it: this is intentionally a forward-only stream.
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",body.len()).map_err(|_|"fixture_response")?;
                let burst=body.len().min(524288);socket.write_all(&body[..burst]).map_err(|_|"fixture_response")?;
                let end=Instant::now()+Duration::from_secs(30);
                for chunk in body[burst..].chunks(1920){
                    if worker_stop.load(Ordering::Acquire){return Ok(());}
                    if Instant::now()>=end{return Err("fixture_stream_timeout");}
                    if socket.write_all(chunk).is_err(){return if worker_stop.load(Ordering::Acquire){Ok(())}else{Err("fixture_stream_write")};}
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            })();
            let result=match result{
                Err("fixture_request"|"fixture_request_eof"|"fixture_response"|"fixture_stream_write") if worker_stop.load(Ordering::Acquire)=>Ok(()),
                other=>other,
            };
            // Close the worker's duplicate before reporting physical completion.
            if let Ok(mut stream)=worker_stream.lock(){stream.take();}
            let _=tx.send(result);
        }).map_err(|_|"fixture_spawn")?;
        Ok(Self{address,stop,stream,done,worker:Some(worker)})
    }
    pub(super) fn uri(&self)->String{format!("http://{}/own.wav",self.address)}
    pub(super) fn close(&mut self)->Result<()>{
        if self.worker.is_none(){return Ok(());}
        self.stop.store(true,Ordering::Release);
        if let Ok(slot)=self.stream.lock(){if let Some(stream)=slot.as_ref(){let _=stream.shutdown(Shutdown::Both);}}
        let result=self.done.recv_timeout(Duration::from_secs(3)).map_err(|_|"fixture_cleanup_pending")?;
        if let Some(worker)=self.worker.take(){worker.join().map_err(|_|"fixture_worker_panicked")?;}
        result
    }
}
impl Drop for Source{fn drop(&mut self){if self.close().is_err(){eprintln!("Session qualification: synthetic_source_cleanup_unconfirmed");}}}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn own_forward_stream_has_the_declared_exact_synthetic_bytes(){
        let mut source=Source::new(48017).unwrap();let mut stream=TcpStream::connect(source.address).unwrap();stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();stream.write_all(b"GET /own.wav HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();let mut reply=Vec::new();stream.read_to_end(&mut reply).unwrap();let end=reply.windows(4).position(|w|w==b"\r\n\r\n").unwrap()+4;
        assert_eq!(&reply[end..],wav(48017));let header=std::str::from_utf8(&reply[..end]).unwrap();assert!(header.contains(&format!("Content-Length: {}",44+48017*4)));assert!(!header.to_ascii_lowercase().contains("accept-ranges"));source.close().unwrap();
    }
    #[test]fn explicit_cancel_closes_the_own_live_stream_and_joins_before_returning(){
        let mut source=Source::new(4_800_017).unwrap();let mut stream=TcpStream::connect(source.address).unwrap();stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();stream.write_all(b"GET /own.wav HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();let mut first=[0u8;512];assert!(stream.read(&mut first).unwrap()>0);let before=Instant::now();source.close().unwrap();assert!(before.elapsed()<Duration::from_secs(3));assert!(source.worker.is_none());source.close().unwrap();
    }
}
