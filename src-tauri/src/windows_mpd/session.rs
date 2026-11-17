//! One worker owns all Windows engine handles and I/O. The UI never holds a
//! mutex over network, process waits, indexing or file dialogs.
use super::library::{Selection, Track};
use super::transport::{MediaLease,admit_tracks};
use super::options::{self, PlaybackOptionAction, PlaybackOptions};
use super::queue::{Journal, SavedQueue};
use super::queue_edit::{self, Edit, Placement};
use super::{
    process::Process,
    protocol::{self, Client, Playback, Result},
    transaction::{self, Route},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::net::TcpListener;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

#[derive(Deserialize)]
struct Manifest {
    version: String,
    sha256: String,
}
fn manifest() -> Manifest {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../windows/mpd-runtime.json"
    )))
    .expect("embedded runtime manifest")
}
pub fn runtime_version() -> String {
    manifest().version
}

fn token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| "random_failed")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

struct Directory(PathBuf);
impl Directory {
    fn new(parent: &Path) -> Result<Self> {
        fs::create_dir_all(parent).map_err(|_| "runtime_directory_failed")?;
        let path = parent.join(format!("mpd-{}", token()?));
        fs::create_dir(&path).map_err(|_| "runtime_directory_failed")?;
        Ok(Self(path))
    }
    fn private(parent:&Path)->Result<Self>{
        use std::os::windows::ffi::OsStrExt;
        fs::create_dir_all(parent).map_err(|_|"runtime_directory_failed")?;
        let path=parent.join(format!("mpd-{}",token()?));
        let security=crate::windows_dsp::ring::PrivateSecurity::directory()?;
        let attributes=security.attributes();
        let wide:Vec<u16>=path.as_os_str().encode_wide().chain([0]).collect();
        if unsafe{windows_sys::Win32::Storage::FileSystem::CreateDirectoryW(wide.as_ptr(),&attributes)}==0{return Err("runtime_directory_failed");}
        Ok(Self(path))
    }

}
impl Drop for Directory {
    fn drop(&mut self) {
        if fs::remove_dir_all(&self.0).is_err() {
            eprintln!("Windows MPD: runtime_cleanup_failed");
        }
    }
}

fn config_path(path: &Path) -> Result<String> {
    let value = path.to_str().ok_or("unsupported_path")?;
    let value = value
        .strip_prefix("\\\\?\\")
        .unwrap_or(value)
        .replace('\\', "/");
    if value.starts_with("UNC/") || value.starts_with("//") {
        return Err("unsupported_path");
    }
    protocol::quote(&value)
}

pub(crate) fn runtime_lock(program: &Path, expected: &str) -> Result<File> {
        crate::diagnostics::measured(crate::local_diagnostics::Stage::StartupEngineRuntime,move||{
    // FILE_SHARE_READ only: the verified executable cannot be replaced or
    // modified between hashing and CreateProcessW, or during the session.
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(program)
        .map_err(|_| "runtime_missing")?;
    let len = file.metadata().map_err(|_| "runtime_missing")?.len();
    if len == 0 || len > 256 * 1024 * 1024 {
        return Err("runtime_changed");
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let bytes = file.read(&mut buffer).map_err(|_| "runtime_changed")?;
        if bytes == 0 {
            break;
        }
        hash.update(&buffer[..bytes]);
    }
    if format!("{:x}", hash.finalize()) != expected {
        return Err("runtime_changed");
    }
    Ok(file)

        })
    }

pub struct Session {
    process: Process,
    client: Client,
    directory: Directory,
    _runtime_lock: File,
    _media_lock: Vec<MediaLease>,
    media_server: Option<super::media_server::Server>,
    queue: Vec<Track>,
    music_root: PathBuf,
    resume_elapsed: f64,
    album_id: Option<String>,
    current_index: usize,
    song_ids: Vec<u32>,
    stopping: Arc<AtomicBool>,
    options: Option<PlaybackOptions>,
    uris: Vec<String>,
    queue_version: u64,
    uncertain: bool,
    audio_settings: crate::windows_audio::Settings,
    direct: Option<Arc<crate::windows_audio::direct_policy::Session>>,
    output_suspended: bool,
    dsp: Option<crate::windows_dsp::chain::Chain>,
    eq_started: bool,
    staged: Option<crate::windows_dsp::staged::Staged>,
    eq_held: Option<queue_edit::View>,
    source_watch:Option<super::liveness::Watch>,
}


