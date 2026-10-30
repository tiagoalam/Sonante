use super::*;
#[test]
#[ignore = "explicit private PipeWire runtime only; never the personal session"]
fn isolated_session_startup_probe() {
    let root =
        PathBuf::from(std::env::var("SONANTE_EQ_TEST_ROOT").expect("private test root required"));
    assert_eq!(root.parent(), Some(std::path::Path::new("/tmp")));
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("seq-"));
    assert_eq!(
        std::env::var("XDG_RUNTIME_DIR").unwrap(),
        root.to_string_lossy()
    );
    assert_eq!(
        std::env::var("PIPEWIRE_REMOTE").unwrap(),
        "sonante-eq-isolated"
    );
    let current = Arc::new(AtomicU64::new(0));
    let mut session = Session::start(
        crate::equalizer::flat_preset(),
        current,
        Arc::new(AtomicBool::new(false)),
        Arc::new(RwLock::new(())),
        Environment::default(),
    )
    .expect("owned Camilla/FIFO/PipeWire preparation");
    let proof = json!({"session_started":true,"pid":session.child.as_ref().unwrap().id(),"stream":session.stream.name,"sink":session.sink.name,"pcm_pipe":session.pipe,"ready":session.health().is_ok(),"personal_session":false});
    fs::write(root.join("session-proof.json"), proof.to_string()).unwrap();
    session.stop().unwrap();
    assert!(!session.root.exists());
}

