//! Explicit native integration fixture. Uses the actual product Session,
//! Process/Job/stdio, TCP peer proof, ring guardian, queue and source leases.
//! File-only Camilla output is compiled ONLY in this test executable.
use super::*;
#[path="native_session_fixture.rs"]
mod synthetic;

struct Fixture{directory:Directory,program:PathBuf,media:PathBuf}
impl Fixture{
    fn new()->Self{
        let assets=std::env::var_os("SONANTE_SESSION_QUALIFICATION_ASSETS").expect("run the explicit qualification script; no assets configured");
        let assets=PathBuf::from(assets).canonicalize().expect("qualification asset directory");
        let exe=std::env::current_exe().unwrap();
        let expected=exe.parent().unwrap().join("runtimes").canonicalize().expect("extract the current pinned runtimes beside this test executable");
        assert_eq!(assets,expected,"only the supplied relative engine bundle is accepted");
        let directory=Directory::private(&std::env::temp_dir()).unwrap();
        let bin=directory.0.join("runtimes/audio-candidate");fs::create_dir_all(&bin).unwrap();
        let manifest:Manifest=serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../windows/audio-runtime.json"))).unwrap();
        for (name,pin) in [("mpd.exe",manifest.sha256.as_str()),("camilladsp.exe",crate::windows_dsp::CAMILLA_SHA.unwrap())]{
            let source=assets.join(name);let _verified=runtime_lock(&source,pin).expect("unchanged engine pin");
            fs::copy(&source,bin.join(name)).unwrap();
        }
        let placeholder=directory.0.join("runtimes/mpd/mpd.exe");
        fs::create_dir_all(placeholder.parent().unwrap()).unwrap();
        // Session selects the explicitly pinned audio-candidate path before
        // opening an executable. The approved Shared path is never executed.
        let media=directory.0.join("own-media");fs::create_dir(&media).unwrap();let media=media.canonicalize().unwrap();
        Self{directory,program:placeholder,media}
    }
    fn track(&self,index:u32)->Track{
        let name=format!("synthetic-{index}.wav");let path=self.media.join(&name);
        let frames=48017u32;let bytes=frames*4;let mut wav=Vec::with_capacity((44+bytes)as usize);
        wav.extend(b"RIFF");wav.extend((36+bytes).to_le_bytes());wav.extend(b"WAVEfmt ");wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());wav.extend(2u16.to_le_bytes());wav.extend(48000u32.to_le_bytes());wav.extend(192000u32.to_le_bytes());wav.extend(4u16.to_le_bytes());wav.extend(16u16.to_le_bytes());wav.extend(b"data");wav.extend(bytes.to_le_bytes());
        for i in 0..frames{for c in 0..2u32{let sample=(((i.wrapping_mul(109)+c*9973)%16001)as i32-8000)as i16;wav.extend(sample.to_le_bytes());}}
        fs::write(&path,wav).unwrap();
        Track{id:format!("own-{index}"),title:format!("Synthetic{index}"),artist:"Own fixture".into(),album:"Qualification".into(),disc:1,number:index,file_name:name,catalog_album_id:None,path,media_root:Some(self.media.clone()),radio:None,upnp:None,remote_source:None,remote_connection_id:None,plex_image:None,remote_lease:None}
    }
    fn prepare(&self,tracks:Vec<Track>,output:&str)->Session{
        crate::windows_dsp::qualification::with_file_output(self.directory.0.join(output),||{
            Session::prepare_with_eq(&self.program,&self.directory.0,Selection{root:self.media.clone(),tracks,start:0,album_id:None},50,Arc::new(AtomicBool::new(false)),&crate::windows_audio::Saved::default(),Some(crate::equalizer::flat_preset())).expect("real Session prepare with File-only output")
        })
    }
}