impl Session {
    fn prepare(
        program: &Path,
        root: &Path,
        selected: Selection,
        volume: u8,
        stopping: Arc<AtomicBool>,
        audio: &crate::windows_audio::Saved,
    ) -> Result<Self> {
        Self::prepare_with_eq(program,root,selected,volume,stopping,audio,None)
    }
    fn prepare_with_eq(
        program: &Path,
        root: &Path,
        mut selected: Selection,
        volume: u8,
        stopping: Arc<AtomicBool>,
        audio: &crate::windows_audio::Saved,
        preset: Option<crate::equalizer::EqPreset>,
    ) -> Result<Self> {
        crate::diagnostics::measured(crate::local_diagnostics::Stage::StartupEngine,move||{
        if (selected.tracks.is_empty() && (preset.is_none() || selected.start!=0))
            || selected.tracks.len() > 1000
            || (!selected.tracks.is_empty() && selected.start >= selected.tracks.len())
        {
            return Err("queue_limit");
        }
        let (media_server, media, relative) = if selected.tracks.iter().any(|t|t.upnp.is_some()) {
            let (leases,uris)=crate::windows_upnp::media::admit(&selected)?;
            (None,leases,uris)
        } else if selected.tracks.iter().any(|t| t.radio.is_some()) {
            if selected.tracks.len() != 1 || selected.start != 0 { return Err("RADIO_INVALID"); }
            let radio = selected.tracks[0].radio.as_ref().ok_or("RADIO_INVALID")?;
            crate::windows_radio::stream_url(&radio.stream_url).map_err(|_| "RADIO_INVALID")?;
            (None, Vec::new(), vec![radio.stream_url.clone()])
        } else {
            let server=if selected.tracks.iter().any(|track|track.remote_source.is_none()) {Some(super::media_server::Server::new()?)}else{None};
            let (leases, uris)=admit_tracks(server.as_ref(), &mut selected, || if stopping.load(Ordering::Acquire) { Err("shutting_down") } else { Ok(()) })?;
            (server, leases, uris)
        };
        let music_root = selected.root.clone();
        audio.settings.validate()?;
        audio.settings.require_choice()?;
        if preset.is_some() && audio.settings.direct(){return Err("EQ_SHARED_ONLY");}
        let endpoint=if audio.settings.direct(){Some(crate::windows_audio::devices::resolve(&audio.settings.alsa_device)?)}else{None};
        let candidate_program=program.parent().and_then(Path::parent).ok_or("runtime_missing")?.join("audio-candidate/mpd.exe");
        let candidate_manifest:Manifest=serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../windows/audio-runtime.json"))).map_err(|_|"runtime_changed")?;
        let approved_manifest=manifest();
        let (program,pin)=if audio.settings.direct()||preset.is_some(){(candidate_program.as_path(),candidate_manifest.sha256.as_str())}else{(program,approved_manifest.sha256.as_str())};
        let executable = runtime_lock(program,pin)?;
        let directory = if preset.is_some(){Directory::private(root)?}else{Directory::new(root)?};
        let source_watch=if preset.is_some(){Some(super::transport::watch(&media)?)}else{None};
        let (mut dsp,pcm)=if let Some(preset)=preset {
            let camilla=program.parent().ok_or("runtime_missing")?.join("camilladsp.exe");
            let (chain,handles)=crate::windows_dsp::chain::Chain::prepare_owned(&camilla,&directory.0,preset,&stopping)?;
            (Some(chain),Some(handles))
        }else{(None,None)};
        let empty_library = directory.0.join("media");
        fs::create_dir(&empty_library).map_err(|_| "runtime_directory_failed")?;
        let parent = config_path(&empty_library)?;
        let secret = token()?;
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| "port_failed")?;
        let address = listener.local_addr().map_err(|_| "port_failed")?;
        let config = format!(
            "music_directory {parent}\ndb_file {}\nlog_level \"default\"\nbind_to_address \"127.0.0.1\"\nport \"{}\"\npassword \"{secret}@read,add,control,admin\"\ndefault_permissions \"\"\nzeroconf_enabled \"no\"\nauto_update \"no\"\nfollow_inside_symlinks \"no\"\nfollow_outside_symlinks \"no\"\nreplaygain \"off\"\ninput {{\n plugin \"curl\"\n proxy \"\"\n}}\naudio_output {{\n type \"wasapi\"\n name \"Sonante Windows Shared\"\n exclusive \"no\"\n dop \"no\"\n mixer_type \"software\"\n}}\n",
            config_path(&directory.0.join("mpd.db"))?, address.port());
        let config=if let Some(pcm)=&pcm {
            let offset=config.find("audio_output {\n").ok_or("EQ_CONFIG_INVALID")?;
            format!("{}audio_output {{\n type \"sonante_private_pcm\"\n name \"Sonante Shared EQ\"\n format \"48000:32:2\"\n mixer_type \"software\"\n{} }}\n",&config[..offset],pcm.mpd_config())
        }else if audio.settings.direct(){
            let offset=config.find("audio_output {\n").ok_or("windows_audio_invalid")?;
            format!("{}{}",&config[..offset],audio.settings.output(endpoint.as_deref())?)
        }else{config};
        let config=if audio.settings.audio_buffer_size_kb!=4096{format!("{config}audio_buffer_size \"{}\"\n",audio.settings.audio_buffer_size_kb)}else{config};
        let config_file = directory.0.join("mpd.conf");
        fs::write(&config_file, config).map_err(|_| "runtime_directory_failed")?;
        // Port reservation alone is not authentication: verify the connected
        // TCP tuple belongs to this child before sending its random password.
        drop(listener);
        let mut process=if pcm.is_some(){Process::start_owned(program,&["--no-daemon",config_file.to_str().ok_or("unsupported_path")?],&directory.0,pcm.as_ref())?}else{Process::start(program,&config_file,&directory.0)?};
        drop(pcm); // Each child owns only its whitelisted inherited duplicates.

        let startup = Instant::now() + Duration::from_secs(8);
        let client = loop {
            if stopping.load(Ordering::Acquire) {
                return Err("shutting_down");
            }
            if !process.alive()? {
                return Err(process.exit_reason());
            }
            process.check_stdio()?;
            match Client::connect(address, &secret, |stream| process.verify_peer(stream)) {
                Ok(client) => break client,
                Err(code) => {
                    if Instant::now() >= startup {
                        process.terminate()?;
                        return Err(code);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };
        let mut session = Self {
            process,
            client,
            directory,
            _runtime_lock: executable,
            _media_lock: media,
            media_server,
            queue: selected.tracks,
            music_root,
            resume_elapsed: 0.0,
            album_id: selected.album_id,
            current_index: selected.start,
            song_ids: Vec::new(),
            uris: relative.clone(),
            queue_version: 0,
            uncertain: false,
            options: None,
            audio_settings: audio.settings.clone(),
            direct: None,
            output_suspended: false,
            dsp,
            eq_started:false,
            staged:None,
            eq_held:None,
            source_watch,
            stopping,
        };
        if session.audio_settings.direct(){
            let direct=crate::windows_audio::direct_policy::Session::initialize(|c|session.policy_command(c)).map_err(crate::windows_audio::policy_error)?;
            if session.audio_settings.direct_muted{direct.set_mute(true,false,&direct.token(),|c|session.policy_command(c)).map_err(crate::windows_audio::policy_error)?;}
            session.direct=Some(direct);
        }else{session.send(&format!("setvol {volume}"))?;}
        let deadline = Instant::now() + Duration::from_secs(20);
        for filename in relative {
            if Instant::now() >= deadline {
                return Err("index_timeout");
            }
            let added = session.send(&format!("addid {}", protocol::quote(&filename)?))?;
            session.song_ids.push(
                protocol::field(&added, "Id")?
                    .ok_or("mpd_protocol")?
                    .parse()
                    .map_err(|_| "mpd_protocol")?,
            );
        }
        session.send(&format!("replay_gain_mode {}",audio.settings.replay_gain))?;
        session.send("crossfade 0")?;
        if session.dsp.is_some(){let outputs=session.send("outputs")?;crate::windows_dsp::binding::verify_mpd(&outputs,false)?;}
        if let Some(chain)=session.dsp.as_mut(){chain.attach(&session.process,address,secret,session.source_watch.clone().ok_or("EQ_RUNTIME_START_FAILED")?)?;}
        Ok(session)

        })
    }

    fn stage(&mut self,before:&Playback)->Result<()>{
        if self.eq_started{return Err("EQ_NEW_GENERATION_REQUIRED");}
        let mut before=before.clone();if self.is_radio(){before.elapsed=0.;}
        let staged=crate::windows_dsp::staged::Staged::new(&before)?;
        self.resume_elapsed=staged.position();self.staged=Some(staged);
        Ok(())
    }

    fn policy_command(&mut self,command:&str)->std::result::Result<Vec<String>,String>{
        let result=if command.contains('\n'){
            match command {
                "command_list_begin\nstop\nsetvol 100\nstatus\ncommand_list_end"=>{self.check().and_then(|_|self.client.command_list(&["stop","setvol 100","status"]))},
                "command_list_begin\npause 1\nsetvol 100\nstatus\ncommand_list_end"=>{self.check().and_then(|_|self.client.command_list(&["pause 1","setvol 100","status"]))},
                _=>Err("invalid_argument"),
            }
        }else{self.send(command)};
        result.map_err(str::to_owned)
    }
    fn prepare_direct_gain(&mut self)->Result<()>{
        if let Some(policy)=self.direct.clone(){self.audio_settings.require_choice()?;policy.before_explicit_play(true,|c|self.policy_command(c)).map_err(crate::windows_audio::policy_error)?;}
        Ok(())
    }
    fn queue_view(&mut self) -> Result<queue_edit::View> {
        let view = match queue_edit::read(|command| self.send(command)) {
            Ok(view) => view,
            Err(_) => {
                self.uncertain = true;
                return Err("PLAYBACK_QUEUE_UNCERTAIN");
            }
        };
        if let Err(code) = queue_edit::aligned(&view, &self.song_ids, &self.uris) {
            self.uncertain = true;
            return Err(code);
        }
        self.current_index = view.cursor.unwrap_or(self.current_index);
        self.queue_version = view.version;
        Ok(view)
    }
    fn adopt_queue(&mut self, tracks: Vec<Track>, view: queue_edit::View, fallback: usize) {
        self.queue = tracks;
        self.current_index = view.cursor.unwrap_or(fallback);
        self.queue_version = view.version;
        self.song_ids = view.entries.iter().map(|e| e.id).collect();
        self.uris = view.entries.into_iter().map(|e| e.uri).collect();
        self.album_id = self
            .queue
            .first()
            .and_then(|t| t.catalog_album_id.clone())
            .filter(|id| {
                self.queue
                    .iter()
                    .all(|t| t.catalog_album_id.as_ref() == Some(id))
            });
    }
    fn enqueue(
        &mut self,
        mut selected: Selection,
        placement: Placement,
        current_intent: impl Fn() -> bool,
    ) -> Result<()> {
        if selected.tracks.is_empty() || selected.tracks.len() + self.queue.len() > 1000 {
            return Err("queue_limit");
        }
        if self.is_radio() || self.is_upnp() || selected.tracks.iter().any(|t| t.radio.is_some()||t.upnp.is_some()) { return Err("QUEUE_INSERT_UPNP_ACTIVE"); }
        if self.media_server.is_none() && selected.tracks.iter().any(|track|track.remote_source.is_none()) {self.media_server=Some(super::media_server::Server::new()?);}
        let (locks, uris) = admit_tracks(self.media_server.as_ref(), &mut selected, || if current_intent() { Ok(()) } else { Err("PLAYBACK_SUPERSEDED") })?;
        let before = self.queue_view()?;
        let at = queue_edit::position(
            self.queue.len(),
            if self.queue.is_empty(){None}else{before.cursor.or(Some(self.current_index))},
            placement,
        )?;
        if !current_intent() {
            return Err("PLAYBACK_SUPERSEDED");
        }
        let previous = self.current_index;
        // Retain all admitted media if an ACK/timeout leaves a partial live queue.
        self._media_lock.splice(at..at, locks);
        if let Some(watch)=&self.source_watch{watch.publish(self._media_lock.iter().map(MediaLease::health_signal).collect::<Result<Vec<_>>>()?)?;}
        let result = queue_edit::insert(&before, &uris, at, |command| self.send(command));
        let after = match result {
            Ok(v) => v,
            Err(code) => {
                if code == "PLAYBACK_QUEUE_UNCERTAIN" {
                    self.uncertain = true;
                }
                return Err(code);
            }
        };
        let mut tracks = self.queue.clone();
        let count = selected.tracks.len();
        tracks.splice(at..at, selected.tracks);
        let fallback = if previous >= at {
            previous + count
        } else {
            previous
        };
        self.adopt_queue(tracks, after, fallback);
        Ok(())
    }
    fn edit_queue(
        &mut self,
        revision: &str,
        generation: u64,
        action: Edit,
        current_intent: impl Fn() -> bool,
    ) -> Result<()> {
        if self.is_radio() || self.is_upnp() { return Err("QUEUE_EDIT_UPNP_ACTIVE"); }
        if self.staged.as_ref().is_some_and(|state|state.paused())&&matches!(&action,Edit::Remove{index}if *index==self.current_index){return Err("QUEUE_EDIT_CURRENT");}
        let before = self.queue_view()?;
        if queue_edit::revision(generation, Some(before.version)) != revision {
            return Err("QUEUE_EDIT_CHANGED");
        }
        let plan = queue_edit::plan(
            self.queue.len(),
            before.cursor.or(Some(self.current_index)),
            before.state != "stop",
            action,
        )?;
        if !current_intent() {
            return Err("PLAYBACK_SUPERSEDED");
        }
        let fallback = queue_edit::remap(self.queue.len(), self.current_index, plan).unwrap_or(0);
        let result = queue_edit::edit(&before, plan, |command| self.send(command));
        let after = match result {
            Ok(v) => v,
            Err(code) => {
                if code == "PLAYBACK_QUEUE_UNCERTAIN" {
                    self.uncertain = true;
                }
                return Err(code);
            }
        };
        let mut tracks = self.queue.clone();
        queue_edit::apply(&mut tracks, plan);
        // Locks follow the exact occurrence order. Release a removed handle only
        // after MPD confirms the removal; uncertain edits retain every handle.
        if let Some(watch)=&self.source_watch{
            let mut next=self._media_lock.iter().map(MediaLease::health_signal).collect::<Result<Vec<_>>>()?;
            queue_edit::apply(&mut next,plan);watch.publish(next)?;
        }
        queue_edit::apply(&mut self._media_lock, plan);
        self.adopt_queue(tracks, after, fallback);
        Ok(())
    }

    fn is_radio(&self) -> bool { self.queue.len() == 1 && self.queue[0].radio.is_some() }
    fn is_upnp(&self)->bool{self.queue.len()==1&&self.queue[0].upnp.is_some()}
    fn saved(&self, state: &Playback) -> Option<SavedQueue> {
        if self.is_radio() || self.is_upnp() || self.queue.is_empty() { return None; }
        let mut saved = SavedQueue::capture(
            &self.music_root,
            &self.queue,
            self.current_index,
            state.elapsed,
            state.duration,
            state.volume,
            self.album_id.clone(),
        );
        saved.options = self.options;
        Some(saved)
    }
    fn checkpoint(&mut self) -> Result<Option<SavedQueue>> {
        // Shutdown already rejects new operations. A final read-only status is
        // still allowed before terminating the owned child; no play/seek here.
        if self.uncertain {
            return Err("PLAYBACK_QUEUE_UNCERTAIN");
        }
        if !self.process.alive()? {
            return Err(self.process.exit_reason());
        }
        self.process.check_stdio()?;
        let lines = self.transport_command("status")?;
        self.record_status_error(&lines);
        let state = protocol::playback(&lines).map_err(|code| {
            if code == "mpd_playback_error" { protocol::queue_position(&lines, &self.song_ids, self.current_index).ok().and_then(|index| self._media_lock.get(index)).map_or(code, |lease| lease.failure_code()) } else { code }
        })?;
        self.options = options::read(&lines)?;
        self.queue_version = queue_edit::version(&lines)?;
        if !self.queue.is_empty(){self.current_index = protocol::queue_position(&lines, &self.song_ids, self.current_index)?;}
        let state=if let Some(staged)=&self.staged{staged.observe(state)?}else{state};
        Ok(self.saved(&state))
    }
    fn check(&self) -> Result<()> {
        if self.uncertain {
            return Err("PLAYBACK_QUEUE_UNCERTAIN");
        }
        if self.stopping.load(Ordering::Acquire) {
            return Err("shutting_down");
        }
        if let Some(dsp)=&self.dsp{dsp.live()?;}
        if !self.process.alive()? {
            return Err(if self.dsp.is_some(){"EQ_RUNTIME_EXITED"}else{self.process.exit_reason()});
        }
        self.process.check_stdio()?;
        if self.media_server.as_ref().is_some_and(|server| !server.healthy()) || self._media_lock.iter().any(|lease| !lease.healthy()) {
            return Err("windows_media_unavailable");
        }
        Ok(())
    }
    fn start_selected(&mut self,selected:super::navigation::Selected)->Result<()>{
        // An explicit new-track gesture discards the old staged resume cursor.
        // Commit the choice only after local/gain admission, before Play. Only
        // the separately admitted Toggle/Resume path may consume the old cursor.
        self.start_planned(Some(selected))
    }
    fn start_planned(&mut self,selected:Option<super::navigation::Selected>) -> Result<()> {
        let was_eq_started=self.eq_started;
        let result=(||{
        if self.queue.is_empty(){return Err("no_track");}
        if self.dsp.is_some()&&self.eq_started{return Err("EQ_NEW_GENERATION_REQUIRED");}
        if selected.is_some_and(|value|value.index()>=self.queue.len()){return Err("queue_boundary");}
        self.prepare_direct_gain()?;
        if let Some(selected)=selected{self.current_index=selected.index();self.resume_elapsed=selected.elapsed();}
        self.staged=None;
        if self.resume_elapsed > 0.0 {
            self.resume_at(self.resume_elapsed)?;
            self.resume_elapsed = 0.0;
        } else {
            if self.dsp.is_some(){self.eq_started=true;}
            self.send(&format!("playid {}", self.song_ids[self.current_index]))?;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let state = self.observe()?;
            if state.state == "play" && !state.audio.is_empty() && state.elapsed > 0.01 {
                if self.direct.is_some(){
                    let outputs=self.send("outputs")?;
                    let verified=if state.audio.split(':').nth(1)==Some("dsd"){
                        crate::windows_audio::negotiation::verify_dop(&outputs,&self.audio_settings.alsa_device,&state.audio).map(|format|{
                            crate::local_diagnostics::record_audio_route_event(crate::local_diagnostics::AudioRouteEvent::DirectDopNegotiated{
                                source_rate:format.source.rate,source_channels:format.source.channels,transport:"dop",
                                transport_rate:format.transport_rate,transport_channels:format.transport_channels,
                                render_container_bits:format.render_container_bits,render_valid_bits:format.render_valid_bits,
                            })
                        })
                    }else{
                        crate::windows_audio::negotiation::verify(&outputs,&self.audio_settings.alsa_device,&state.audio).map(|format|{
                            crate::local_diagnostics::record_audio_route_event(crate::local_diagnostics::AudioRouteEvent::DirectPcmNegotiated{
                                source:format.source,plugin_input:format.plugin_input,selected:format.selected,
                                render_container_bits:format.render_container_bits,render_valid_bits:format.render_valid_bits,converted:format.converted,
                            })
                        })
                    };
                    if let Err(code)=verified{if Instant::now()>=deadline{return Err(code);}std::thread::sleep(Duration::from_millis(40));continue;}
                }
                if let Some(dsp)=self.dsp.as_mut(){
                    match dsp.health()? {
                        crate::windows_dsp::chain::Health::Active=>{},
                        // MPD's decoder clock can advance before its first PCM
                        // block is published. Ready is not success: wait within
                        // the original deadline without another Play or Seek.
                        crate::windows_dsp::chain::Health::Ready if Instant::now()<deadline=>{
                            std::thread::sleep(Duration::from_millis(40));continue;
                        },
                        _=>return Err("EQ_RUNTIME_UNCONFIRMED"),
                    }
                    let outputs=self.send("outputs")?;crate::windows_dsp::binding::verify_mpd(&outputs,true)?;
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(self._media_lock.get(self.current_index).map_or("playback_unconfirmed", |lease| lease.failure_code()));
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        })();
        if result.is_err()&&self.dsp.is_some()&&!was_eq_started&&self.eq_started {
            // An accepted command may outlive its failed confirmation. Hold
            // this exact generation; never replay/seek to disguise a failure.
            let held=self.pause().and_then(|_|self.observe()).is_ok_and(|state|state.state!="play");
            if !held&&self.abort().is_err(){self.uncertain=true;return Err("PLAYBACK_QUEUE_UNCERTAIN");}
        }
        result
    }
    fn record_status_error(&self,lines:&[String]){
        if let Ok(Some(raw))=protocol::field(lines,"error"){
            if self.direct.is_some() && raw.to_ascii_lowercase().contains("sonante direct dop unsupported") {
                if let Ok(Some(audio))=protocol::field(lines,"audio") {
                    if let Some(source)=crate::windows_audio::negotiation::parse_dsd_source(audio) {
                        let lower=raw.to_ascii_lowercase();
                        let reason=if lower.contains("endpoint rejected"){"endpoint_rejected"}
                            else if lower.contains("readback mismatch"){"readback_mismatch"}
                            else if lower.contains("invalid dsd"){"invalid_source_format"}
                            else{"dop_unavailable"};
                        crate::local_diagnostics::record_audio_route_event(crate::local_diagnostics::AudioRouteEvent::DirectDopRejected{
                            source_rate:source.rate,source_channels:source.channels,transport:"dop",
                            transport_rate:source.rate/2,transport_channels:source.channels,reason,
                        });
                    }
                }
            }
            crate::diagnostics::playback_error(self.audio_settings.direct(),self.dsp.is_some(),raw);
        }
    }
    fn transport_command(&mut self, command:&str)->Result<Vec<String>> {
        super::transport::command(&self._media_lock,&mut||self.client.command(command))
    }
    fn send(&mut self, command: &str) -> Result<Vec<String>> {
        self.check()?;
        self.transport_command(command)
    }
}

impl Route for Session {
    fn hold_for_replacement(&mut self)->Result<()>{
        if self.dsp.is_none(){return self.stop();}
        // Cancel/Stop terminates PCM. A candidate replacement retains this
        // exact old generation paused, so rollback resumes without Play/Seek.
        let before=self.queue_view()?;self.send("pause 1")?;
        let after=self.queue_view()?;
        if after.state!="pause"||after.cursor!=before.cursor||after.entries!=before.entries||after.version!=before.version||(after.elapsed-before.elapsed).abs()>0.25{return Err("EQ_CURSOR_UNCONFIRMED");}
        self.eq_held=Some(after);Ok(())
    }
    fn restore_held(&mut self,saved:&Playback)->Result<()>{
        if self.dsp.is_none(){
            self.resume_at(saved.elapsed)?;
            if saved.state=="pause"{self.pause()?;}
            return if self.observe()?.state==saved.state{Ok(())}else{Err("rollback_failed")};
        }
        let held=self.eq_held.take().ok_or("rollback_failed")?;
        let observed=self.queue_view()?;
        if observed.state!="pause"||held.entries!=observed.entries||held.cursor!=observed.cursor||held.version!=observed.version||(held.elapsed-observed.elapsed).abs()>0.25{return Err("rollback_failed");}
        if saved.state!="play"{return Err("rollback_failed");}
        self.send("pause 0")?;
        if self.observe()?.state!="play"{return Err("rollback_failed");}Ok(())
    }
    fn suspend_output(&mut self)->Result<()>{
        if self.direct.is_none(){return Ok(());}
        let before=self.send("status")?;
        self.output_suspended=true;
        self.send("disableoutput 0")?;
        let outputs=self.send("outputs")?;
        if protocol::field(&outputs,"outputid")?!=Some("0")||protocol::field(&outputs,"outputenabled")?!=Some("0"){return Err("windows_direct_unconfirmed");}
        let after=self.send("status")?;
        for key in ["state","songid","playlist","elapsed","volume"]{if protocol::field(&before,key)?!=protocol::field(&after,key)?{return Err("windows_direct_unconfirmed");}}
        Ok(())
    }
    fn restore_output(&mut self)->Result<()>{
        if !self.output_suspended{return Ok(());}
        let before=self.send("status")?;
        self.send("enableoutput 0")?;
        let outputs=self.send("outputs")?;
        if protocol::field(&outputs,"outputid")?!=Some("0")||protocol::field(&outputs,"outputenabled")?!=Some("1"){return Err("windows_direct_unconfirmed");}
        let after=self.send("status")?;
        for key in ["state","songid","playlist","elapsed","volume"]{if protocol::field(&before,key)?!=protocol::field(&after,key)?{return Err("windows_direct_unconfirmed");}}
        self.output_suspended=false;Ok(())
    }
    fn observe(&mut self) -> Result<Playback> {
        let lines = self.send("status")?;
        self.record_status_error(&lines);
        let mut state = protocol::playback(&lines).map_err(|code| {
            if code == "mpd_playback_error" { protocol::queue_position(&lines, &self.song_ids, self.current_index).ok().and_then(|index| self._media_lock.get(index)).map_or(code, |lease| lease.failure_code()) } else { code }
        })?;
        self.options = options::read(&lines)?;
        self.queue_version = queue_edit::version(&lines)?;
        if !self.queue.is_empty(){self.current_index = protocol::queue_position(&lines, &self.song_ids, self.current_index)?;}
        if self.is_radio() {
            let tags = if state.state == "play" { Some(self.send("currentsong")?) } else { None };
            super::radio::project(&mut self.queue[0], &mut state, tags.as_deref(), u64::from(self.song_ids[0]))?;
        }
        if let Some(staged)=&self.staged{staged.observe(state)}else{Ok(state)}
    }
    fn stop(&mut self) -> Result<()> {
        let before=if self.staged.is_some(){Some(self.observe()?)}else{None};
        if let Some(stopped)=crate::windows_dsp::staged::Staged::stop_after(before.as_ref(),|command|self.send(command).map(|_|()))?{
            self.resume_elapsed=stopped.position();self.staged=Some(stopped);
        }
        Ok(())
    }
    fn start(&mut self) -> Result<()> {
        self.start_planned(None)
    }
    fn seek(&mut self, seconds: f64) -> Result<()> {
        if self.dsp.is_some(){return Err("EQ_NEW_GENERATION_REQUIRED");}
        self.send(&format!("seekcur {seconds:.3}"))?;
        Ok(())
    }
    fn resume_at(&mut self, seconds: f64) -> Result<()> {
        if self.queue.is_empty(){return Err("no_track");}
        if self.is_radio(){self.resume_elapsed=0.;return self.start();}
        if self.dsp.is_some(){if self.eq_started{return Err("EQ_NEW_GENERATION_REQUIRED");}self.eq_started=true;}
        self.prepare_direct_gain()?;
        self.send(&format!(
            "seekid {} {seconds:.3}",
            self.song_ids[self.current_index]
        ))?;
        Ok(())
    }
    fn pause(&mut self) -> Result<()> {
        if self.is_radio()&&self.dsp.is_none() { return self.stop(); }
        self.send("pause 1")?;
        Ok(())
    }
    fn abort(&mut self) -> Result<()> {
        let dsp=self.dsp.as_mut().map_or(Ok(()),|dsp|dsp.stop());
        let process=self.process.terminate();dsp.and(process)
    }
}

impl super::sleep::Route for Session {
    fn command(&mut self, text: &str) -> Result<Vec<String>> {
        self.send(text)
    }
    fn abort_owned(&mut self) -> Result<()> {self.abort()}
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(policy)=&self.direct{policy.invalidate();}
        // Explicitly wait before Directory is dropped. The job remains a kernel
        // kill-on-close fallback even on crash/abort or a shutdown timeout.
        if let Err(code) = self.abort() {
            eprintln!("Windows MPD session cleanup: {code}");
        }
        let _ = &self.directory;
    }
}
#[cfg(test)]
#[path="native_session_qualification.rs"]
mod native_qualification;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    observation: String,
    pub generation: u64,
    engine_running: bool,
    state: String,
    file_name: Option<String>,
    queue: Vec<Track>,
    #[serde(skip_serializing_if="Vec::is_empty")]
    queue_durations: Vec<Option<f64>>,
    queue_index: usize,
    album_id: Option<String>,
    elapsed: f64,
    duration: f64,
    volume: u8,
    audio: String,
    runtime_version: String,
    warning: Option<&'static str>,
    options: Option<PlaybackOptions>,
    queue_revision: String,
    audio_settings: crate::windows_audio::Settings,
    logical_state: String,
    direct: Option<crate::windows_audio::DirectSnapshot>,
    pub(crate) equalizer:crate::windows_dsp::Snapshot,
    pub(crate) audio_route: crate::audio_route::AudioRouteProjection,
    #[serde(skip)]
    account_status: Option<crate::windows_navidrome::Status>,
    #[serde(skip)]
    config_save_response: Option<super::library::imports::SaveResponse>,
    #[serde(skip)]
    ab_watch:Option<crate::audio::EqTransitionWatch>,
}

pub use super::action::Action;

impl Snapshot {
 pub(crate) fn remote_info_binding(&self,index:usize)->Option<crate::windows_source_info::RemoteBinding>{
  if !self.engine_running||self.warning.is_some()||index!=self.queue_index{return None;}
  let track=self.queue.get(index)?;let source=track.remote_source.as_ref()?;
  if !matches!(source,super::queue_v3::Source::Plex{..}){return None;}
  let lease=track.remote_lease.as_ref()?;if !lease.healthy()||lease.source()!=source{return None;}
  Some(crate::windows_source_info::RemoteBinding{generation:self.generation,revision:self.queue_revision.clone(),index,source:source.clone()})
 }
}
impl Snapshot {
 pub(crate) fn scrobbling_observation(&self,sources:super::library::Sources)->crate::windows_scrobbling::Observation{
  crate::windows_scrobbling::sample(self.engine_running,&self.state,self.warning.is_some()||self.direct.as_ref().is_some_and(|d|!d.consistent),self.elapsed,self.duration,self.queue_index,self.queue.get(self.queue_index),sources)
 }
}
impl Snapshot {
 pub(crate) fn upnp_snapshot(&self)->crate::upnp::Snapshot{
  let consistent=self.warning.is_none()&&self.direct.as_ref().is_none_or(|v|v.consistent);
  crate::upnp::Snapshot{session:if consistent{self.queue.get(self.queue_index).and_then(|t|t.upnp.as_ref()).map(|m|m.session_id.clone())}else{None},state:if !self.engine_running{self.logical_state.clone()}else{self.state.clone()},elapsed:self.elapsed,duration:self.duration,volume:u32::from(self.volume),muted:self.direct.as_ref().map_or(self.volume==0,|d|d.muted),volume_available:consistent&&!self.queue.is_empty(),volume_writable:consistent&&!self.queue.is_empty()&&!self.audio_settings.direct(),error:!consistent,play_mode:self.options.map_or("NORMAL",|o|if o.random{"SHUFFLE"}else if o.repeat{if o.single!=super::options::SingleMode::Off{"REPEAT_ONE"}else{"REPEAT_ALL"}}else{"NORMAL"}).into()}
 }
 pub(crate) fn remote_frame(&self)->crate::windows_remote::model::Frame{
  crate::windows_remote::model::frame(self.generation,self.engine_running,&self.state,&self.logical_state,self.elapsed,self.duration,self.volume,&self.audio,&self.queue,self.queue_index,&self.queue_revision,self.direct.as_ref().is_none_or(|v|v.consistent),self.warning.is_some(),self.audio_settings.direct(),self.direct.as_ref().map_or(self.volume==0,|value|value.muted))
 }
 pub(crate) fn remote_queue(&self,offset:usize)->std::result::Result<crate::remote::QueuePage,String>{crate::windows_remote::model::queue(&self.remote_frame(),&self.queue,&self.queue_durations,offset)}
 pub(crate) fn remote_track(&self,key:&str)->Option<Track>{let frame=self.remote_frame();(frame.snapshot.available&&frame.snapshot.media==key).then(||self.queue.get(self.queue_index).cloned()).flatten()}
}
impl Snapshot {pub(crate) fn current_source(&self,index:usize)->Option<&super::queue_v3::Source>{if index!=self.queue_index{return None;}self.queue.get(index)?.remote_source.as_ref()}}
pub(crate) enum AccountChange {Connect(crate::windows_navidrome::PreparedAccount),Disconnect}
struct ConfigPhase(Arc<AtomicU64>);
impl Drop for ConfigPhase {fn drop(&mut self){self.0.fetch_add(1,Ordering::Release);}}
enum Operation {
    Account(AccountChange,u64),
    PauseAB(u64,u64),
    Configure(Option<Vec<PathBuf>>,super::library::Sources,Option<crate::windows_plex::PreparedAccount>,Option<crate::windows_audio::Settings>,bool,Option<bool>,Option<bool>,Option<super::library::UsageMode>,u64),
    ConfirmDirect(crate::windows_audio::Settings,u64),
    Equalizer(Option<crate::equalizer::EqPreset>,Option<crate::equalizer::EqPreset>,Vec<Track>,u64,u64),
    CompareEqualizer(crate::windows_dsp::comparison::Action,Option<String>,Option<crate::equalizer::EqPreset>,crate::audio::EqTransitionWatch,u64),
    Status,
    Load(Selection, Option<u64>),
    Enqueue(Selection, Placement, u64),
    Edit(String, Edit, u64),
    Action(u64, Action, Option<u64>, Option<Vec<Track>>),
    Upnp(crate::windows_upnp::Request,crate::windows_upnp::Guard,Option<u64>),
    Remote(u64,crate::remote::Command,crate::remote::Guard,Option<u64>,Option<Vec<Track>>,Option<u64>),
    Gain(u64,Option<u64>,Action),
    StopRadio(String, Option<u64>),
    Recover,
    SleepStop(u64),
    Shutdown,
}
struct Message {
    operation: Operation,
    reply: mpsc::Sender<Result<Snapshot>>,
}

#[derive(Clone)]
pub struct Controller {
    sender: super::admission::Sender<Message>,
    intent: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
    source: Arc<super::source::View>,
    system_media: Arc<crate::windows_system_media::View>,
    output: Arc<crate::windows_output::View<super::process::Identity>>,
    analyzer:Arc<crate::windows_shared::analyzer::View>,
    navidrome: crate::windows_navidrome::Controller,
    plex: crate::windows_plex::Controller,
    settings:super::library::Library,
    config_revision:Arc<AtomicU64>,
    ready:Arc<AtomicBool>,
    audio_settings:crate::windows_audio::Store,
    direct_context:Arc<std::sync::Mutex<Option<(u64,crate::windows_audio::direct_policy::Context)>>>,
    audio_admission:Arc<crate::windows_audio::admission::Gate>,
    eq_intervention:Arc<AtomicU64>,
}
impl Controller {
    pub fn new(program: PathBuf, root: PathBuf, queue_file: PathBuf, legacy_queue: Vec<PathBuf>, history: crate::windows_history::Controller, navidrome:crate::windows_navidrome::Controller, plex:crate::windows_plex::Controller, settings:super::library::Library) -> Result<Self> {
        Self::new_observed(program,root,queue_file,legacy_queue,history,navidrome,plex,settings,|_|{})
    }
    pub(crate) fn new_observed<F:Fn(&Result<Snapshot>)+Send+'static>(program: PathBuf, root: PathBuf, queue_file: PathBuf, legacy_queue: Vec<PathBuf>, history: crate::windows_history::Controller, navidrome:crate::windows_navidrome::Controller, plex:crate::windows_plex::Controller, settings:super::library::Library, observer:F) -> Result<Self> {
        let (sender, receiver) = super::admission::channel::<Message>();
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = stopping.clone();
        let intent = Arc::new(AtomicU64::new(0));
        let worker_intent = intent.clone();
        let source = Arc::new(super::source::View::default());
        let worker_source = source.clone();
        let system_media = Arc::new(crate::windows_system_media::View::default());
        let worker_system_media = system_media.clone();
        let output = Arc::new(crate::windows_output::View::default());
        let worker_output = output.clone();
        let analyzer=Arc::new(crate::windows_shared::analyzer::View::default());
        let worker_analyzer=analyzer.clone();
        let mut history_publisher = history.publisher();
        let worker_navidrome=navidrome.clone();
        let worker_plex=plex.clone();
        let worker_settings=settings.clone();
        let audio_settings=crate::windows_audio::Store::new(root.parent().ok_or("runtime_directory_failed")?.join("user-data/windows-audio-v1.json"));
        let worker_audio=audio_settings.clone();
        let direct_context=Arc::new(std::sync::Mutex::new(None));let worker_direct_context=direct_context.clone();
        let audio_admission=Arc::new(crate::windows_audio::admission::Gate::default());let worker_audio_admission=audio_admission.clone();
        let eq_intervention=Arc::new(AtomicU64::new(0));let worker_eq_intervention=eq_intervention.clone();
        let ready=Arc::new(AtomicBool::new(false));let worker_ready=ready.clone();
        let config_revision=Arc::new(AtomicU64::new(0));let worker_config_revision=config_revision.clone();
        std::thread::Builder::new()
            .name("sonante-windows-mpd".into())
            .spawn(move || {
                let startup_clock=std::time::Instant::now();
                let _source_lease = super::source::ClearOnDrop(worker_source.clone());
                let _media_lease = crate::windows_system_media::ClearOnDrop(worker_system_media.clone());
                let _output_lease = crate::windows_output::ClearOnDrop(worker_output.clone());
                let _analyzer_lease=crate::windows_shared::analyzer::ClearOnDrop(worker_analyzer.clone());
                // Restoring an enabled account opens no playback listener and
                // performs no HTTP request. A vault failure does not stop Local.
                if let Ok(sources)=worker_settings.sources() {
                    if let Err(code)=tauri::async_runtime::block_on(worker_navidrome.apply_enabled_from_config(sources.navidrome)) {eprintln!("Windows Navidrome startup: {}",crate::windows_navidrome::playback_error(&code));}
                    if let Err(code)=tauri::async_runtime::block_on(worker_plex.apply_enabled_from_config(sources.plex)){eprintln!("Windows Plex startup: {}",crate::windows_plex::playback_error(&code));}
                }
                let mut journal = Journal::open_chain(queue_file, legacy_queue);
                worker_ready.store(true,Ordering::Release);
                crate::local_diagnostics::record(crate::local_diagnostics::Stage::StartupReady,startup_clock.elapsed(),None);
                let mut current: Option<Session> = None;
                let mut generation = 0u64;
                let logical_state=std::cell::Cell::new("pause");
                let eq_failure=std::cell::RefCell::new(None::<crate::windows_dsp::Error>);
                let eq_revision=std::cell::Cell::new(0u64);
                let eq_transition=std::cell::Cell::new(None::<&'static str>);
                let mut mute = super::mute::Memory::default();
                // Unlike queue generation, this changes only with volume ownership.
                let mut mute_owner = 0u64;
                let observation = super::observation::Clock::default();
                let snapshot = |current: &mut Option<Session>,
                                generation,
                                journal: &mut Journal,
                                force|
                 -> Result<Snapshot> {
                    let mut options = None;
                    let mut queue_version = None;
                    let (state, file_name, running, mut queue, index, album_id) =
                        if let Some(session) = current.as_mut() {
                            let state = session.observe()?;
                            options = session.options;
                            queue_version = Some(session.queue_version);
                            journal.observe_checkpoint(session.saved(&state), force);
                            (
                                state,
                                session.queue.get(session.current_index).map(|track|track.file_name.clone()),
                                true,
                                session.queue.clone(),
                                session.current_index,
                                session.album_id.clone(),
                            )
                        } else if let Some(saved) = &journal.saved {
                            let queue = saved.tracks();
                            (
                                Playback {
                                    state: "stop".into(),
                                    elapsed: saved.elapsed,
                                    duration: saved.duration,
                                    volume: saved.volume,
                                    audio: String::new(),
                                },
                                Some(queue[saved.index].file_name.clone()),
                                false,
                                queue,
                                saved.index,
                                saved.album_id.clone(),
                            )
                        } else {
                            (
                                Playback {
                                    state: "idle".into(),
                                    elapsed: 0.0,
                                    duration: 0.0,
                                    volume: 50,
                                    audio: String::new(),
                                },
                                None,
                                false,
                                Vec::new(),
                                0,
                                None,
                            )
                        };
                    worker_navidrome.contextualize(&mut queue);
                    let audio_settings=match current.as_ref(){Some(s)=>s.audio_settings.clone(),None=>worker_audio.load()?.settings};
                    let direct=current.as_ref().and_then(|s|s.direct.as_ref()).map(|p|crate::windows_audio::direct_snapshot(p,state.volume));
                    let mut equalizer=if let Some(dsp)=current.as_mut().and_then(|session|session.dsp.as_mut()){
                        dsp.snapshot(state.state=="play")?
                    }else{crate::windows_dsp::Snapshot::default()};
                    if equalizer.in_path!=Some(true){equalizer.in_path=Some(false);equalizer.error=*eq_failure.borrow();}
                    equalizer.revision=eq_revision.get();
                    equalizer.transition=if state.state!="play"{eq_transition.get().map(str::to_owned)}else{None};
                    let route_dsp = match equalizer.phase {
                        crate::windows_dsp::Phase::Off => crate::audio_route::RouteDspState::Off,
                        crate::windows_dsp::Phase::Preparing => crate::audio_route::RouteDspState::Preparing,
                        crate::windows_dsp::Phase::Ready => crate::audio_route::RouteDspState::Ready,
                        crate::windows_dsp::Phase::Active if equalizer.verified && equalizer.in_path == Some(true) => crate::audio_route::RouteDspState::ActiveVerified,
                        crate::windows_dsp::Phase::Active => crate::audio_route::RouteDspState::ActiveUnverified,
                        crate::windows_dsp::Phase::Updating => crate::audio_route::RouteDspState::Updating,
                        crate::windows_dsp::Phase::Stopping => crate::audio_route::RouteDspState::Stopping,
                        crate::windows_dsp::Phase::Error => crate::audio_route::RouteDspState::Failed,
                    };
                    let interpreted = crate::audio_route::interpret_legacy_route(
                        &audio_settings.audio_output_type,
                        &audio_settings.alsa_device,
                    );
                    let route_operation_id = if route_dsp != crate::audio_route::RouteDspState::Off {
                        (equalizer.revision > 0).then(|| format!("windows-dsp:{}", equalizer.revision))
                    } else if running {
                        Some(format!("windows-session:{generation}"))
                    } else {
                        None
                    };
                    let route_generation = observation.current().parse().unwrap_or(u64::MAX);
                    let audio_route = crate::audio_route::project_audio_route(
                        crate::audio_route::RouteProjectionInput {
                            persisted: Some(interpreted.clone()),
                            applied: running.then_some(interpreted.route),
                            supervisor: if running {
                                crate::audio_route::RouteSupervisorState::Available
                            } else {
                                crate::audio_route::RouteSupervisorState::Unavailable
                            },
                            playback_consistent: direct.as_ref().is_none_or(|value| value.consistent),
                            playing: running && state.state == "play",
                            // Session and volume policy do not prove a WASAPI
                            // exclusive stream; this adapter has no PCM probe.
                            direct_pcm_observed: false,
                            dsp: route_dsp,
                            operation: if matches!(route_dsp,
                                crate::audio_route::RouteDspState::Preparing
                                    | crate::audio_route::RouteDspState::Ready
                                    | crate::audio_route::RouteDspState::Updating
                                    | crate::audio_route::RouteDspState::Stopping)
                            { crate::audio_route::RouteOperationState::Transitioning }
                            else if route_dsp == crate::audio_route::RouteDspState::Failed
                                || route_dsp == crate::audio_route::RouteDspState::ActiveUnverified
                            { crate::audio_route::RouteOperationState::Uncertain }
                            else { crate::audio_route::RouteOperationState::Stable },
                            diagnostic: None,
                            operation_id: route_operation_id,
                            generation: route_generation,
                            platform: crate::audio_route::RoutePlatform::Windows,
                            null_output_test: false,
                        },
                    );
                    Ok(Snapshot {
                        config_save_response:None,
                        ab_watch:None,
                        observation: observation.current(),
                        generation,
                        engine_running: running,
                        state: state.state,
                        file_name,
                        queue_durations: worker_settings.queue_durations(&queue),
                        queue,
                        queue_index: index,
                        album_id,
                        elapsed: state.elapsed,
                        duration: state.duration,
                        volume: state.volume,
                        audio: state.audio,
                        runtime_version: runtime_version(),
                        warning: journal.warning,
                        options,
                        queue_revision: queue_edit::revision(generation, queue_version),
                        account_status:None,
                        audio_settings,
                        logical_state:logical_state.get().into(),
                        direct,
                        equalizer,
                        audio_route,
                    })
                };
                while let Ok(envelope) = receiver.recv() {
                    // Retain admission through command execution and the ACK,
                    // even when the caller has dropped its reply receiver.
                    let (_admission, message) = envelope.into_parts();
                    // Read-only polling stays on the same serialized worker,
                    // but is not a route transaction that should refuse gain.
                    // A context ticket still catches a pending mutation ahead.
                    if let Operation::Upnp(request,guard,_)=&message.operation {
                        let valid=guard()&&match request{crate::windows_upnp::Request::Action{id:Some(id),..}=>current.as_ref().is_some_and(|s|crate::windows_upnp::media::require(&s.queue,id).is_ok()),_=>true};
                        if !valid{let _=message.reply.send(Err("UPNP_SESSION_CHANGED"));continue;}
                    }
                    let _audio_operation=if matches!(&message.operation,Operation::Status){None}else{
                        match worker_audio_admission.operation(){Ok(guard)=>Some(guard),Err(code)=>{if message.reply.send(Err(code)).is_err(){eprintln!("Windows audio: operation_receiver_closed");}continue;}}
                    };
                    if let Operation::Remote(_,_,guard,_,_,_)=&message.operation {
                        if !guard(){let _=message.reply.send(Err("REMOTE_CHANGED"));continue;}
                    }
                    if let Err(code) = observation.advance(matches!(message.operation, Operation::Shutdown)) {
                        if message.reply.send(Err(code)).is_err() {
                            eprintln!("Windows MPD: operation_receiver_closed");
                        }
                        continue;
                    }
                    if matches!(message.operation, Operation::Shutdown) {
                        let checkpoint = if let Some(session) = current.as_mut() {
                            session
                                .checkpoint()
                                .map(|value| journal.observe_checkpoint(value, true))
                        } else {
                            Ok(())
                        };
                        journal.flush();
                        let cleanup = if let Some(mut session) = current.take() {
                            session.abort()
                        } else {
                            Ok(())
                        };
                        if let Err(code) = cleanup {
                            eprintln!("Windows MPD shutdown cleanup: {code}");
                        }
                        let result = checkpoint
                            .and(cleanup)
                            .and(journal.warning.map_or(Ok(()), Err))
                            .and_then(|_| snapshot(&mut current, generation, &mut journal, false));
                        if message.reply.send(result).is_err() {
                            eprintln!("Windows MPD: shutdown_receiver_closed");
                        }
                        break;
                    }
                    if !matches!(&message.operation,Operation::Status|Operation::Gain(..)|Operation::Action(_,Action::Volume{..}|Action::Mute{..},_,_)|Operation::Remote(_,crate::remote::Command::Volume{..},_,_,_,_)){
                        worker_audio_admission.changed();
                    }
                    if !matches!(&message.operation,Operation::Status|Operation::CompareEqualizer(..)){
                        // Gesture admission also invalidates immediately outside
                        // this worker. Recheck at execution for a comparison that
                        // was requested during source/candidate preparation.
                        worker_eq_intervention.fetch_add(1,Ordering::AcqRel);
                    }
                    let upnp_guard=match &message.operation{Operation::Upnp(_,guard,_)=>Some(guard.clone()),_=>None};
                    let remote_guard=match &message.operation{Operation::Remote(_,_,guard,_,_,_)=>Some(guard.clone()),_=>None};
                    let ab_pause=match &message.operation{Operation::PauseAB(intent,_)=>Some((*intent,worker_eq_intervention.load(Ordering::Acquire))),_=>None};
                    let queue_diagnostic=matches!(&message.operation,Operation::Load(..)|Operation::Enqueue(..)|Operation::Edit(..));
                    let queue_clock=std::time::Instant::now();
                    let result = (|| -> Result<Snapshot> {
                        if worker_stopping.load(Ordering::Acquire) {
                            return Err("shutting_down");
                        }
                        let operation=match message.operation{
                            Operation::Upnp(request,guard,admitted)=>{
                                if !guard()||worker_settings.observed_network_scope().is_none_or(|s|!s.0){return Err("UPNP_SESSION_CHANGED");}
                                match request {
                                    crate::windows_upnp::Request::Prepare(selected)=>{
                                        if admitted!=Some(worker_intent.load(Ordering::Acquire)){return Err("PLAYBACK_SUPERSEDED");}
                                        if journal.warning.is_some(){return Err("PLAYBACK_PERSISTENCE_FAILED");}
                                        crate::windows_upnp::media::admit(&selected)?;
                                        let volume=current.as_mut().map(|s|s.observe().map(|v|v.volume)).transpose()?.unwrap_or_else(||journal.saved.as_ref().map_or(50,|s|s.volume));
                                        let mut audio=worker_audio.load()?;
                                        if let Some(old)=current.as_ref(){
                                            if !old.audio_settings.same_route(&audio.settings){return Err("windows_audio_device_changed");}
                                            if let Some(policy)=&old.direct{policy.check_active().map_err(crate::windows_audio::policy_error)?;audio.settings.direct_muted=crate::windows_audio::direct_snapshot(policy,volume).muted;}
                                        }
                                        let mut candidate=Session::prepare_with_eq(&program,&root,selected,volume,worker_stopping.clone(),&audio,current.as_ref().and_then(|s|s.dsp.as_ref()).map(|d|d.applied().clone()))?;
                                        if let(Some(next),Some(old))=(candidate.dsp.as_mut(),current.as_ref().and_then(|s|s.dsp.as_ref())){next.inherit_render(old)?;}
                                        if let Some(options)=current.as_ref().and_then(|s|s.options).or_else(||journal.saved.as_ref().and_then(|s|s.options)){options::restore(options,|c|candidate.send(c))?;}
                                        let outcome=crate::windows_upnp::stopped::prepare(&mut candidate,current.as_mut(),||guard()&&admitted==Some(worker_intent.load(Ordering::Acquire))&&!worker_stopping.load(Ordering::Acquire),||journal.replace_logical(None));
                                        if outcome.retired_previous{current.take();generation+=1;mute_owner+=1;logical_state.set("pause");}
                                        outcome.result?;
                                        current=Some(candidate);generation+=1;mute_owner+=1;logical_state.set("stop");
                                        return snapshot(&mut current,generation,&mut journal,true);
                                    }
                                    crate::windows_upnp::Request::Action{id,action}=>{
                                        let fresh=snapshot(&mut current,generation,&mut journal,false)?;
                                        if let Some(id)=&id{crate::windows_upnp::media::require(&fresh.queue,id)?;}
                                        if !guard(){return Err("UPNP_SESSION_CHANGED");}
                                        Operation::Action(generation,action,admitted,None)
                                    }
                                }
                            }
                            Operation::Remote(expected,command,guard,admitted,prepared,volume_context)=>{
                                if expected!=generation||!guard(){return Err("REMOTE_CHANGED");}
                                if volume_context.is_some_and(|v|!worker_audio_admission.accepts(v)){return Err("engine_busy");}
                                if worker_settings.observed_network_scope()!=Some((true,true)){return Err("REMOTE_CHANGED");}
                                let fresh=snapshot(&mut current,generation,&mut journal,false)?;
                                let action=crate::windows_remote::model::resolve(&command,&fresh.remote_frame())?;
                                if !guard(){return Err("REMOTE_CHANGED");}
                                Operation::Action(expected,action,admitted,prepared)
                            }
                            Operation::PauseAB(admitted,revision)=>{
                                if admitted!=worker_intent.load(Ordering::Acquire)||revision%2!=0||worker_config_revision.load(Ordering::Acquire)!=revision{return Err("AB_CHANGED");}
                                worker_settings.edition_scope().map_err(|_|"AB_CHANGED")?;
                                let configured=worker_audio.load()?.settings;
                                if configured.audio_output_type!="pipewire"||current.as_ref().is_some_and(|s|s.audio_settings.audio_output_type!="pipewire"){return Err("AB_SHARED_ONLY");}
                                if current.is_none(){return snapshot(&mut current,generation,&mut journal,false);}
                                // Reuse exact existing radio/EQ/terminal pause semantics;
                                // no new engine, queue load, seek or automatic resume.
                                Operation::Action(generation,Action::SetPlaying{want_play:false},Some(admitted),None)
                            }
                            Operation::Gain(context,expected,action)=>{
                                if !worker_audio_admission.accepts(context){return Err("engine_busy");}
                                if matches!(&action,Action::Mute{..})&&expected.is_some_and(|value|value!=generation){return Err("SHARED_MUTE_CONTEXT_CHANGED");}
                                Operation::Action(expected.unwrap_or(generation),action,None,None)
                            }
                            Operation::StopRadio(id,admitted)=>{
                                if admitted.is_some_and(|v|v!=worker_intent.load(Ordering::Acquire)){return Err("PLAYBACK_SUPERSEDED");}
                                let session=current.as_ref().ok_or("RADIO_STATION_CHANGED")?;
                                match super::radio::stop_action(session.queue.get(session.current_index),&id,session.dsp.is_some())?{
                                    Some(action)=>Operation::Action(generation,action,admitted,None),
                                    None=>Operation::StopRadio(id,admitted),
                                }
                            }
                            other=>other,
                        };
                        match operation {
                            Operation::Account(change,admitted) => {
                                if admitted!=worker_intent.load(Ordering::Acquire){return Err("PLAYBACK_SUPERSEDED");}
                                let remote=current.as_ref().is_some_and(|session|session.queue.iter().any(|track|matches!(track.remote_source,Some(super::queue_v3::Source::Navidrome{..}))));
                                if remote {
                                    let saved=current.as_mut().ok_or("no_track")?.checkpoint()?;
                                    journal.replace_logical(saved)?;
                                }
                                let account=tauri::async_runtime::block_on(async {
                                    match change {AccountChange::Connect(prepared)=>worker_navidrome.commit_account(prepared).await,AccountChange::Disconnect=>worker_navidrome.disconnect().await}
                                }).map_err(|e|crate::windows_navidrome::playback_error(&e))?;
                                if remote {
                                    let session=current.as_mut().ok_or("no_track")?;
                                    if session.abort().is_err(){session.uncertain=true;return Err("windows_config_uncertain");}
                                    drop(current.take());generation+=1;mute_owner+=1;
                                }
                                let mut value=snapshot(&mut current,generation,&mut journal,false)?;value.account_status=Some(account);Ok(value)
                            }
                            Operation::ConfirmDirect(expected,admitted)=>{
                                if admitted!=worker_intent.load(Ordering::Acquire){return Err("PLAYBACK_SUPERSEDED");}
                                let mut value=worker_audio.load()?;if value.settings!=expected||!expected.direct(){return Err(crate::windows_audio::direct_policy::STALE);}
                                if current.as_ref().is_some_and(|s|!s.audio_settings.same_route(&value.settings)){return Err(crate::windows_audio::direct_policy::STALE);}
                                value.settings.direct_volume_choice=Some(crate::windows_audio::Choice{version:1,endpoint:expected.alsa_device,dop_enabled:expected.dop_enabled});
                                worker_audio.save(&value)?;if let Some(s)=current.as_mut(){s.audio_settings.direct_volume_choice=value.settings.direct_volume_choice.clone();}
                                snapshot(&mut current,generation,&mut journal,false)
                            }
                            Operation::Configure(roots,sources,mut plex_account,audio,complete_setup,experimental,online,mode,admitted) => {
                                if admitted!=worker_intent.load(Ordering::Acquire){return Err("PLAYBACK_SUPERSEDED");}
                                sources.validate()?;
                                if complete_setup&&worker_settings.first_run()?&&current.is_some(){return Err("engine_busy");}
                                let before=worker_settings.sources()?;let before_roots=worker_settings.configured_roots()?;
                                let owner=worker_settings.imports.admit(sources.local&&(roots.is_some()||!before.local),before_roots.is_empty())?;
                                let configured=(|| -> Result<Snapshot> {
                                worker_config_revision.fetch_add(1,Ordering::AcqRel);let _phase=ConfigPhase(worker_config_revision.clone());
                                let before_experimental=worker_settings.experimental_features()?;let next_experimental=experimental.unwrap_or(before_experimental);
                                let before_online=worker_settings.online_artwork_enabled()?;let next_online=online.unwrap_or(before_online);
                                let before_mode=worker_settings.usage_mode()?;let next_mode=mode.unwrap_or(before_mode);
                                let before_audio=worker_audio.load()?;let mut next_audio=before_audio.clone();
                                if let Some(mut audio)=audio{
                                    audio.validate()?;audio.require_choice()?;
                                    // A settings draft cannot undo an intentional mute
                                    // admitted after the modal was opened.
                                    audio.direct_muted=before_audio.settings.direct_muted;
                                    next_audio.settings=audio;
                                }
                                let output_changed=!before_audio.settings.same_route(&next_audio.settings);
                                if output_changed&&next_audio.settings.direct(){crate::windows_audio::devices::resolve(&next_audio.settings.alsa_device)?;}
                                if output_changed{
                                    if let Some(s)=current.as_mut(){
                                        let observed=s.observe()?;
                                        if !s.audio_settings.direct(){next_audio.shared_volume=observed.volume;}
                                        else if let Some(direct)=&s.direct{if crate::windows_audio::direct_snapshot(direct,observed.volume).gain==Some(crate::windows_audio::direct_policy::Gain::Muted){next_audio.settings.direct_muted=true;}}
                                    }else if !before_audio.settings.direct(){
                                        // A paused startup has no engine. Its confirmed journal
                                        // volume is still Shared's value, never Direct unity.
                                        if let Some(saved)=journal.saved.as_ref(){next_audio.shared_volume=saved.volume;}
                                    }
                                }
                                let account_changed=plex_account.is_some();
                                let stopping_source=current.as_ref().is_some_and(|session|super::transport::enabled(&session.queue,sources).is_err()||(account_changed&&session.queue.iter().any(|track|matches!(track.remote_source,Some(super::queue_v3::Source::Plex{..})))));
                                let stopping_source=stopping_source||output_changed&&current.is_some();
                                let old_logical=journal.saved.clone();
                                let route_state=if output_changed{current.as_mut().map(|s|s.observe().map(|p|if p.state=="stop"{"stop"}else{"pause"})).transpose()?.unwrap_or(logical_state.get())}else{logical_state.get()};
                                let checkpoint_changed=stopping_source||output_changed&&journal.saved.is_some();
                                if checkpoint_changed {
                                    let mut saved=match current.as_mut(){Some(s)=>s.checkpoint()?,None=>journal.saved.clone()};
                                    if output_changed&&!next_audio.settings.direct(){if let Some(q)=saved.as_mut(){q.volume=next_audio.shared_volume;}}
                                    journal.replace_logical(saved)?;
                                }
                                let mut first_store=true;
                                let result=super::config_transaction::with_audio(&before_audio,&next_audio,|a|worker_audio.save(a),||super::config_transaction::apply_all_options((before_roots,before,(before_experimental,before_online,before_mode)),roots,sources,(next_experimental,next_online,next_mode),account_changed,
                                    |roots,flags,(experimental,online,mode)|{let bind=if first_store {first_store=false;Some(&owner)}else{None};worker_settings.configure_import_preferences(roots,flags,Some(experimental),Some(online),Some(mode),bind).map(|_|())},
                                    |enabled|tauri::async_runtime::block_on(worker_navidrome.apply_enabled_from_config(enabled)).map(|_|()).map_err(|e|crate::windows_navidrome::playback_error(&e)),
                                    |enabled|tauri::async_runtime::block_on(worker_plex.configure(enabled,plex_account.take())).map(|_|()).map_err(|e|crate::windows_plex::playback_error(&e)),
                                    ||current.as_ref().is_none_or(|session|session._media_lock.iter().all(|lease|lease.healthy()))));
                                if let Err(mut error)=result {
                                    if checkpoint_changed&&journal.replace_logical(old_logical).is_err(){error=super::config_transaction::UNCERTAIN;}
                                    if error==super::config_transaction::UNCERTAIN {if let Some(session)=current.as_mut(){session.uncertain=true;if let Err(code)=session.abort(){eprintln!("Windows source rollback cleanup: {code}");}}}
                                    return Err(error);
                                }
                                if complete_setup{worker_settings.complete_first_run()?;}
                                if stopping_source {
                                    let session=current.as_mut().ok_or("no_track")?;
                                    if session.abort().is_err(){session.uncertain=true;return Err("windows_config_uncertain");}
                                    drop(current.take());generation+=1;mute_owner+=1;
                                }
                                if output_changed{logical_state.set(route_state);}
                                if let Some(s)=current.as_mut(){s.audio_settings=next_audio.settings;}
                                snapshot(&mut current,generation,&mut journal,false)
                                })();
                                match configured {
                                    Ok(mut snapshot)=>{snapshot.config_save_response=Some(owner.commit()?);Ok(snapshot)}
                                    Err(code)=>{owner.fail(code);Err(code)}
                                }
                            }
                            Operation::Recover => {
                                let checkpoint = if let Some(session) = current.as_mut() {
                                    session
                                        .checkpoint()
                                        .map(|value| journal.observe_checkpoint(value, true))
                                } else {
                                    Ok(())
                                };
                                if let Some(session) = current.as_mut() {
                                    session.abort()?;
                                }
                                drop(current.take());
                                generation += 1;
                                mute_owner += 1;
                                journal.flush();
                                if checkpoint.is_err() && journal.warning.is_none() {
                                    journal.warning = Some("queue_checkpoint_failed");
                                }
                                snapshot(&mut current, generation, &mut journal, false)
                            }
                            Operation::SleepStop(admitted) => {
                                if admitted != worker_intent.load(Ordering::Acquire) {
                                    return Err("PLAYBACK_SUPERSEDED");
                                }
                                // A quiet logical queue must never create an engine or
                                // reopen media merely because its timer has expired.
                                if let Some(session)=current.as_mut().filter(|s|s.dsp.is_some()){
                                    let stopped=super::sleep::close_terminal(session,Session::checkpoint,Session::abort);
                                    if stopped.as_ref().err().is_some_and(|failure|!failure.terminated){
                                        session.uncertain=true;
                                        return Err("windows_config_uncertain");
                                    }
                                    drop(current.take());generation+=1;mute_owner+=1;logical_state.set("stop");
                                    let mut saved=stopped.map_err(|failure|failure.code)?;
                                    if let Some(saved)=saved.as_mut(){saved.elapsed=0.;}
                                    journal.replace_logical(saved)?;
                                    return snapshot(&mut current,generation,&mut journal,false);
                                }
                                if let Some(session) = current.as_mut() {
                                    if let Err(failure) = super::sleep::stop(session) {
                                        if failure.terminated {
                                            drop(current.take());
                                            generation += 1;
                                            mute_owner += 1;
                                        }
                                        // Keep the last confirmed journal on failure.
                                        return Err(failure.code);
                                    }
                                    let value =
                                        snapshot(&mut current, generation, &mut journal, true)?;
                                    if journal.warning.is_some() {
                                        return Err("PLAYBACK_PERSISTENCE_FAILED");
                                    }
                                    Ok(value)
                                } else {
                                    snapshot(&mut current, generation, &mut journal, false)
                                }
                            }
                            Operation::Equalizer(preset,bypass_expected,prepared,expected_generation,admitted)=>{
                                if preset.is_some()&&crate::windows_dsp::CAMILLA_SHA.is_none(){return Err("EQ_RUNTIME_UNTRUSTED");}
                                eq_transition.set(None);
                                eq_revision.set(eq_revision.get().checked_add(1).ok_or("EQ_CANCELLED")?);
                                if expected_generation!=generation||admitted!=worker_intent.load(Ordering::Acquire){return Err("PLAYBACK_SUPERSEDED");}
                                let audio=worker_audio.load()?;
                                if audio.settings.direct(){return Err("EQ_SHARED_ONLY");}
                                // The approved Shared MPD has no owned rendered
                                // cursor/drain attestation (its WASAPI Drain has
                                // an upstream hardware TODO). Do not cut its
                                // retained buffers and claim a continuous handoff.
                                if preset.is_some(){if let Some(old)=current.as_mut().filter(|s|s.dsp.is_none()){
                                    if matches!(old.observe()?.state.as_str(),"play"|"pause"){return Err("EQ_SHARED_SWITCH_UNCONFIRMED");}
                                }}
                                if let Some(preset)=preset.as_ref(){
                                    crate::equalizer::validate_v1(preset).map_err(|_|"EQ_PRESET_INVALID")?;
                                    crate::windows_dsp::safety::assess(preset,48000).map_err(|_|"EQ_UNSAFE_HEADROOM")?;
                                    if let Some(chain)=current.as_mut().and_then(|session|session.dsp.as_mut()){
                                        if matches!(chain.health()?,crate::windows_dsp::chain::Health::Ready|crate::windows_dsp::chain::Health::Active){
                                            chain.apply(preset.clone())?;*eq_failure.borrow_mut()=None;
                                            return snapshot(&mut current,generation,&mut journal,false);
                                        }
                                    }
                                }else{
                                    let Some(chain)=current.as_ref().and_then(|session|session.dsp.as_ref())else{return snapshot(&mut current,generation,&mut journal,false)};
                                    if bypass_expected.as_ref()!=Some(chain.applied()){return Err("EQ_BYPASS_LEVEL_RETURN_CONFIRMATION_REQUIRED");}
                                }
                                // Resolve every media dependency and start both owned
                                // engines before freezing/cutting the existing route.
                                if current.is_none(){if let Some(code)=journal.warning{return Err(code);}}
                                let (mut selection,mut before)=if let Some(old)=current.as_mut(){
                                    let before=old.observe()?;
                                    (Selection{root:old.music_root.clone(),tracks:old.queue.clone(),start:old.current_index,album_id:old.album_id.clone()},before)
                                }else if let Some(saved)=journal.saved.as_ref(){
                                    (saved.selection(),Playback{state:logical_state.get().into(),elapsed:saved.elapsed,duration:saved.duration,volume:saved.volume,audio:String::new()})
                                }else{
                                    (Selection{root:PathBuf::new(),tracks:Vec::new(),start:0,album_id:None},Playback{state:"idle".into(),elapsed:0.,duration:0.,volume:audio.shared_volume,audio:String::new()})
                                };
                                if prepared.len()!=selection.tracks.len(){return Err("PLAYBACK_SUPERSEDED");}
                                for (track,ready) in selection.tracks.iter_mut().zip(prepared){
                                    if track.id!=ready.id||track.path!=ready.path||track.media_root!=ready.media_root||track.remote_source!=ready.remote_source{return Err("PLAYBACK_SUPERSEDED");}
                                    track.remote_lease=ready.remote_lease;track.remote_connection_id=ready.remote_connection_id;
                                }
                                super::transport::enabled(&selection.tracks,worker_settings.sources()?)?;
                                let was_playing=before.state=="play";
                                if selection.tracks.is_empty()&&preset.is_none(){
                                    if let Some(mut old)=current.take(){old.abort()?;}
                                    generation+=1;logical_state.set("pause");*eq_failure.borrow_mut()=None;
                                    return snapshot(&mut current,generation,&mut journal,false);
                                }
                                let previous_options=current.as_ref().and_then(|s|s.options).or_else(||journal.saved.as_ref().and_then(|s|s.options));
                                let mut candidate=Session::prepare_with_eq(&program,&root,selection,before.volume,worker_stopping.clone(),&audio,preset)?;
                                if let Some(target)=previous_options{options::restore(target,|command|candidate.send(command))?;}
                                if admitted!=worker_intent.load(Ordering::Acquire){return Err("PLAYBACK_SUPERSEDED");}
                                if let Some(old)=current.as_mut(){
                                    if was_playing {
                                        // EQ generations are held without Stop/Cancel;
                                        // Shared is paused without seeking on rollback.
                                        if old.dsp.is_some(){old.hold_for_replacement()?;}else{old.pause()?;}
                                    }
                                    before=old.observe()?;
                                    candidate.current_index=old.current_index;
                                    if let Some(saved)=old.saved(&before){
                                        // Failure to persist leaves the old route held;
                                        // no repeat/seek/autoplay is used to hide the error.
                                        journal.replace_logical(Some(saved))?;
                                    }
                                }
                                candidate.stage(&before)?;
                                if admitted!=worker_intent.load(Ordering::Acquire){return Err("PLAYBACK_SUPERSEDED");}
                                if let Some(chain)=current.as_mut().and_then(|session|session.dsp.as_mut()){chain.drain_held()?;}
                                // Once cut, failure falls back to the confirmed logical
                                // queue WITHOUT EQ and PAUSED, matching Linux policy.
                                if let Some(old)=current.as_mut(){old.abort()?;}
                                current.take();generation+=1;mute_owner+=1;logical_state.set(if before.state=="stop"{"stop"}else{"pause"});
                                current=Some(candidate);*eq_failure.borrow_mut()=None;
                                // Source cursor is a logical MPD position, not
                                // a client/DAC render clock. Until that handoff
                                // is qualified, route changes stay paused. Live
                                // preset/compare above retain the existing route.
                                eq_transition.set(Some(if matches!(before.state.as_str(),"stop"|"idle"){"stopped"}else{"paused"}));
                                snapshot(&mut current,generation,&mut journal,true)
                            }
                            Operation::CompareEqualizer(action,token,expected,watch,expected_generation)=>{
                                if generation!=expected_generation{return Err("EQ_COMPARISON_STALE");}
                                if worker_audio.load()?.settings.direct(){return Err("EQ_SHARED_ONLY");}
                                let session=current.as_mut().ok_or("EQ_COMPARISON_UNAVAILABLE")?;
                                let playing=session.observe()?.state=="play";
                                let chain=session.dsp.as_mut().ok_or("EQ_COMPARISON_UNAVAILABLE")?;
                                let old=chain.snapshot(playing)?;
                                crate::windows_dsp::compare(chain,&old,action,token,expected,Some(watch)).map_err(crate::windows_dsp::error_code)?;
                                eq_revision.set(eq_revision.get().checked_add(1).ok_or("EQ_CANCELLED")?);
                                snapshot(&mut current,generation,&mut journal,false)
                            }
                            Operation::Status => {
                                snapshot(&mut current, generation, &mut journal, false)
                            }
                            Operation::StopRadio(id, admitted) => {
                                if admitted.is_some_and(|v| v != worker_intent.load(Ordering::Acquire)) { return Err("PLAYBACK_SUPERSEDED"); }
                                let session = current.as_mut().ok_or("RADIO_STATION_CHANGED")?;
                                super::radio::require_station(session.queue.get(session.current_index), &id)?;
                                session.stop()?;
                                snapshot(&mut current, generation, &mut journal, true)
                            }
                            Operation::Load(selected, admitted) => {
                                let load_intent=admitted.unwrap_or_else(||worker_intent.load(Ordering::Acquire));
                                if admitted
                                    .is_some_and(|v| v != worker_intent.load(Ordering::Acquire))
                                {
                                    return Err("PLAYBACK_SUPERSEDED");
                                }
                                // Readability, runtime hash, startup and indexing happen
                                // before stopping the previous source.
                                super::radio::admit(&selected, journal.warning)?;
                                super::transport::enabled(&selected.tracks,worker_settings.sources()?)?;
                                let volume = current
                                    .as_mut()
                                    .map(|s| s.observe().map(|p| p.volume))
                                    .transpose()?
                                    .unwrap_or_else(|| {
                                        journal.saved.as_ref().map_or(50, |q| q.volume)
                                    });
                                let mut audio=worker_audio.load()?;
                                if let Some(session)=current.as_ref(){
                                    if !session.audio_settings.same_route(&audio.settings){return Err("windows_audio_device_changed");}
                                    if let Some(policy)=&session.direct{
                                        let observed=crate::windows_audio::direct_snapshot(policy,volume);
                                        policy.check_active().map_err(crate::windows_audio::policy_error)?;
                                        // Session replacement must carry the actually owned
                                        // mute, even if an earlier persistence write failed.
                                        audio.settings.direct_muted=observed.gain==Some(crate::windows_audio::direct_policy::Gain::Muted);
                                    }
                                }
                                let mut candidate = Session::prepare_with_eq(
                                    &program,&root,selected,volume,worker_stopping.clone(),&audio,
                                    current.as_ref().and_then(|s|s.dsp.as_ref()).map(|dsp|dsp.applied().clone()),
                                )?;
                                if let (Some(next),Some(previous))=(candidate.dsp.as_mut(),current.as_ref().and_then(|s|s.dsp.as_ref())){next.inherit_render(previous)?;}
                                let previous_options = current
                                    .as_ref()
                                    .and_then(|s| s.options)
                                    .or_else(|| journal.saved.as_ref().and_then(|q| q.options));
                                if let Some(target) = previous_options {
                                    options::restore(target, |command| candidate.send(command))?;
                                }
                                if admitted
                                    .is_some_and(|v| v != worker_intent.load(Ordering::Acquire))
                                {
                                    return Err("PLAYBACK_SUPERSEDED");
                                }
                                if candidate.dsp.is_some(){transaction::activate_guarded(&mut candidate,current.as_mut(),||load_intent==worker_intent.load(Ordering::Acquire))?;}
                                else{transaction::activate(&mut candidate,current.as_mut())?;}
                                let warning = if let Some(mut old) = current.take() {
                                    old.abort().err()
                                } else {
                                    None
                                };
                                logical_state.set("pause");
                                current = Some(candidate);
                                generation += 1;
                                mute_owner += 1;
                                let mut result =
                                    snapshot(&mut current, generation, &mut journal, true)?;
                                result.warning = warning.or(result.warning);
                                Ok(result)
                            }
                            Operation::Enqueue(selected, placement, admitted) => {
                                super::transport::enabled(&selected.tracks,worker_settings.sources()?)?;
                                if admitted != worker_intent.load(Ordering::Acquire) {
                                    return Err("PLAYBACK_SUPERSEDED");
                                }
                                journal.flush();
                                if journal.warning.is_some_and(|w| {
                                    matches!(w, "queue_save_failed" | "queue_restore_failed")
                                }) {
                                    return Err("PLAYBACK_PERSISTENCE_FAILED");
                                }
                                if let Some(session) = current.as_mut() {
                                    session.enqueue(selected, placement, || {
                                        admitted == worker_intent.load(Ordering::Acquire)
                                            && !worker_stopping.load(Ordering::Acquire)
                                    })?;
                                    generation += 1;
                                    let result =
                                        snapshot(&mut current, generation, &mut journal, true)?;
                                    if result.warning.is_some() {
                                        return Err("PLAYBACK_PERSISTENCE_FAILED");
                                    }
                                    Ok(result)
                                } else {
                                    let value = if let Some(saved) = &journal.saved {
                                        saved.inserted(selected, placement)?
                                    } else {
                                        SavedQueue::capture(
                                            &selected.root,
                                            &selected.tracks,
                                            0,
                                            0.0,
                                            0.0,
                                            50,
                                            selected.album_id.clone(),
                                        )
                                    };
                                    if admitted != worker_intent.load(Ordering::Acquire)
                                        || worker_stopping.load(Ordering::Acquire)
                                    {
                                        return Err("PLAYBACK_SUPERSEDED");
                                    }
                                    journal.replace_logical(Some(value))?;
                                    generation += 1;
                                    snapshot(&mut current, generation, &mut journal, false)
                                }
                            }
                            Operation::Edit(revision, action, admitted) => {
                                if admitted != worker_intent.load(Ordering::Acquire) {
                                    return Err("PLAYBACK_SUPERSEDED");
                                }
                                journal.flush();
                                if journal.warning.is_some_and(|w| {
                                    matches!(w, "queue_save_failed" | "queue_restore_failed")
                                }) {
                                    return Err("PLAYBACK_PERSISTENCE_FAILED");
                                }
                                if let Some(session) = current.as_mut() {
                                    session.edit_queue(&revision, generation, action, || {
                                        admitted == worker_intent.load(Ordering::Acquire)
                                            && !worker_stopping.load(Ordering::Acquire)
                                    })?;
                                    generation += 1;
                                    if session.queue.is_empty() {
                                        mute_owner += 1;
                                        if session.dsp.is_some(){journal.observe_empty();session.staged=None;session.current_index=0;}
                                        else{super::queue_finish::confirmed_empty(&mut journal, &mut current, |session| session.abort())?;}
                                    }
                                    let result =
                                        snapshot(&mut current, generation, &mut journal, true)?;
                                    if result.warning.is_some() {
                                        return Err("PLAYBACK_PERSISTENCE_FAILED");
                                    }
                                    Ok(result)
                                } else {
                                    if revision != queue_edit::revision(generation, None) {
                                        return Err("QUEUE_EDIT_CHANGED");
                                    }
                                    let value =
                                        journal.saved.as_ref().ok_or("no_track")?.edited(action)?;
                                    if admitted != worker_intent.load(Ordering::Acquire)
                                        || worker_stopping.load(Ordering::Acquire)
                                    {
                                        return Err("PLAYBACK_SUPERSEDED");
                                    }
                                    let cleared = value.is_none();
                                    journal.replace_logical(value)?;
                                    generation += 1;
                                    if cleared { mute_owner += 1; }
                                    snapshot(&mut current, generation, &mut journal, false)
                                }
                            }

                            Operation::Action(expected, action, admitted, prepared) => {
                                if remote_guard.as_ref().is_some_and(|guard|!guard()){return Err("REMOTE_CHANGED");}
                                if upnp_guard.as_ref().is_some_and(|g|!g()){return Err("UPNP_SESSION_CHANGED");}
                                let eq_admitted=admitted.unwrap_or_else(||worker_intent.load(Ordering::Acquire));
                                if admitted
                                    .is_some_and(|v| v != worker_intent.load(Ordering::Acquire))
                                {
                                    return Err("PLAYBACK_SUPERSEDED");
                                }
                                if expected != generation {
                                    return Err("stale_request");
                                }
                                if matches!(action, Action::Release) {
                                    if let Some(session) = current.as_mut() {
                                        session.abort()?;
                                    }
                                    journal.clear()?;
                                    drop(current.take());
                                    generation += 1;
                                    mute_owner += 1;
                                    return snapshot(&mut current, generation, &mut journal, false);
                                }
                                if current.is_none() {
                                    let mut saved = journal.saved.clone().ok_or("no_track")?;
                                    let resume = match action {
                                        Action::Toggle | Action::SetPlaying { want_play: true } => {
                                            saved.elapsed
                                        }
                                        Action::SetPlaying { want_play: false } => {
                                            return snapshot(
                                                &mut current,
                                                generation,
                                                &mut journal,
                                                false,
                                            )
                                        }
                                        Action::Next | Action::Previous => {
                                            use super::navigation::{self,Direction,Step};
                                            let direction=if matches!(action,Action::Next){Direction::Next}else{Direction::Previous};
                                            let Step::Selected(selected)=navigation::step(direction,saved.index,saved.tracks().len(),"stop",true,saved.options)?else{return Err("queue_unconfirmed")};
                                            saved.index=selected.index();selected.elapsed()
                                        }
                                        Action::Jump { index } => {
                                            if index >= saved.tracks().len() {
                                                return Err("queue_boundary");
                                            }
                                            saved.index = index;
                                            0.0
                                        }
                                        Action::DirectMute {..}=>return Err(crate::windows_audio::direct_policy::STALE),
                                        Action::Volume { value } => {
                                            if worker_audio.load()?.settings.direct(){return Err(crate::windows_audio::direct_policy::GRADUAL);}
                                            if value > 100 {
                                                return Err("invalid_volume");
                                            }
                                            saved.volume = value;
                                            journal.replace_logical(Some(saved))?;
                                            return snapshot(
                                                &mut current,
                                                generation,
                                                &mut journal,
                                                false,
                                            );
                                        }
                                        Action::Mute { muted, token } => {
                                            if worker_audio.load()?.settings.direct(){return Err(crate::windows_audio::direct_policy::STALE);}
                                            if let Some(value) = mute.plan(generation, mute_owner, &token, saved.volume, muted)? {
                                                saved.volume = value;
                                                journal.replace_logical(Some(saved))?;
                                            }
                                            return snapshot(&mut current, generation, &mut journal, false);
                                        }
                                        Action::Stop => {
                                            saved.elapsed = 0.0;
                                            journal.replace_logical(Some(saved))?;
                                            return snapshot(
                                                &mut current,
                                                generation,
                                                &mut journal,
                                                false,
                                            );
                                        }
                                        Action::Seek { .. } | Action::ChangeOptions { .. } => {
                                            return Err("no_track")
                                        }
                                        Action::Release => unreachable!(),
                                    };
                                    let mut selection=saved.selection();
                                    super::transport::enabled(&selection.tracks,worker_settings.sources()?)?;
                                    if selection.tracks.iter().any(|track|track.remote_source.is_some()) {
                                        let prepared=prepared.ok_or("NAVIDROME_CONTEXT_CHANGED")?;
                                        if prepared.len()!=selection.tracks.len(){return Err("PLAYBACK_SUPERSEDED");}
                                        for (track,ready) in selection.tracks.iter_mut().zip(prepared) {
                                            if track.id!=ready.id || track.remote_source!=ready.remote_source || track.path!=ready.path || track.media_root!=ready.media_root {return Err("PLAYBACK_SUPERSEDED");}
                                            track.remote_lease=ready.remote_lease;track.remote_connection_id=ready.remote_connection_id;
                                        }
                                    }
                                    let mut candidate = Session::prepare(
                                        &program,
                                        &root,
                                        selection,
                                        saved.volume,
                                        worker_stopping.clone(),
                                        &worker_audio.load()?,
                                    )?;
                                    if let Some(target) = saved.options {
                                        options::restore(target, |command| {
                                            candidate.send(command)
                                        })?;
                                    }
                                    candidate.resume_elapsed = resume;
                                    // Cache/queue stay untouched if media validation or activation fails.
                                    if admitted
                                        .is_some_and(|v| v != worker_intent.load(Ordering::Acquire))
                                    {
                                        return Err("PLAYBACK_SUPERSEDED");
                                    }
                                    transaction::activate(&mut candidate, None)?;
                                    logical_state.set("pause");
                                    current = Some(candidate);
                                    generation += 1;
                                    mute_owner += 1;
                                    return snapshot(&mut current, generation, &mut journal, true);
                                }
                                let eq_plan=if let Some(session)=current.as_mut().filter(|session|session.dsp.is_some()){
                                    let state=session.observe()?;
                                    crate::windows_dsp::action::plan(&action,&state,session.current_index,session.queue.len(),session.is_radio(),session.options,session.eq_started)?
                                }else{None};
                                if let Some(plan)=eq_plan {
                                    let session=current.as_mut().ok_or("no_track")?;
                                    let before=session.observe()?;
                                    let previous_index=session.current_index;
                                    let selection=Selection{root:session.music_root.clone(),tracks:session.queue.clone(),start:plan.index,album_id:session.album_id.clone()};
                                    let preset=session.dsp.as_ref().ok_or("EQ_CANCELLED")?.applied().clone();
                                    let audio=worker_audio.load()?;
                                    let mut candidate=Session::prepare_with_eq(&program,&root,selection,before.volume,worker_stopping.clone(),&audio,Some(preset))?;
                                    candidate.dsp.as_mut().ok_or("EQ_CANCELLED")?.inherit_render(session.dsp.as_ref().ok_or("EQ_CANCELLED")?)?;
                                    if let Some(target)=session.options{options::restore(target,|command|candidate.send(command))?;}
                                    if plan.stop&&before.state=="play"{session.hold_for_replacement()?;}
                                    let frozen=session.observe()?;
                                    if !plan.stop&&session.current_index!=previous_index{return Err("stale_request");}
                                    if eq_admitted!=worker_intent.load(Ordering::Acquire)||upnp_guard.as_ref().is_some_and(|g|!g()){return Err("PLAYBACK_SUPERSEDED");}
                                    if plan.play {
                                        candidate.resume_elapsed=plan.elapsed;
                                        transaction::activate_committed(&mut candidate,Some(session),||eq_admitted==worker_intent.load(Ordering::Acquire)&&upnp_guard.as_ref().is_none_or(|g|g()),|next,observed|{
                                            if next.is_upnp(){return journal.replace_logical(None);}
                                            let saved=next.saved(observed).ok_or("PLAYBACK_PERSISTENCE_FAILED")?;
                                            journal.replace_logical(Some(saved))
                                        })?;
                                    }else{
                                        if frozen.state=="play"{session.pause()?;}
                                        let mut target=frozen;target.elapsed=plan.elapsed;target.state=if plan.stop{"stop".into()}else{"pause".into()};
                                        candidate.stage(&target)?;
                                    }
                                    // Playing cuts already committed while the old PCM
                                    // generation was recoverable. Paused/stopped cuts have
                                    // emitted zero PCM and never issue Play/Seek here.
                                    if !plan.play{
                                        let observed=candidate.observe()?;
                                        if eq_admitted!=worker_intent.load(Ordering::Acquire)||upnp_guard.as_ref().is_some_and(|g|!g()){candidate.abort()?;return Err("PLAYBACK_SUPERSEDED");}
                                        if let Some(saved)=candidate.saved(&observed){journal.replace_logical(Some(saved))?;}
                                    }
                                    if session.abort().is_err(){session.uncertain=true;return Err("PLAYBACK_QUEUE_UNCERTAIN");}
                                    current=Some(candidate);generation+=1;mute_owner+=1;
                                    logical_state.set(if plan.stop{"stop"}else{"pause"});
                                    return snapshot(&mut current,generation,&mut journal,true);
                                }
                                let session = current.as_mut().ok_or("no_track")?;
                                match action {
                                    Action::Toggle | Action::SetPlaying { .. } => {
                                        let state = session.observe()?;
                                        if let Action::SetPlaying { want_play } = action {
                                            if want_play == (state.state == "play") {
                                                return snapshot(
                                                    &mut current,
                                                    generation,
                                                    &mut journal,
                                                    true,
                                                );
                                            }
                                        }
                                        if state.state == "play" {
                                            session.pause()?;
                                        } else if session.staged.is_some() {
                                            session.start()?;
                                        } else if state.state == "pause" {
                                            session.prepare_direct_gain()?;
                                            session.send("pause 0")?;
                                        } else {
                                            session.start()?;
                                        }
                                    }
                                    Action::ChangeOptions { action, expected } => {
                                        options::change(action, expected, |command| {
                                            session.send(command)
                                        })?;
                                    }
                                    Action::Stop => session.stop()?,
                                    Action::Release => unreachable!(),
                                    Action::Next | Action::Previous => {
                                        use super::navigation::{self,Direction,Step};
                                        let direction=if matches!(action,Action::Next){Direction::Next}else{Direction::Previous};
                                        let observed=session.observe()?;
                                        if session.is_radio(){
                                            session.prepare_direct_gain()?;session.send(direction.command())?;
                                        }else{
                                            match navigation::step(direction,session.current_index,session.queue.len(),&observed.state,session.staged.is_some(),session.options)?{
                                                Step::Native(direction)=>{session.prepare_direct_gain()?;session.send(direction.command())?;},
                                                Step::Selected(selected)=>session.start_selected(selected)?,
                                            }
                                        }
                                    }
                                    Action::Jump { index } => {
                                        let selected=super::navigation::jump(index,session.queue.len())?;
                                        session.start_selected(selected)?;
                                    }
                                    Action::Seek { seconds, index } => {
                                        if session.is_radio() { return Err("RADIO_LIVE_NOT_SEEKABLE"); }
                                        let state = session.observe()?;
                                        if (session.queue.len() > 1
                                            && index != Some(session.current_index))
                                            || index.is_some_and(|i| i != session.current_index)
                                        {
                                            return Err("stale_request");
                                        }
                                        if !seconds.is_finite()
                                            || seconds < 0.0
                                            || state.duration <= 0.0
                                            || seconds > state.duration
                                            || !["play", "pause"].contains(&state.state.as_str())
                                        {
                                            return Err("invalid_seek");
                                        }
                                        session.seek(seconds)?;
                                    }
                                    Action::DirectMute {muted,token}=>{
                                        let direct=session.direct.clone().ok_or(crate::windows_audio::direct_policy::STALE)?;
                                        crate::windows_audio::direct_policy::Context::new(crate::windows_audio::projection::VolumeBackend::DirectEquipment,Some(direct.clone())).validate_token(&token).map_err(crate::windows_audio::policy_error)?;
                                        direct.set_mute(muted,session.audio_settings.confirmed(),&token,|c|session.policy_command(c)).map_err(crate::windows_audio::policy_error)?;
                                        let mut saved=worker_audio.load()?;if !saved.settings.same_route(&session.audio_settings){return Err(crate::windows_audio::direct_policy::STALE);}
                                        saved.settings.direct_muted=muted;session.audio_settings.direct_muted=muted;
                                        worker_audio.save(&saved)?;
                                    }
                                    Action::Mute { muted, token } => {
                                        if session.direct.is_some(){return Err(crate::windows_audio::direct_policy::STALE);}
                                        let volume = session.observe()?.volume;
                                        if let Some(value) = mute.plan(generation, mute_owner, &token, volume, muted)? {
                                            session.send(&format!("setvol {value}"))?;
                                        }
                                    }
                                    Action::Volume { value } => {
                                        if session.direct.is_some(){return Err(crate::windows_audio::direct_policy::GRADUAL);}
                                        if value > 100 {
                                            return Err("invalid_volume");
                                        }
                                        session.send(&format!("setvol {value}"))?;
                                    }
                                }
                                snapshot(&mut current, generation, &mut journal, true)
                            }
                            Operation::Upnp(..)|Operation::Remote(..)|Operation::Gain(..)|Operation::PauseAB(..)|Operation::Shutdown => unreachable!(),
                        }
                    })();
                    let result=result.and_then(|mut view|{
                        if let Some((admitted,intervention))=ab_pause {
                            let watch=crate::audio::EqTransitionWatch{intent:worker_intent.clone(),intent_value:admitted,intervention:worker_eq_intervention.clone(),intervention_value:intervention,stopping:worker_stopping.clone()};
                            view.ab_watch=Some(crate::windows_shared::ab_pause::confirm(watch,view.engine_running,&view.state)?);
                        }Ok(view)
                    });
                    if let Err(code)=&result{
                        // Observe the original static error without extra I/O,
                        // status queries, result conversion or profile logging.
                        crate::local_diagnostics::record(crate::local_diagnostics::Stage::QueueApply,Duration::ZERO,Some(*code));
                        let uncertain=current.as_ref().is_some_and(|s|s.uncertain);
                        let eq=current.as_ref().is_some_and(|s|s.dsp.is_some());
                        let current_ready=if eq&&super::liveness::needs_current_probe(code){
                            current.as_mut().is_some_and(|s|s.check().is_ok()&&s.dsp.as_mut().is_some_and(|d|matches!(d.health(),Ok(crate::windows_dsp::chain::Health::Ready|crate::windows_dsp::chain::Health::Active))))
                        }else{true};
                        if super::liveness::retire_current(code,uncertain,eq,current_ready){
                            if let Some(mut failed)=current.take(){if failed.abort().is_err(){eprintln!("Windows playback: fallback_cleanup_failed");}}
                            generation+=1;logical_state.set("pause");if eq{*eq_failure.borrow_mut()=Some(crate::windows_dsp::Error::ContinuityLost);}
                        }
                    }
                    if let Ok(mut view)=worker_direct_context.lock(){
                        *view=result.as_ref().ok().and_then(|value|current.as_ref().and_then(|s|s.direct.as_ref()).map(|policy|(value.generation,crate::windows_audio::direct_policy::Context::new(crate::windows_audio::projection::VolumeBackend::DirectEquipment,Some(policy.clone())))));
                    }
                    if let Ok(value) = &result {
                        if let Err(code) = worker_system_media.publish(value.generation, value.engine_running, &value.state, value.queue.get(value.queue_index), value.queue_index) {
                            if code != "shutting_down" { eprintln!("Windows system media: snapshot_unavailable"); }
                        }
                        history_publisher.publish(value.engine_running, &value.state, value.warning.is_some(), value.queue.get(value.queue_index));
                        let identity = if crate::windows_output::eligible(value.engine_running, &value.state) {
                            current.as_ref().and_then(|session|crate::windows_output::render_owner(session.dsp.is_some(),session.process.identity(),session.dsp.as_ref().and_then(|chain|chain.render_identity())))
                        } else { None };
                        worker_output.publish(value.generation, identity);
                        let revision=worker_config_revision.load(Ordering::Acquire);
                        let full=worker_settings.observed_usage_mode()==Some(super::library::UsageMode::Full);
                        let reader=if crate::windows_shared::analyzer::eligible(full,value.engine_running,&value.state,value.equalizer.verified,value.equalizer.in_path)&&crate::windows_shared::analyzer::source_allowed(&value.audio,value.file_name.as_deref()){
                            current.as_ref().and_then(|s|s.dsp.as_ref()).and_then(|chain|chain.analyzer_reader())
                        }else{None};
                        let config=worker_config_revision.clone();let stop=worker_stopping.clone();
                        worker_analyzer.publish(value.generation,revision,reader,Arc::new(move||revision%2==0&&config.load(Ordering::Acquire)==revision&&!stop.load(Ordering::Acquire)));
                        worker_source.publish_bound(
                            value.generation,
                            value.engine_running,
                            &value.state,
                            &value.queue,
                            value.queue_index,
                            current.as_ref().and_then(|session| session._media_lock.get(value.queue_index)).and_then(|lease| lease.identity()),
                        );
                    } else {
                        if let Err(code) = worker_system_media.invalidate() {
                            if code != "shutting_down" { eprintln!("Windows system media: invalidate_failed"); }
                        }
                        worker_output.clear();
                        worker_analyzer.clear();
                        worker_source.clear();
                    }
                    if queue_diagnostic{crate::local_diagnostics::record(crate::local_diagnostics::Stage::QueueApply,queue_clock.elapsed(),result.as_ref().err().copied());}
                    observer(&result);
                    if message.reply.send(result).is_err() {
                        eprintln!("Windows MPD: operation_receiver_closed");
                    }
                }
                drop(current);
            })
            .map_err(|_| "worker_failed")?;
        Ok(Self {
            sender,
            navidrome,
            plex,
            settings,
            audio_settings,
            direct_context,
            audio_admission,
            eq_intervention,
            config_revision,
            ready,
            stopping,
            intent,
            source,
            system_media,
            output,
            analyzer,
        })
    }
    fn call(&self, operation: Operation) -> Result<Snapshot> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Message { operation, reply })?;
        receiver.recv().map_err(|_| "worker_failed")?
    }
    pub fn system_media_view(&self) -> Arc<crate::windows_system_media::View> { self.system_media.clone() }
    pub(crate) fn analyzer_view(&self)->Arc<crate::windows_shared::analyzer::View>{self.analyzer.clone()}
    pub fn admit_intent(&self) -> String {
        (self.intent.fetch_add(1, Ordering::AcqRel) + 1).to_string()
    }
    pub fn intent_current(&self, token: &str) -> bool {
        token
            .parse::<u64>()
            .is_ok_and(|v| v == self.intent.load(Ordering::Acquire))
            && !self.stopping.load(Ordering::Acquire)
    }
    pub fn cancel_intent(&self, token: &str) {
        if let Ok(value) = token.parse::<u64>() {
            let _ = self.intent.compare_exchange(
                value,
                value.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
    pub fn stop_radio(&self, id: String, token: Option<String>) -> Result<Snapshot> {
        let token = token.map(|value| value.parse().map_err(|_| "PLAYBACK_SUPERSEDED")).transpose()?;
        self.call(Operation::StopRadio(id, token))
    }
    pub fn load_admitted(&self, selection: Selection, token: &str) -> Result<Snapshot> {
        let token = token.parse().map_err(|_| "PLAYBACK_SUPERSEDED")?;
        if token!=self.intent.load(Ordering::Acquire){return Err("PLAYBACK_SUPERSEDED");}
        let selection=self.prepare_selection(selection)?;
        self.call(Operation::Load(selection, Some(token)))
    }
    pub fn action_admitted(
        &self,
        generation: u64,
        action: Action,
        token: &str,
    ) -> Result<Snapshot> {
        let token = token.parse().map_err(|_| "PLAYBACK_SUPERSEDED")?;
        self.prepared_action(generation,action,Some(token))
    }
    pub fn enqueue(
        &self,
        selected: Selection,
        placement: Placement,
        token: &str,
    ) -> Result<Snapshot> {
        let token = token.parse().map_err(|_| "PLAYBACK_SUPERSEDED")?;
        if token!=self.intent.load(Ordering::Acquire){return Err("PLAYBACK_SUPERSEDED");}
        let selected=if selected.tracks.iter().any(|track|track.remote_source.is_some()) && !self.status()?.engine_running {
            super::transport::enabled(&selected.tracks,self.sources()?)?;selected
        }else{self.prepare_selection(selected)?};
        self.call(Operation::Enqueue(selected, placement, token))
    }
    pub fn edit_queue(&self, revision: String, action: Edit, token: &str) -> Result<Snapshot> {
        let token = token.parse().map_err(|_| "PLAYBACK_SUPERSEDED")?;
        self.call(Operation::Edit(revision, action, token))
    }
    pub fn recover(&self) -> Result<Snapshot> {
        self.call(Operation::Recover)
    }
    pub fn status(&self) -> Result<Snapshot> {
        self.call(Operation::Status)
    }
    pub fn load(&self, file: PathBuf) -> Result<Snapshot> {
        let path=crate::windows_io::read(crate::windows_io::READ_TIMEOUT,move|job|{
            if !job.current(){return Err("file_unavailable");}
            let file=crate::windows_file_info::normalize_root(&file).map_err(|_|"file_unavailable")?;
            if !job.current(){return Err("file_unavailable");}
            let path=file.canonicalize().map_err(|_|"file_unavailable")?;
            if !job.current(){return Err("file_unavailable");}Ok(path)
        }).map_err(|e|if e==crate::windows_io::Error::Busy{"engine_busy"}else{"file_unavailable"})??;
        let root = path.parent().ok_or("unsupported_path")?.to_owned();
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or("unsupported_path")?
            .to_owned();
        let extension = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ![
            "flac", "wav", "mp3", "ogg", "opus", "m4a", "aac", "aif", "aiff", "wv",
        ]
        .contains(&extension.as_str())
        {
            return Err("unsupported_file");
        }
        self.load_queue(Selection {
            root: root.clone(),
            tracks: vec![Track {
                id: format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes())),
                catalog_album_id: None,
                title: name.clone(),
                artist: String::new(),
                album: String::new(),
                disc: 1,
                number: 1,
                file_name: name,
                media_root: Some(root.clone()),
            radio: None,upnp:None,
            remote_source: None,
            remote_connection_id: None,
            plex_image:None,remote_lease: None,
                path,
            }],
            start: 0,
            album_id: None,
        })
    }
    pub fn load_queue(&self, selection: Selection) -> Result<Snapshot> {
        let selection=self.prepare_selection(selection)?;
        self.call(Operation::Load(selection, None))
    }
    pub fn gain(&self,expected:Option<u64>,action:Action)->Result<Snapshot>{
        if !matches!(&action,Action::Volume{..}|Action::Mute{..}){return Err("invalid_argument");}
        let gate=self.audio_admission.volume_intervention(&self.eq_intervention,self.stopping.load(Ordering::Acquire))?;
        let expected=if let Action::Mute{token,..}=&action{
            let token=token.strip_prefix("windows:").ok_or("SHARED_MUTE_CONTEXT_CHANGED")?.parse::<u64>().map_err(|_|"SHARED_MUTE_CONTEXT_CHANGED")?;
            if expected.is_some_and(|expected|expected!=token){return Err("SHARED_MUTE_CONTEXT_CHANGED");}Some(token)
        }else{expected};
        let (reply,receiver)=mpsc::channel();
        let context=self.audio_admission.context();
        self.sender.send(Message{operation:Operation::Gain(context,expected,action),reply})?;
        drop(gate); // No mutex held while the worker acquires its operation gate.
        receiver.recv().map_err(|_|"worker_failed")?
    }
    pub fn direct_mute(&self,muted:bool,token:String)->Result<Snapshot>{
        let gate=self.audio_admission.volume(self.stopping.load(Ordering::Acquire))?;
        let (generation,admitted)={let view=self.direct_context.lock().map_err(|_|crate::windows_audio::direct_policy::STALE)?;let (generation,context)=view.as_ref().ok_or(crate::windows_audio::direct_policy::STALE)?;context.validate_token(&token).map_err(crate::windows_audio::policy_error)?;(*generation,self.admit_intent().parse().map_err(|_|"PLAYBACK_SUPERSEDED")?)};
        let (reply,receiver)=mpsc::channel();
        self.sender.send(Message{operation:Operation::Action(generation,Action::DirectMute{muted,token},Some(admitted),None),reply})?;
        drop(gate); // The worker can now execute; no lock is held while waiting.
        receiver.recv().map_err(|_|"worker_failed")?
    }
    pub(crate) fn upnp_control(&self,command:crate::upnp::Control,guard:crate::windows_upnp::Guard)->Result<Snapshot>{
        use crate::upnp::Control;use crate::windows_upnp::Request;
        if !guard(){return Err("UPNP_SESSION_CHANGED");}
        let before=self.status()?;
        let (expected,command)=match command{Control::Session{id,action}=>(Some(id),*action),other=>(None,other)};
        if let Some(id)=&expected{crate::windows_upnp::media::require(&before.queue,id)?;}
        let request=match command{
            Control::Prepare(media) if expected.is_none()=>Request::Prepare(crate::windows_upnp::media::selection(media)?),
            Control::Clear=>Request::Action{id:expected,action:Action::Release},
            Control::Play=>Request::Action{id:expected,action:Action::SetPlaying{want_play:true}},
            Control::Pause=>Request::Action{id:expected,action:Action::SetPlaying{want_play:false}},
            Control::Stop=>Request::Action{id:expected,action:Action::Stop},
            Control::Seek(seconds) if seconds.is_finite()&&seconds>=0.=>Request::Action{id:expected,action:Action::Seek{seconds,index:Some(0)}},
            Control::Volume(value) if value<=100&&!before.audio_settings.direct()=>Request::Action{id:expected,action:Action::Volume{value:value as u8}},
            Control::Mute(muted) if !before.audio_settings.direct()=>Request::Action{id:expected,action:Action::Mute{muted,token:format!("windows:{}",before.generation)}},
            _=>return Err("UPNP_CONTROL_INVALID"),
        };
        let volume=matches!(request,Request::Action{action:Action::Volume{..}|Action::Mute{..},..});
        let gate=if volume{Some(self.audio_admission.volume_intervention(&self.eq_intervention,self.stopping.load(Ordering::Acquire))?)}else{self.note_eq_intervention();None};
        if !guard(){return Err("UPNP_SESSION_CHANGED");}
        let admitted=if volume{None}else{Some(self.admit_intent().parse().map_err(|_|"PLAYBACK_SUPERSEDED")?)};
        let(reply,receiver)=mpsc::channel();self.sender.send(Message{operation:Operation::Upnp(request,guard,admitted),reply})?;drop(gate);
        receiver.recv().map_err(|_|"worker_failed")?
    }
    pub(crate) fn remote_control(&self,command:crate::remote::Command,guard:crate::remote::Guard)->Result<Snapshot>{
        if !guard()||self.settings.observed_network_scope()!=Some((true,true)){return Err("REMOTE_CHANGED");}
        let before=self.status()?;let action=crate::windows_remote::model::resolve(&command,&before.remote_frame())?;
        let is_volume=matches!(action,Action::Volume{..});
        let gate=if is_volume{Some(self.audio_admission.volume_intervention(&self.eq_intervention,self.stopping.load(Ordering::Acquire))?)}else{self.note_eq_intervention();None};
        let admitted=if is_volume{None}else{Some(self.admit_intent().parse::<u64>().map_err(|_|"PLAYBACK_SUPERSEDED")?)};
        let resumes=matches!(action,Action::SetPlaying{want_play:true}|Action::Next|Action::Previous|Action::Jump{..});
        let prepared=if resumes&&!before.engine_running&&before.queue.iter().any(|t|t.remote_source.is_some()){Some(self.prepare_remote_tracks(before.queue.clone())?)}else{None};
        if !guard(){return Err("REMOTE_CHANGED");}
        let context=is_volume.then(||self.audio_admission.context());
        let(reply,receiver)=mpsc::channel();self.sender.send(Message{operation:Operation::Remote(before.generation,command,guard,admitted,prepared,context),reply})?;drop(gate);
        receiver.recv().map_err(|_|"worker_failed")?
    }
    pub fn action(&self, generation: u64, action: Action) -> Result<Snapshot> {
        self.prepared_action(generation,action,None)
    }
    fn prepare_selection(&self, mut selected:Selection)->Result<Selection>{
        super::transport::enabled(&selected.tracks,self.sources()?)?;
        if selected.tracks.iter().any(|track|track.remote_source.is_some()) {
            selected.tracks=self.prepare_remote_tracks(selected.tracks)?;
        }
        Ok(selected)
    }
    fn prepare_remote_tracks(&self,mut tracks:Vec<Track>)->Result<Vec<Track>>{
        crate::diagnostics::measured(crate::local_diagnostics::Stage::QueueResolve,move||{
        for kind in [0,1] {
            let indices=tracks.iter().enumerate().filter_map(|(index,track)|match &track.remote_source{Some(super::queue_v3::Source::Navidrome{..})if kind==0=>Some(index),Some(super::queue_v3::Source::Plex{..})if kind==1=>Some(index),_=>None}).collect::<Vec<_>>();
            if indices.is_empty(){continue;}
            let selected=indices.iter().map(|i|tracks[*i].clone()).collect();
            let prepared=if kind==0{tauri::async_runtime::block_on(self.navidrome.prepare_tracks(selected)).map_err(|e|crate::windows_navidrome::playback_error(&e))?}else{tauri::async_runtime::block_on(self.plex.prepare_tracks(selected)).map_err(|e|crate::windows_plex::playback_error(&e))?};
            if prepared.len()!=indices.len(){return Err("windows_media_unavailable");}
            for(index,track)in indices.into_iter().zip(prepared){tracks[index]=track;}
        }Ok(tracks)

        })
    }
    fn prepared_action(&self,generation:u64,action:Action,mut admitted:Option<u64>)->Result<Snapshot>{
        if matches!(action,Action::Volume{..}|Action::Mute{..}){return self.gain(Some(generation),action);}
        if matches!(action,Action::DirectMute{..}){return Err("invalid_argument");}
        self.note_eq_intervention();
        let resumes=matches!(action,Action::Toggle|Action::SetPlaying{want_play:true}|Action::Next|Action::Previous|Action::Jump{..});
        let prepared=if resumes {
            let before=self.status()?;
            if before.generation!=generation{return Err("stale_request");}
            if !before.engine_running && before.queue.iter().any(|track|track.remote_source.is_some()) {
                let intent=admitted.unwrap_or_else(||self.intent.load(Ordering::Acquire));
                if intent!=self.intent.load(Ordering::Acquire){return Err("PLAYBACK_SUPERSEDED");}
                admitted=Some(intent);
                Some(self.prepare_remote_tracks(before.queue)?)
            }else{None}
        }else{None};
        self.call(Operation::Action(generation,action,admitted,prepared))
    }
    pub(crate) fn apply_equalizer(&self,preset:Option<crate::equalizer::EqPreset>,bypass_expected:Option<crate::equalizer::EqPreset>)->Result<crate::windows_dsp::Snapshot>{
        self.eq_intervention.fetch_add(1,Ordering::AcqRel);
        let admitted=self.admit_intent().parse().map_err(|_|"PLAYBACK_SUPERSEDED")?;
        let before=self.status()?;
        let live=preset.is_some()&&before.equalizer.in_path==Some(true)&&before.equalizer.verified;
        let prepared=if !live&&before.queue.iter().any(|track|track.remote_source.is_some()){self.prepare_remote_tracks(before.queue.clone())?}else{before.queue.clone()};
        self.call(Operation::Equalizer(preset,bypass_expected,prepared,before.generation,admitted)).map(|s|s.equalizer)
    }
    pub(crate) fn equalizer_snapshot(&self)->Result<crate::windows_dsp::Snapshot>{self.status().map(|s|s.equalizer)}
    pub(crate) fn note_eq_intervention(&self){self.eq_intervention.fetch_add(1,Ordering::AcqRel);}
    pub(crate) fn compare_equalizer(&self,action:crate::windows_dsp::comparison::Action,token:Option<String>,expected:Option<crate::equalizer::EqPreset>)->Result<crate::windows_dsp::Snapshot>{
        let watch=crate::audio::EqTransitionWatch{intent:self.intent.clone(),intent_value:self.intent.load(Ordering::Acquire),intervention:self.eq_intervention.clone(),intervention_value:self.eq_intervention.load(Ordering::Acquire),stopping:self.stopping.clone()};
        let before=self.status()?;
        self.call(Operation::CompareEqualizer(action,token,expected,watch,before.generation)).map(|s|s.equalizer)
    }
    pub(crate) fn ab_context(&self)->Result<(u64,(u64,super::library::Sources))>{
        let revision=self.config_revision.load(Ordering::Acquire);
        if !self.ab_revision_current(revision){return Err("AB_BUSY");}
        let scope=self.settings.edition_scope().map_err(|_|"AB_CHANGED")?;
        if self.audio_settings.load()?.settings.audio_output_type!="pipewire"{return Err("AB_SHARED_ONLY");}
        if !self.ab_revision_current(revision){return Err("AB_CHANGED");}Ok((revision,scope))
    }
    pub(crate) fn ab_revision_current(&self,revision:u64)->bool{revision%2==0&&self.config_revision.load(Ordering::Acquire)==revision&&!self.stopping.load(Ordering::Acquire)&&self.ready.load(Ordering::Acquire)}
    pub(crate) fn pause_for_ab(&self,intent:&str,revision:u64)->Result<crate::audio::EqTransitionWatch>{
        let admitted=intent.parse().map_err(|_|"AB_CHANGED")?;
        self.call(Operation::PauseAB(admitted,revision))?.ab_watch.ok_or("AB_PAUSE")
    }
    pub(crate) fn ready(&self)->bool{self.ready.load(Ordering::Acquire)}
    pub(crate) fn preferences(&self)->Result<(Vec<PathBuf>,super::library::Sources,crate::windows_plex::PublicSettings,crate::windows_audio::Settings,bool)>{
        self.preferences_options().map(|(r,s,p,a,f,_)|(r,s,p,a,f))
    }
    pub(crate) fn preferences_options(&self)->Result<(Vec<PathBuf>,super::library::Sources,crate::windows_plex::PublicSettings,crate::windows_audio::Settings,bool,bool)>{
        self.preferences_features().map(|(r,s,p,a,f,e,_)|(r,s,p,a,f,e))
    }
    pub(crate) fn preferences_features(&self)->Result<(Vec<PathBuf>,super::library::Sources,crate::windows_plex::PublicSettings,crate::windows_audio::Settings,bool,bool,bool)>{
        let revision=self.config_revision.load(Ordering::Acquire);if revision%2!=0{return Err("engine_busy");}
        let (roots,sources)=self.settings.preferences()?;
        let plex=tauri::async_runtime::block_on(self.plex.public_settings()).map_err(|e|crate::windows_plex::playback_error(&e))?;
        let audio=self.audio_settings.load()?.settings;let first_run=self.settings.first_run()?;let experimental=self.settings.experimental_features()?;let online=self.settings.online_artwork_enabled()?;
        if self.config_revision.load(Ordering::Acquire)!=revision{return Err("engine_busy");}Ok((roots,sources,plex,audio,first_run,experimental,online))
    }
    pub(crate) fn preferences_modes(&self)->Result<(Vec<PathBuf>,super::library::Sources,crate::windows_plex::PublicSettings,crate::windows_audio::Settings,bool,bool,bool,super::library::UsageMode)>{
        let revision=self.config_revision.load(Ordering::Acquire);if revision%2!=0{return Err("engine_busy");}
        let (r,s,p,a,f,e,o)=self.preferences_features()?;let mode=self.settings.usage_mode()?;
        if self.config_revision.load(Ordering::Acquire)!=revision{return Err("engine_busy");}Ok((r,s,p,a,f,e,o,mode))
    }
    pub(crate) fn confirm_direct(&self,expected:crate::windows_audio::Settings,intent:&str)->Result<()>{self.call(Operation::ConfirmDirect(expected,intent.parse().map_err(|_|"PLAYBACK_SUPERSEDED")?)).map(|_|())}
    pub(crate) fn sources(&self)->Result<super::library::Sources>{
        let revision=self.config_revision.load(Ordering::Acquire);if revision%2!=0{return Err("engine_busy");}
        let value=self.settings.sources()?;
        if self.config_revision.load(Ordering::Acquire)!=revision{return Err("engine_busy");}Ok(value)
    }
    pub(crate) fn configure_sources(&self,roots:Option<Vec<PathBuf>>,sources:super::library::Sources,plex_account:Option<crate::windows_plex::PreparedAccount>,audio:Option<crate::windows_audio::Settings>,complete_setup:bool,intent:&str)->Result<()> {
        self.configure_options(roots,sources,plex_account,audio,complete_setup,None,intent)
    }
    pub(crate) fn configure_options(&self,roots:Option<Vec<PathBuf>>,sources:super::library::Sources,plex_account:Option<crate::windows_plex::PreparedAccount>,audio:Option<crate::windows_audio::Settings>,complete_setup:bool,experimental:Option<bool>,intent:&str)->Result<()>{
        self.configure_features(roots,sources,plex_account,audio,complete_setup,experimental,None,intent)
    }
    pub(crate) fn configure_features(&self,roots:Option<Vec<PathBuf>>,sources:super::library::Sources,plex_account:Option<crate::windows_plex::PreparedAccount>,audio:Option<crate::windows_audio::Settings>,complete_setup:bool,experimental:Option<bool>,online:Option<bool>,intent:&str)->Result<()>{
        self.configure_preferences(roots,sources,plex_account,audio,complete_setup,experimental,online,None,intent)
    }
    pub(crate) fn configure_preferences(&self,roots:Option<Vec<PathBuf>>,sources:super::library::Sources,plex_account:Option<crate::windows_plex::PreparedAccount>,audio:Option<crate::windows_audio::Settings>,complete_setup:bool,experimental:Option<bool>,online:Option<bool>,mode:Option<super::library::UsageMode>,intent:&str)->Result<()>{
        self.configure_preferences_response(roots,sources,plex_account,audio,complete_setup,experimental,online,mode,intent).map(|_|())
    }
    pub(crate) fn configure_preferences_response(&self,roots:Option<Vec<PathBuf>>,sources:super::library::Sources,plex_account:Option<crate::windows_plex::PreparedAccount>,audio:Option<crate::windows_audio::Settings>,complete_setup:bool,experimental:Option<bool>,online:Option<bool>,mode:Option<super::library::UsageMode>,intent:&str)->Result<super::library::imports::SaveResponse>{
        let admitted=intent.parse().map_err(|_|"PLAYBACK_SUPERSEDED")?;
        self.call(Operation::Configure(roots,sources,plex_account,audio,complete_setup,experimental,online,mode,admitted))?.config_save_response.ok_or("windows_config_uncertain")
    }
    pub(crate) fn account_change(&self,change:AccountChange,intent:&str)->Result<crate::windows_navidrome::Status>{
        let admitted=intent.parse().map_err(|_|"PLAYBACK_SUPERSEDED")?;
        self.call(Operation::Account(change,admitted))?.account_status.ok_or("NAVIDROME_UNAVAILABLE")
    }
    pub fn stop_for_sleep(&self) -> Result<()> {
        // Atomic admission is immediate, even while the MPD worker prepares a
        // candidate. Its final admission check rejects that superseded source.
        if self.stopping.load(Ordering::Acquire) {
            return Err("shutting_down");
        }
        let token = self.admit_intent().parse().map_err(|_| "worker_failed")?;
        let (reply, receiver) = mpsc::channel();
        self.sender.send_stop(Message { operation: Operation::SleepStop(token), reply })?;
        receiver.recv().map_err(|_| "worker_failed")??;
        Ok(())
    }
    pub fn source_binding(&self, uri: &str, index: usize) -> Option<super::source::Binding> {
        if self.stopping.load(Ordering::Acquire) {
            return None;
        }
        let binding = self
            .source
            .capture(uri, index, self.intent.load(Ordering::Acquire))?;
        self.source_binding_current(&binding).then_some(binding)
    }
    pub fn source_binding_current(&self, binding: &super::source::Binding) -> bool {
        !self.stopping.load(Ordering::Acquire)
            && self
                .source
                .current(binding, self.intent.load(Ordering::Acquire))
    }
    pub fn output_binding(&self, generation: u64) -> Option<crate::windows_output::Binding<super::process::Identity>> {
        let binding = self.output.capture(generation, self.intent.load(Ordering::Acquire))?;
        self.output_binding_current(&binding).then_some(binding)
    }
    pub fn output_binding_current(&self, binding: &crate::windows_output::Binding<super::process::Identity>) -> bool {
        !self.stopping.load(Ordering::Acquire)
            && self.output.current(binding, self.intent.load(Ordering::Acquire))
            && binding.identity.alive()
    }
    pub fn begin_shutdown(&self) {
        self.sender.close();
        self.stopping.store(true, Ordering::Release);
        self.source.clear();
        self.output.clear();
        self.analyzer.clear();
    }
    pub fn shutdown(&self) -> Result<()> {
        self.begin_shutdown();
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send_shutdown(Message {
                operation: Operation::Shutdown,
                reply,
            })?;
        receiver
            .recv_timeout(Duration::from_secs(6))
            .map_err(|_| "shutdown_failed")??;
        Ok(())
    }
}
