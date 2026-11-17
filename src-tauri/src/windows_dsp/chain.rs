//! One generation of the owned MPD -> private PCM -> Camilla -> WASAPI Shared
//! route. This owns no queue or UI state; the existing playback actor decides
//! cutover/resume and must replace this generation after Cancel/EOF.
use super::{binding,pcm_core,ring::{ChildHandles,Guardian,Ring},runtime::{CamillaRuntimeController,OwnedWebSocketTransport,RuntimeEndpoint,RuntimeError,RuntimeHealth,RuntimeShared},peq::{convert_preset,PcmFormat}};
use crate::{equalizer::EqPreset,windows_mpd::{process::Process,protocol::Result}};
use serde_json::json;
use std::{fs::{File,OpenOptions},io::Write,path::Path,sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}},thread,time::{Duration,Instant}};

pub(super) fn pcm()->PcmFormat{PcmFormat{sample_rate_hz:48000,channels:2}}
pub(crate) struct Chain {
    pub ring:Arc<Ring>,
    child:Process,
    pub(super) controller:Arc<CamillaRuntimeController<OwnedWebSocketTransport>>,
    _runtime_lock:File,
    guardian:Option<Guardian>,
    pub(super) current:Arc<AtomicU64>,
    pub(super) id:u64,
    pub(super) preset:EqPreset,
    pub(super) reference:bool,
    pub(super) comparison:Option<super::comparison::Active>,
}
fn runtime_error(error:RuntimeError)->&'static str{match error{
    RuntimeError::SessionStale|RuntimeError::IntentChanged=>"EQ_CANCELLED",
    RuntimeError::RollbackFailed|RuntimeError::RuntimePoisoned=>"EQ_ROLLBACK_FAILED",
    RuntimeError::ConcurrentApply=>"EQ_BUSY",
    _=>"EQ_APPLY_FAILED",
}}
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum Health{Ready,Active,Draining,Drained}
impl Chain {
    pub(crate) fn analyzer_reader(&self)->Option<Arc<dyn crate::windows_shared::analyzer::Reader>>{
        if super::CAMILLA_ANALYZER_PROTOCOL!=Some(1){return None;}
        Some(Arc::new(AnalyzerReader{ring:self.ring.clone(),controller:self.controller.clone(),current:self.current.clone(),id:self.id}))
    }
    pub(crate) fn render_identity(&self)->Option<Arc<crate::windows_mpd::process::Identity>>{self.child.identity()}
    #[cfg(test)]
    pub(crate) fn qualification_consumer(&self)->Option<Arc<crate::windows_mpd::process::Identity>>{self.render_identity()}
    pub fn prepare_owned(program:&Path,directory:&Path,preset:EqPreset,stopping:&AtomicBool)->Result<(Self,ChildHandles)>{
        static NEXT:AtomicU64=AtomicU64::new(1);
        let id=NEXT.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|n.checked_add(1)).map_err(|_|"EQ_CANCELLED")?;
        Self::prepare(program,directory,preset,Arc::new(AtomicU64::new(id)),id,stopping)
    }
    /// Dependency/preset/capture are prepared before an existing route is cut.
    /// No MPD command or playback is issued here, including with an empty queue.
    pub fn prepare(program:&Path,directory:&Path,preset:EqPreset,current:Arc<AtomicU64>,id:u64,stopping:&AtomicBool)->Result<(Self,ChildHandles)>{
        crate::equalizer::validate_preset(&preset).map_err(|_|"EQ_PRESET_INVALID")?;
        let render=convert_preset(&preset,pcm()).map_err(|_|"EQ_UNSAFE_HEADROOM")?;
        if stopping.load(Ordering::Acquire)||id==0||current.load(Ordering::Acquire)!=id{return Err("EQ_CANCELLED");}
        let pin=super::CAMILLA_SHA.ok_or("EQ_RUNTIME_UNTRUSTED")?;
        let runtime=crate::windows_mpd::session::runtime_lock(program,pin)?;
        let ring=Ring::new()?;let consumer=ring.child()?;let producer=ring.child()?;
        let endpoint=RuntimeEndpoint::reserve(id,binding::capture(consumer.parameters()),binding::playback()).map_err(runtime_error)?;
        let config=json!({"title":endpoint.title,"devices":{"samplerate":48000,"chunksize":1024,"queuelimit":4,"silence_timeout":0.,"capture_samplerate":48000,"enable_rate_adjust":false,"target_level":4096,"volume_ramp_time":40.,"capture":endpoint.capture,"playback":endpoint.playback},"filters":render.filters,"pipeline":render.pipeline});
        let config_path=directory.join("camilla.json");
        let bytes=serde_json::to_vec(&config).map_err(|_|"EQ_CONFIG_INVALID")?;
        let mut file=OpenOptions::new().write(true).create_new(true).open(&config_path).map_err(|_|"EQ_CONFIG_FAILED")?;
        file.write_all(&bytes).and_then(|_|file.sync_all()).map_err(|_|"EQ_CONFIG_FAILED")?;drop(file);
        let port=endpoint.port.to_string();
        // --wait keeps the owned control process alive after natural EOF for
        // GetStopReason=Done. It never reuses the consumed generation/claim.
        let child=Process::start_owned(program,&["--address","127.0.0.1","--port",&port,"--wait",config_path.to_str().ok_or("unsupported_path")?],directory,Some(&consumer))?;
        drop(consumer);
        let identity=child.identity().ok_or("peer_unverified")?;
        let controller=Arc::new(CamillaRuntimeController::new(endpoint,current.clone(),Arc::new(RuntimeShared::default()),OwnedWebSocketTransport{identity}));
        let chain=Self{ring,child,controller,_runtime_lock:runtime,guardian:None,current,id,preset,reference:false,comparison:None};
        let end=Instant::now()+Duration::from_secs(8);
        loop {
            chain.live()?;
            if stopping.load(Ordering::Acquire){return Err("EQ_CANCELLED");}
            let sample=chain.ring.snapshot()?;
            if sample.consumer_ready && chain.controller.confirm_initial(&chain.preset,pcm()).is_ok(){break;}
            if Instant::now()>=end{return Err("EQ_RUNTIME_START_FAILED");}
            thread::sleep(Duration::from_millis(20));
        }
        Ok((chain,producer))
    }
    pub fn attach(&mut self,producer:&Process,address:std::net::SocketAddr,secret:String,sources:crate::windows_mpd::liveness::Watch)->Result<()>{
        self.live()?;
        if self.guardian.is_some(){return Err("EQ_BUSY");}
        let ring=self.ring.clone();let controller=self.controller.clone();
        let identity=producer.identity().ok_or("peer_unverified")?;
        let producer_stdio=producer.stdio_failure_signal();let consumer_stdio=self.child.stdio_failure_signal();
        let eof_since=std::sync::Mutex::new(None::<Instant>);
        let probe=Arc::new(move ||{
            if producer_stdio.load(Ordering::Acquire)||consumer_stdio.load(Ordering::Acquire)||!sources.healthy(){return false;}
            let Ok(mut client)=crate::windows_mpd::protocol::Client::connect(address,&secret,|stream|identity.verify_peer(stream))else{return false};
            let Ok(status)=client.command("status")else{return false};
            if !crate::windows_mpd::liveness::producer_status_healthy(&status){return false;}

            let Ok(sample)=ring.snapshot()else{return false};
            if sample.flags&(pcm_core::LOST|pcm_core::STOP)!=0{return false;}
            if sample.flags&pcm_core::EOF!=0 {
                // --wait may have no active config after its confirmed drain.
                // Process identity and generation still guard this response.
                let Ok(mut since)=eof_since.lock()else{return false};
                let since=since.get_or_insert_with(Instant::now);
                match controller.stop_reason().ok().and_then(|reason|reason.as_str().map(str::to_owned)).as_deref(){
                    Some("Done")=>true,
                    Some("None")=>since.elapsed()<Duration::from_secs(6),
                    _=>false,
                }
            }else{controller.health()!=RuntimeHealth::Poisoned&&controller.probe_owned().is_ok()}
        });
        self.guardian=Some(Guardian::start_guarded(self.ring.clone(),producer.witness()?,self.child.witness()?,Some(probe))?);
        Ok(())
    }
    pub fn live(&self)->Result<()>{
        if self.current.load(Ordering::Acquire)!=self.id{return Err("EQ_CANCELLED");}
        if !self.child.alive()?{return Err("EQ_RUNTIME_EXITED");}
        self.child.check_stdio()?;
        let sample=self.ring.snapshot()?;
        if sample.flags&(pcm_core::LOST|pcm_core::STOP)!=0{return Err("EQ_CONTINUITY_LOST");}
        Ok(())
    }
    pub fn health(&mut self)->Result<Health>{
        self.live()?;let sample=self.ring.snapshot()?;
        if sample.flags&pcm_core::EOF!=0 {
            // EOF alone is not audible completion. Only Camilla's owned Done
            // acknowledges its playback drain; this still is not a DAC clock.
            if sample.read!=sample.written{return Ok(Health::Draining);}
            return match self.controller.stop_reason().map_err(runtime_error)?.as_str(){Some("Done")=>Ok(Health::Drained),Some("None")=>Ok(Health::Draining),_=>Err("EQ_RUNTIME_UNCONFIRMED")};
        }
        self.controller.confirm_rendered(&self.preset,pcm(),self.reference).map_err(|_|"EQ_RUNTIME_UNCONFIRMED")?;
        Ok(if sample.written==0{Health::Ready}else{Health::Active})
    }
    pub fn apply(&mut self,preset:EqPreset)->Result<()>{
        if matches!(self.health()?,Health::Draining|Health::Drained){return Err("EQ_CONTINUITY_LOST");}
        let result=self.controller.apply_preset(self.id,&preset,pcm()).map_err(runtime_error);
        if result.is_ok(){self.preset=preset;self.reference=false;self.comparison=None;}
        else if self.controller.health()==RuntimeHealth::Poisoned{let _=self.stop();}
        result
    }
    pub fn inherit_render(&mut self,previous:&Self)->Result<()>{
        if previous.reference {
            self.controller.apply_comparison(self.id,&self.preset,pcm(),true,&||true).map_err(runtime_error)?;
            self.reference=true;
        }
        self.comparison=previous.comparison.clone();
        Ok(())
    }
    pub fn applied(&self)->&EqPreset{&self.preset}
    pub fn drain_held(&mut self)->Result<()>{
        // Caller has confirmed MPD pause. Candidate2 preserves pause without
        // Cancel and permits terminal EOF while held; no producer can append.
        self.live()?;self.ring.eof()?;
        let end=Instant::now()+Duration::from_secs(6);
        loop{
            match self.health()?{Health::Drained=>return Ok(()),Health::Draining=>{},_=>return Err("EQ_RUNTIME_UNCONFIRMED")}
            if Instant::now()>=end{return Err("EQ_RUNTIME_UNCONFIRMED");}
            thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn snapshot(&mut self,playing:bool)->Result<super::Snapshot>{
        let health=self.health()?;
        Ok(super::Snapshot {
            applied:if self.reference{None}else{Some(self.preset.clone())},
            comparison:self.comparison_view(),
            protection:super::safety::assess(&self.preset,48000).ok(),
            phase:if health==Health::Active&&playing{super::Phase::Active}else{super::Phase::Ready},
            verified:matches!(health,Health::Ready|Health::Active),
            in_path:Some(true),
            ..super::Snapshot::default()
        })
    }
    pub fn stop(&mut self)->Result<()>{
        // Guardian cancels both exact owned Jobs, independently of MPD pause,
        // UI polling or a stalled capture reader. Never reset mapping indices.
        let guardian=self.guardian.take().map_or(Ok(()),Guardian::shutdown);
        let signalled=self.ring.stop(false);
        let stopped=self.child.terminate();
        guardian.and(signalled).and(stopped)
    }
}
impl Drop for Chain{fn drop(&mut self){if self.stop().is_err(){eprintln!("Windows EQ: cleanup_failed");}}}

struct AnalyzerReader{ring:Arc<Ring>,controller:Arc<CamillaRuntimeController<OwnedWebSocketTransport>>,current:Arc<AtomicU64>,id:u64}
impl crate::windows_shared::analyzer::Reader for AnalyzerReader{
    fn identity(&self)->[u64;3]{let (owner,n0,n1)=self.ring.observation_key();[owner as u64,n0,n1]}
    fn current(&self)->bool{
        self.current.load(Ordering::Acquire)==self.id&&self.controller.health()==RuntimeHealth::Healthy&&self.ring.snapshot().is_ok_and(|s|s.consumer_ready&&s.flags&(pcm_core::EOF|pcm_core::STOP|pcm_core::LOST)==0)
    }
    fn query(&self,action:crate::windows_shared::analyzer::Action,epoch:u64,token:&str)->Result<serde_json::Value>{
        if !self.current(){return Err("WIN_ANALYZER_SOURCE_CHANGED");}
        let reply=self.controller.analyzer_query(self.id,self.ring.observation_key(),action,epoch,token)?;
        if !self.current(){return Err("WIN_ANALYZER_SOURCE_CHANGED");}Ok(reply)
    }
}
