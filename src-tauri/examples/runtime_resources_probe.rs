//! Release-mode resource/selection probe. Never starts the player or opens audio.
#![allow(dead_code)]
#[path = "../src/installed_runtimes.rs"] mod installed_runtimes;
#[path = "../src/mpd_engine.rs"] mod mpd_engine;
fn main() {
    assert!(!cfg!(debug_assertions), "run this probe with --release");
    let direct=mpd_engine::MpdEngine::resolve().expect("installed Direct selection");
    if std::env::args().any(|v| v=="--direct-only") {
        let output=std::process::Command::new(direct.program()).arg("--version").output().unwrap();
        assert!(output.status.success()); assert!(direct.evidence(false).is_some());
        println!("direct-only fixed selection passed"); return;
    }
    let eq=mpd_engine::MpdEngine::resolve_eq().expect("installed EQ selection");
    let camilla=installed_runtimes::release(installed_runtimes::Kind::Camilla,"SONANTE_EQ_CAMILLA_PATH").expect("installed Camilla selection");
    let mut command=std::process::Command::new(&camilla);
    installed_runtimes::configure_camilla(&mut command);
    let env:std::collections::HashMap<_,_>=command.get_envs().collect();
    assert_eq!(env[std::ffi::OsStr::new("PIPEWIRE_MODULE_DIR")],Some(std::ffi::OsStr::new("/usr/lib/sonante/runtimes/v1/lib/pipewire-0.3")));
    assert_eq!(env[std::ffi::OsStr::new("PIPEWIRE_CONFIG_NAME")],None);
    for path in [direct.program(),eq.program(),camilla.as_path()] {
        let output=std::process::Command::new(path).arg("--version").output().expect("version child");
        assert!(output.status.success());
        println!("{}: {}",path.display(),String::from_utf8_lossy(&output.stdout).lines().next().unwrap_or(""));
    }
    assert!(direct.evidence(false).is_some());
    assert!(eq.evidence(false).is_none());
}