#[test]
#[ignore="Native Windows only: explicit current bundle; real Session/PCM/analyzer, File output, no audio endpoint"]
fn isolated_current_session_analyzer_file_qualification(){
    use crate::windows_shared::analyzer::{Action as AnalyzerAction,Receive,Sample};
    let fixture=Fixture::new();let mut source=synthetic::Source::new(4_800_017).expect("own forward-only synthetic source");
    let selected=crate::windows_upnp::media::qualification_selection(source.uri()).expect("only the owned loopback synthetic WAV fixture is admitted");
    let output=fixture.directory.0.join("current-analyzer.raw");
    let mut session=crate::windows_dsp::qualification::with_file_output(output.clone(),||{
        Session::prepare_with_eq(&fixture.program,&fixture.directory.0,selected,100,Arc::new(AtomicBool::new(false)),&crate::windows_audio::Saved::default(),Some(crate::equalizer::flat_preset())).expect("actual stopped Session with current runtime and File sink")
    });
    let producer=session.process.identity().unwrap();let consumer=session.dsp.as_ref().unwrap().qualification_consumer().unwrap();
    let before=session.observe().unwrap();assert_eq!(before.state,"stop");assert_eq!(session.dsp.as_ref().unwrap().ring.snapshot().unwrap().written,0,"preparation never starts PCM");
    let mut old_cursor=before.clone();old_cursor.state="pause".into();old_cursor.elapsed=42.5;old_cursor.duration=100.;
    session.stage(&old_cursor).expect("logical stopped candidate keeps old cursor without PCM");
    assert_eq!(session.observe().unwrap().elapsed,42.5);assert_eq!(session.dsp.as_ref().unwrap().ring.snapshot().unwrap().written,0);
    // Exact selected-track method used by the actor. A forward-only source
    // would expose an accidental inherited Seek to the old 42.5s cursor.
    session.start_selected(super::super::navigation::jump(0,1).unwrap()).expect("actual selected-track start discards old cursor");
    assert!(session.observe().unwrap().elapsed<10.,"source clock must start near zero, not at the inherited cursor");
    let reader=session.dsp.as_ref().unwrap().analyzer_reader().expect("current Camilla analyzer protocol reader");
    assert!(reader.current());let epoch=37;let token="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    reader.query(AnalyzerAction::Start,epoch,token).expect("owned analyzer start");
    let mut receive=Receive::default();let end=Instant::now()+Duration::from_secs(3);let mut frames=0;
    while frames<2{
        assert!(Instant::now()<end,"two fresh real metric frames were not confirmed");
        let at=Instant::now();let reply=reader.query(AnalyzerAction::Read,epoch,token).expect("owned current analyzer read");
        if let Sample::Fresh(frame)=receive.accept(reply,epoch,token,at.elapsed()).expect("current epoch/token/sequence/frame decoder"){
            assert!(frame.left_rms>0.&&frame.right_rms>0.&&frame.left_peak>0.&&frame.right_peak>0.);assert_eq!(frame.spectrum.len(),48);frames+=1;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    reader.query(AnalyzerAction::Stop,epoch,token).expect("owned analyzer stop");
    session.pause().expect("actual product pause");assert_eq!(session.observe().unwrap().state,"pause");
    session.dsp.as_mut().unwrap().drain_held().expect("terminal EOF and owned File Done acknowledgement");
    let snapshot=session.dsp.as_ref().unwrap().ring.snapshot().unwrap();assert_eq!(snapshot.read,snapshot.written);assert!(snapshot.written>0&&snapshot.written<4_800_017);
    assert!(!reader.current());assert!(reader.query(AnalyzerAction::Read,epoch,token).is_err(),"terminal generation cannot publish stale bars");
    let bytes=fs::read(&output).expect("own flushed File capture");assert_eq!(bytes.len()as u64,snapshot.written*8);
    let preset=crate::equalizer::flat_preset();let gain=10f64.powf(crate::windows_dsp::safety::assess(&preset,48000).unwrap().effective_preamp_db/20.);
    let mut max_lsb=0i64;
    for(i,frame)in bytes.chunks_exact(8).enumerate(){for c in 0..2{
        let actual=i32::from_le_bytes(frame[c*4..c*4+4].try_into().unwrap())as i64;
        let expected=(((synthetic::sample(i,c)as i32)<<16)as f64*gain).round()as i64;max_lsb=max_lsb.max((actual-expected).abs());
    }}
    assert!(max_lsb<=2,"accepted source prefix differs from original Flat render");
    source.close().expect("own HTTP source physically joined");session.abort().unwrap();wait_closed(&producer,&consumer);
    println!("SESSION_CURRENT_ANALYZER_FILE=OK; FRESH_REAL_FRAMES={frames}; ACCEPTED_PCM_FRAMES={}; MAX_SAMPLE_ERROR_LSB={max_lsb}; AUDIO_DEVICE_OPENED=false; HARDWARE_CLAIM=false",snapshot.written);
}
fn wait_closed(producer:&crate::windows_mpd::process::Identity,consumer:&crate::windows_mpd::process::Identity){
    let end=Instant::now()+Duration::from_secs(12);
    while producer.alive()||consumer.alive(){assert!(Instant::now()<end,"owned guardian did not close both jobs");std::thread::sleep(Duration::from_millis(25));}
}
#[test]
#[ignore="Native Windows only: explicit bundled fixture script, never automatic execution or audio hardware"]
fn isolated_session_file_qualification(){
    let fixture=Fixture::new();
    let tracks=vec![fixture.track(1),fixture.track(2),fixture.track(3)];
    let mut session=fixture.prepare(tracks,"initial.raw");
    let producer=session.process.identity().unwrap();let consumer=session.dsp.as_ref().unwrap().qualification_consumer().unwrap();
    let state=session.observe().unwrap();assert_eq!(state.state,"stop");
    assert!(producer.alive()&&consumer.alive());assert_eq!(session.dsp.as_ref().unwrap().ring.snapshot().unwrap().written,0);
    assert_eq!(session.queue.len(),3);
    let selected=Selection{root:fixture.media.clone(),tracks:vec![fixture.track(4)],start:0,album_id:None};
    session.enqueue(selected,Placement::End,||true).unwrap();assert_eq!(session.queue.len(),4);
    let revision=queue_edit::revision(1,Some(session.queue_version));
    session.edit_queue(&revision,1,Edit::Move{from:3,to:1},||true).unwrap();assert_eq!(session.queue[1].id,"own-4");
    let revision=queue_edit::revision(1,Some(session.queue_version));
    session.edit_queue(&revision,1,Edit::Remove{index:2},||true).unwrap();assert_eq!(session.queue.len(),3);
    assert!(session.source_watch.as_ref().unwrap().healthy());
    let mut paused=session.observe().unwrap();paused.state="pause".into();paused.elapsed=0.25;
    session.stage(&paused).unwrap();let staged=session.observe().unwrap();assert_eq!(staged.state,"pause");assert_eq!(staged.elapsed,0.25);
    assert_eq!(session.dsp.as_ref().unwrap().ring.snapshot().unwrap().written,0,"staging must not Play or Seek");
    let queue_path=fixture.directory.0.join("own-queue.json");let mut journal=Journal::open_chain(queue_path.clone(),Vec::new());
    journal.replace_logical(session.saved(&staged)).unwrap();let before=fs::read(&queue_path).unwrap();
    let reopened=Journal::open_chain(queue_path.clone(),Vec::new());assert_eq!(reopened.saved.as_ref().unwrap().tracks().len(),3);
    // Drop the real owned container listener, retaining all source leases.
    // Guardian must react while paused, without status polling from this test.
    drop(session.media_server.take());wait_closed(&producer,&consumer);
    assert!(!session.source_watch.as_ref().unwrap().healthy());assert_eq!(fs::read(&queue_path).unwrap(),before);
    drop(session);drop(producer);drop(consumer);
    println!("SESSION_QUEUE_STAGE_SOURCE_GUARD=OK; PCM_WRITTEN=0; AUDIO_DEVICE_OPENED=false");
    // Normal explicit cleanup must independently close both exact owned Jobs.
    let mut session=fixture.prepare(vec![fixture.track(5)],"cleanup.raw");
    let producer=session.process.identity().unwrap();let consumer=session.dsp.as_ref().unwrap().qualification_consumer().unwrap();
    session.abort().unwrap();wait_closed(&producer,&consumer);drop(session);
    println!("SESSION_OWNED_CLEANUP=OK; AUDIO_DEVICE_OPENED=false; HARDWARE_CLAIM=false");
    let mut session=fixture.prepare(vec![fixture.track(6)],"producer-error.raw");
    let producer=session.process.identity().unwrap();let consumer=session.dsp.as_ref().unwrap().qualification_consumer().unwrap();
    // An owned local HTTP404 causes an MPD playback error while the retained
    // source flags remain healthy. The status probe must close both Jobs too.
    let listener=TcpListener::bind("127.0.0.1:0").unwrap();let port=listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let server=std::thread::spawn(move||{
        use std::io::{Read,Write};let end=Instant::now()+Duration::from_secs(5);
        loop{match listener.accept(){
            Ok((mut socket,peer))=>{
                assert!(peer.ip().is_loopback());socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();socket.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request=Vec::new();let mut bytes=[0;1024];
                while !request.windows(4).any(|w|w==b"\r\n\r\n")&&request.len()<32768{let n=socket.read(&mut bytes).unwrap();if n==0{break;}request.extend_from_slice(&bytes[..n]);}
                socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();break;
            }
            Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>{assert!(Instant::now()<end,"own HTTP fixture not requested");std::thread::sleep(Duration::from_millis(10));}
            Err(_)=>panic!("own HTTP fixture failed"),
        }}
    });
    let uri=format!("http://127.0.0.1:{port}/own-missing-fixture.flac");
    let added=session.client.command(&format!("addid {}",protocol::quote(&uri).unwrap())).unwrap();
    let id=protocol::field(&added,"Id").unwrap().unwrap().parse::<u32>().unwrap();
    session.client.command(&format!("playid {id}")).unwrap();
    wait_closed(&producer,&consumer);server.join().unwrap();
    assert!(session.source_watch.as_ref().unwrap().healthy());drop(session);
    println!("SESSION_PRODUCER_STATUS_GUARD=OK; AUDIO_DEVICE_OPENED=false; HARDWARE_CLAIM=false");
}