#[test]
#[ignore = "explicit private synthetic PCM/PipeWire proof only"]
fn isolated_audio_probe() {
    let root = PathBuf::from(std::env::var("SONANTE_EQ_TEST_ROOT").expect("private root"));
    assert_eq!(root.parent(), Some(std::path::Path::new("/tmp")));
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("seq-"));
    assert_eq!(
        std::env::var("XDG_RUNTIME_DIR").unwrap(),
        root.to_string_lossy()
    );
    assert_eq!(
        std::env::var("PIPEWIRE_REMOTE").unwrap(),
        "sonante-eq-isolated"
    );
    let mut session = Session::start(
        crate::equalizer::flat_preset(),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(RwLock::new(())),
        Environment::default(),
    )
    .unwrap();
    let pid = session.child.as_ref().unwrap().id();
    fs::write(
        root.join("ready.json"),
        json!({"pipe":session.pipe,"pid":pid,"stream":session.stream.name}).to_string(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut last = 0;
    loop {
        assert!(Instant::now() < deadline, "private driver timed out");
        let request = fs::read(root.join("request.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        if let Some(v) = request {
            let n = v["n"].as_u64().unwrap();
            if n > last {
                last = n;
                let mut p = crate::equalizer::flat_preset();
                match v["phase"].as_str().unwrap() {
                    "stop" => {
                        session.stop().unwrap();
                        fs::write(
                            root.join("reply.json"),
                            json!({"n":n,"clean":!session.root.exists()}).to_string(),
                        )
                        .unwrap();
                        break;
                    }
                    "peak" => {
                        p.bands[0].frequency_hz = 997.;
                        p.bands[0].gain_db = 6.;
                    }
                    "low_shelf" => {
                        p.bands[0].frequency_hz = 500.;
                        p.bands[0].gain_db = 6.;
                        p.bands[0].filter_type = crate::equalizer::EqFilterType::LowShelf;
                    }
                    "high_shelf" => {
                        p.bands[0].frequency_hz = 4000.;
                        p.bands[0].gain_db = 6.;
                        p.bands[0].filter_type = crate::equalizer::EqFilterType::HighShelf;
                    }
                    "preamp" => p.preamp_db = -6.,
                    "kill" => {
                        session.child.as_mut().unwrap().kill().unwrap();
                        thread::sleep(Duration::from_millis(50));
                        assert_eq!(session.health(), Err(Error::RuntimeExited));
                        session.stop().unwrap();
                        fs::write(
                            root.join("reply.json"),
                            json!({"n":n,"detected_exit":true,"clean":!session.root.exists()})
                                .to_string(),
                        )
                        .unwrap();
                        break;
                    }
                    "flat" => {}
                    other => panic!("unknown phase {other}"),
                }
                session
                    .controller
                    .apply_preset(session.id, &p, pcm())
                    .unwrap();
                session.preset = p;
                session.health().unwrap();
                assert_eq!(session.child.as_ref().unwrap().id(), pid);
                fs::write(
                    root.join("reply.json"),
                    json!({"n":n,"verified":true,"pid":pid,"protection":safety::assess(&session.preset,48000).unwrap()}).to_string(),
                )
                .unwrap();
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "export private synthetic safety cases only"]
fn isolated_safety_cases() {
    let root = PathBuf::from(std::env::var("SONANTE_EQ_TEST_ROOT").unwrap());
    assert_eq!(root.parent(), Some(std::path::Path::new("/tmp")));
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("seq-"));
    let mut presets = crate::equalizer::built_in_presets();
    let mut p = crate::equalizer::flat_preset();
    p.id = "positive".into();
    p.preamp_db = 12.;
    presets.push(p);
    for (id, f, q, g, kind) in [
        (
            "peak_low",
            20.,
            30.,
            12.,
            crate::equalizer::EqFilterType::Peak,
        ),
        (
            "shelf_high",
            20000.,
            30.,
            12.,
            crate::equalizer::EqFilterType::HighShelf,
        ),
        (
            "shelf_low",
            20.,
            0.1,
            -12.,
            crate::equalizer::EqFilterType::LowShelf,
        ),
    ] {
        let mut p = crate::equalizer::flat_preset();
        for b in &mut p.bands {
            b.enabled = false
        }
        p.id = id.into();
        let b = &mut p.bands[0];
        b.enabled = true;
        b.frequency_hz = f;
        b.q = q;
        b.gain_db = g;
        b.filter_type = kind;
        presets.push(p);
    }
    let mut p = crate::equalizer::flat_preset();
    p.id = "stacked".into();
    for (i, b) in p.bands.iter_mut().enumerate() {
        b.enabled = i < 3;
        b.frequency_hz = 997.;
        b.gain_db = 6.;
    }
    presets.push(p);
    let mut cases = vec![];
    for p in presets {
        let s = match safety::assess(&p, 48000) {
            Ok(s) => s,
            Err(_) => {
                cases.push(json!({"preset":p,"admitted":false}));
                continue;
            }
        };
        let conf = peq::convert_preset(&p, pcm()).unwrap();
        let coefficients = p
            .bands
            .iter()
            .map(|b| safety::coefficients(b, 48000))
            .collect::<Vec<_>>();
        cases.push(json!({"admitted":true,"preset":p,"protection":s,"config":conf,"coefficients":coefficients}));
    }
    fs::write(root.join("safety-cases.json"), json!(cases).to_string()).unwrap();
}

#[test]
#[ignore = "explicit owned MPD/PipeWire transition proof only"]
fn isolated_transition_probe() {
    let root = PathBuf::from(std::env::var("SONANTE_EQ_TEST_ROOT").unwrap());
    assert_eq!(root.parent(), Some(std::path::Path::new("/tmp")));
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("seq-"));
    assert_eq!(
        std::env::var("XDG_RUNTIME_DIR").unwrap(),
        root.to_string_lossy()
    );
    assert_eq!(
        std::env::var("PIPEWIRE_REMOTE").unwrap(),
        "sonante-eq-isolated"
    );
    let wait = |name: &str| {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if let Ok(b) = fs::read(root.join(name)) {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) {
                    break v;
                }
            }
            assert!(Instant::now() < deadline, "private driver {name}");
            thread::sleep(Duration::from_millis(10));
        }
    };
    let mut session = Session::start(
        crate::equalizer::flat_preset(),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(RwLock::new(())),
        Environment::default(),
    )
    .unwrap();
    fs::write(
        root.join("session-ready.json"),
        json!({"pipe":session.pipe,"camilla_pid":session.child.as_ref().unwrap().id()}).to_string(),
    )
    .unwrap();
    wait("mpd-ready.json");
    let mut audio = crate::audio::AudioEngine::new(
        root.join("mpd.sock").to_str().unwrap(),
        root.join("media").to_str().unwrap(),
    );
    let track = crate::audio::TrackMetadata {
        title: "Synthetic".into(),
        artist: "Fixture".into(),
        album: "Fixture".into(),
        thumb: None,
        plex_image: None,
        media_locator: None,
        uri: "synthetic.wav".into(),
        duration: Some(30.),
    };
    audio
        .play_tracks(vec![track], vec!["synthetic.wav".into()], 0)
        .unwrap();
    fs::write(root.join("playing.json"), "{}").unwrap();
    let request = wait("capture.json");
    let watch = audio.watch_eq_transition();
    let resolved = audio
        .capture_device_switch_queue()
        .unwrap()
        .bind_resolved_uris(vec!["synthetic.wav".into()])
        .unwrap();
    let saved = audio.prepare_reserved_device_switch(&resolved).unwrap();
    if request["to"] == "bypass" {
        session.stop().unwrap();
    }
    fs::write(
        root.join("switch-ready.json"),
        json!({"elapsed":saved.unwrap().elapsed}).to_string(),
    )
    .unwrap();
    wait("new-mpd.json");
    assert_eq!(
        audio.restore_eq_switch(saved, &resolved).unwrap(),
        crate::audio::DeviceSwitchRestoreOutcome::Complete
    );
    fs::write(root.join("restore-ready.json"), "{}").unwrap();
    let finish = wait("finish.json");
    if finish["intervene"] == true {
        audio.pause().unwrap();
    }
    if session.child.is_some() {
        session.health().unwrap();
    }
    let result = audio.resume_eq_switch(saved, &resolved, &watch).unwrap();
    fs::write(root.join("transition-result.json"),json!({"result":result,"watch_current":watch.current(),"original_elapsed":saved.unwrap().elapsed,"true_bypass":session.child.is_none()}).to_string()).unwrap();
    wait("inspected.json");
    if session.child.is_some() {
        session.stop().unwrap();
    }
}

#[test]
#[ignore = "explicit isolated private PipeWire continuity fault only"]
fn isolated_continuity_actor_probe() {
    let root=PathBuf::from(std::env::var("SONANTE_EQ_TEST_ROOT").unwrap());
    assert_eq!(root.parent(),Some(std::path::Path::new("/tmp")));assert!(root.file_name().unwrap().to_string_lossy().starts_with("seq-"));
    assert_eq!(std::env::var("XDG_RUNTIME_DIR").unwrap(),root.to_string_lossy());assert_eq!(std::env::var("PIPEWIRE_REMOTE").unwrap(),"sonante-eq-isolated");
    let state=State::new();let failures=Arc::new(AtomicU64::new(0));let count=failures.clone();
    let prepared=state.prepare(crate::equalizer::flat_preset(),Arc::new(move||{count.fetch_add(1,Ordering::SeqCst);})).unwrap();
    state.commit().unwrap();state.observe_playing(true);
    fs::write(root.join("actor-ready.json"),json!({"pipe":prepared.pipe}).to_string()).unwrap();
    let deadline=Instant::now()+Duration::from_secs(25);let mut checked=false;
    loop {
        assert!(Instant::now()<deadline,"isolated actor driver timed out");
        if !checked&&root.join("check-normal").exists(){let s=state.snapshot();assert!(s.verified);assert!(s.error.is_none());checked=true;fs::write(root.join("normal-ok"),b"ok").unwrap();}
        if failures.load(Ordering::SeqCst)>0 {
            let s=state.snapshot();assert_eq!(s.error,Some(Error::ContinuityLost));assert!(!s.verified);assert!(checked);assert_eq!(failures.load(Ordering::SeqCst),1);
            state.stop().unwrap();state.begin_shutdown();state.join();
            fs::write(root.join("actor-proof.json"),json!({"normal_verified":checked,"fault":"continuity_lost","verified_after_fault":s.verified,"failure_callback_count":1,"owned_session_clean":!prepared.pipe.parent().unwrap().exists(),"scope":"actual DSP actor plus owned MPD; callback observed, not full Tauri fallback"}).to_string()).unwrap();break;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "explicit private A/B capture and lifecycle proof only"]
fn isolated_comparison_probe() {
 let root=PathBuf::from(std::env::var("SONANTE_EQ_TEST_ROOT").unwrap());assert_eq!(root.parent(),Some(std::path::Path::new("/tmp")));assert!(root.file_name().unwrap().to_string_lossy().starts_with("seq-"));assert_eq!(std::env::var("XDG_RUNTIME_DIR").unwrap(),root.to_string_lossy());assert_eq!(std::env::var("XDG_CONFIG_HOME").unwrap(),root.join("config").to_string_lossy());assert_eq!(std::env::var("PIPEWIRE_REMOTE").unwrap(),"sonante-eq-isolated");
 let source=crate::equalizer::built_in_presets()[2].clone();let state=State::new();let failed=Arc::new(AtomicBool::new(false));let flag=failed.clone();let prepared=state.prepare(source.clone(),Arc::new(move||{flag.store(true,Ordering::Release)})).unwrap();state.commit().unwrap();
 let mut audio=crate::audio::AudioEngine::new(root.join("mpd.sock").to_str().unwrap(),root.join("media").to_str().unwrap());
 fs::write(root.join("ab-ready.json"),json!({"pipe":prepared.pipe,"source":source,"protection":safety::assess(&source,48000).unwrap(),"reference_dsp":peq::convert_reference(&source,pcm()).unwrap(),"equalized_dsp":peq::convert_preset(&source,pcm()).unwrap(),"reference_cases":crate::equalizer::built_in_presets().iter().map(|p|json!({"preset":p,"protection":safety::assess(p,48000).unwrap(),"dsp":peq::convert_reference(p,pcm()).unwrap()})).collect::<Vec<_>>(),"coefficients":source.bands.iter().filter(|b|b.enabled).map(|b|safety::coefficients(b,48000)).collect::<Vec<_>>()} ).to_string()).unwrap();
 let deadline=Instant::now()+Duration::from_secs(90);let mut last=0;
 loop {
  assert!(Instant::now()<deadline);assert!(!failed.load(Ordering::Acquire));
  let req=fs::read(root.join("request.json")).ok().and_then(|s|serde_json::from_slice::<serde_json::Value>(&s).ok());
  if let Some(v)=req {let n=v["n"].as_u64().unwrap();if n>last {last=n;let phase=v["phase"].as_str().unwrap();let before=state.snapshot();let token=before.comparison.as_ref().map(|c|c.token.clone());
   let result=match phase {
    "reference"=>state.compare(comparison::Action::Reference,token,before.applied.clone(),Some(audio.watch_eq_transition())),
    "equalized"=>state.compare(comparison::Action::Equalized,token,None,None),
    "finish"=>state.compare(comparison::Action::Finish,token,None,None),
    "pause"=>{audio.pause().unwrap();state.observe_playing(false);Ok(state.snapshot())},
    "playing"=>{audio.note_eq_intervention();state.observe_playing(true);Ok(state.snapshot())},
    "stopped"=>{audio.note_eq_intervention();state.observe_playing(false);Ok(state.snapshot())},
    "new_profile"=>{audio.note_eq_intervention();let mut p=source.clone();p.id="fresh".into();p.name="Fresh synthetic".into();p.preamp_db=-7.;state.apply(p.clone()).unwrap();assert_eq!(state.compare(comparison::Action::Finish,token,None,None).unwrap_err(),Error::ComparisonStale);assert_eq!(state.snapshot().applied.as_ref(),Some(&p));Ok(state.snapshot())},
    "output_stop"=>{state.stop().unwrap();assert_eq!(state.compare(comparison::Action::Finish,token,None,None).unwrap_err(),Error::ComparisonUnavailable);assert!(!prepared.pipe.parent().unwrap().exists());Ok(state.snapshot())},
    "quit"=>{state.begin_shutdown();assert_eq!(state.compare(comparison::Action::Finish,token,None,None).unwrap_err(),Error::Cancelled);state.join();fs::write(root.join("reply.json"),json!({"n":n,"clean":!prepared.pipe.parent().unwrap().exists(),"store_absent":!root.join("config/sonante/equalizer-shared-v1.json").exists()}).to_string()).unwrap();break;},
    _=>panic!("unknown private phase"),
   };
   fs::write(root.join("reply.json"),json!({"n":n,"ok":result.is_ok(),"error":result.as_ref().err().map(|e|format!("{e:?}")),"snapshot":state.snapshot()}).to_string()).unwrap();
  }}thread::sleep(Duration::from_millis(15));
 }
}
