use crate::audio::{AudioEngine, AudioState, PlaybackState};
use mpris_server::{
    zbus::{self, fdo},
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, RootInterface, Server,
    Time, TrackId, Volume,
};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

const BUS_NAME_SUFFIX: &str = "sonante";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BasicMediaAction {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
}

trait BasicMediaControls {
    fn play(&mut self) -> Result<(), String>;
    fn pause(&mut self) -> Result<(), String>;
    fn play_pause(&mut self) -> Result<(), String>;
    fn next(&mut self) -> Result<(), String>;
    fn previous(&mut self) -> Result<(), String>;
}

impl BasicMediaControls for AudioEngine {
    fn play(&mut self) -> Result<(), String> {
        AudioEngine::play(self)
    }

    fn pause(&mut self) -> Result<(), String> {
        AudioEngine::pause(self)
    }

    fn play_pause(&mut self) -> Result<(), String> {
        self.toggle_play_pause()
    }

    fn next(&mut self) -> Result<(), String> {
        AudioEngine::next(self)
    }

    fn previous(&mut self) -> Result<(), String> {
        AudioEngine::previous(self)
    }
}

fn apply_basic_action(
    controls: &mut impl BasicMediaControls,
    action: BasicMediaAction,
) -> Result<(), String> {
    match action {
        BasicMediaAction::Play => controls.play(),
        BasicMediaAction::Pause => controls.pause(),
        BasicMediaAction::PlayPause => controls.play_pause(),
        BasicMediaAction::Next => controls.next(),
        BasicMediaAction::Previous => controls.previous(),
    }
}

#[derive(Debug)]
struct SonanteMpris {
    app: AppHandle,
}

impl SonanteMpris {
    async fn control(&self, action: BasicMediaAction) -> fdo::Result<()> {
        let app = self.app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let state = app
                .try_state::<AudioState>()
                .ok_or_else(|| "Estado de áudio indisponível para o controle MPRIS.".to_string())?;
            let mut audio = state.0.lock().map_err(|error| {
                format!("Falha ao acessar o estado de áudio via MPRIS: {error}")
            })?;
            apply_basic_action(&mut *audio, action)
        })
        .await
        .map_err(|error| fdo::Error::Failed(format!("Falha na tarefa MPRIS: {error}")))?
        .map_err(fdo::Error::Failed)
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        let app = self.app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let state = app
                .try_state::<AudioState>()
                .ok_or_else(|| "Estado de áudio indisponível para o MPRIS.".to_string())?;
            let audio = state.0.lock().map_err(|error| {
                format!("Falha ao acessar o estado de áudio via MPRIS: {error}")
            })?;
            audio.playback_state()
        })
        .await
        .map_err(|error| fdo::Error::Failed(format!("Falha na tarefa MPRIS: {error}")))?
        .map(|state| match state {
            PlaybackState::Playing => PlaybackStatus::Playing,
            PlaybackState::Paused => PlaybackStatus::Paused,
            PlaybackState::Stopped => PlaybackStatus::Stopped,
        })
        .map_err(fdo::Error::Failed)
    }
}

impl RootInterface for SonanteMpris {
    async fn raise(&self) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("Raise não é suportado.".into()))
    }

    async fn quit(&self) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("Quit não é suportado.".into()))
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _fullscreen: bool) -> zbus::Result<()> {
        Err(zbus::Error::Unsupported)
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn identity(&self) -> fdo::Result<String> {
        Ok("Sonante".into())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok(String::new())
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

impl PlayerInterface for SonanteMpris {
    async fn next(&self) -> fdo::Result<()> {
        self.control(BasicMediaAction::Next).await
    }

    async fn previous(&self) -> fdo::Result<()> {
        self.control(BasicMediaAction::Previous).await
    }

    async fn pause(&self) -> fdo::Result<()> {
        self.control(BasicMediaAction::Pause).await
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        self.control(BasicMediaAction::PlayPause).await
    }

    async fn stop(&self) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("Stop não é suportado.".into()))
    }

    async fn play(&self) -> fdo::Result<()> {
        self.control(BasicMediaAction::Play).await
    }

    async fn seek(&self, _offset: Time) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("Seek não é suportado.".into()))
    }

    async fn set_position(&self, _track_id: TrackId, _position: Time) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported(
            "SetPosition não é suportado.".into(),
        ))
    }

    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("OpenUri não é suportado.".into()))
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        SonanteMpris::playback_status(self).await
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(LoopStatus::None)
    }

    async fn set_loop_status(&self, _loop_status: LoopStatus) -> zbus::Result<()> {
        Err(zbus::Error::Unsupported)
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _rate: PlaybackRate) -> zbus::Result<()> {
        Err(zbus::Error::Unsupported)
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_shuffle(&self, _shuffle: bool) -> zbus::Result<()> {
        Err(zbus::Error::Unsupported)
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(Metadata::new())
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(1.0)
    }

    async fn set_volume(&self, _volume: Volume) -> zbus::Result<()> {
        Err(zbus::Error::Unsupported)
    }

    async fn position(&self) -> fdo::Result<Time> {
        Ok(Time::ZERO)
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

type SonanteMprisServer = Server<SonanteMpris>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecyclePhase {
    NotStarted,
    Starting,
    Running,
    Failed,
    Stopping,
}

#[derive(Debug)]
struct ServiceLifecycle {
    phase: LifecyclePhase,
    server: Option<SonanteMprisServer>,
    startup_task: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl ServiceLifecycle {
    fn new() -> Self {
        Self {
            phase: LifecyclePhase::NotStarted,
            server: None,
            startup_task: None,
        }
    }

    fn begin_start(&mut self) -> bool {
        if self.phase != LifecyclePhase::NotStarted {
            return false;
        }
        self.phase = LifecyclePhase::Starting;
        true
    }

    fn finish_start(&mut self, server: SonanteMprisServer) -> Option<SonanteMprisServer> {
        if self.phase == LifecyclePhase::Starting {
            self.startup_task = None;
            self.server = Some(server);
            self.phase = LifecyclePhase::Running;
            None
        } else {
            Some(server)
        }
    }

    fn start_failed(&mut self) {
        if self.phase == LifecyclePhase::Starting {
            self.startup_task = None;
            self.phase = LifecyclePhase::Failed;
        }
    }

    fn set_startup_task(&mut self, task: tauri::async_runtime::JoinHandle<()>) {
        if self.phase == LifecyclePhase::Starting {
            self.startup_task = Some(task);
        } else {
            task.abort();
        }
    }

    fn begin_shutdown(
        &mut self,
    ) -> (
        Option<SonanteMprisServer>,
        Option<tauri::async_runtime::JoinHandle<()>>,
    ) {
        self.phase = LifecyclePhase::Stopping;
        (self.server.take(), self.startup_task.take())
    }
}

#[derive(Debug)]
pub struct MprisState(Mutex<ServiceLifecycle>);

impl Default for MprisState {
    fn default() -> Self {
        Self(Mutex::new(ServiceLifecycle::new()))
    }
}

pub fn start(app: AppHandle) {
    let Some(state) = app.try_state::<MprisState>() else {
        eprintln!("[MPRIS] Estado do serviço indisponível.");
        return;
    };
    let should_start = match state.0.lock() {
        Ok(mut lifecycle) => lifecycle.begin_start(),
        Err(_) => {
            eprintln!("[MPRIS] Falha ao acessar o lifecycle do serviço.");
            false
        }
    };
    if !should_start {
        return;
    }

    let task_app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let server = match Server::new(
            BUS_NAME_SUFFIX,
            SonanteMpris {
                app: task_app.clone(),
            },
        )
        .await
        {
            Ok(server) => server,
            Err(error) => {
                eprintln!("[MPRIS] Session D-Bus indisponível: {error}");
                if let Some(state) = task_app.try_state::<MprisState>() {
                    if let Ok(mut lifecycle) = state.0.lock() {
                        lifecycle.start_failed();
                    }
                }
                return;
            }
        };

        let unused_server = task_app
            .try_state::<MprisState>()
            .and_then(|state| state.0.lock().ok()?.finish_start(server));
        if let Some(server) = unused_server {
            if let Err(error) = server.release_bus_name().await {
                eprintln!("[MPRIS] Falha ao liberar o nome D-Bus: {error}");
            }
        }
    });
    if let Some(state) = app.try_state::<MprisState>() {
        match state.0.lock() {
            Ok(mut lifecycle) => lifecycle.set_startup_task(task),
            Err(_) => {
                task.abort();
                eprintln!("[MPRIS] Falha ao guardar a tarefa de inicialização.");
            }
        }
    } else {
        task.abort();
    }
}

pub fn shutdown(app: &AppHandle) {
    let (server, startup_task) = match app.try_state::<MprisState>() {
        Some(state) => match state.0.lock() {
            Ok(mut lifecycle) => lifecycle.begin_shutdown(),
            Err(_) => {
                eprintln!("[MPRIS] Falha ao acessar o lifecycle no shutdown.");
                (None, None)
            }
        },
        None => (None, None),
    };

    if let Some(task) = startup_task {
        task.abort();
    }

    if let Some(server) = server {
        if let Err(error) = tauri::async_runtime::block_on(server.release_bus_name()) {
            eprintln!("[MPRIS] Falha ao encerrar o serviço: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeControls {
        actions: Vec<BasicMediaAction>,
        failure: Option<String>,
    }

    impl FakeControls {
        fn record(&mut self, action: BasicMediaAction) -> Result<(), String> {
            self.actions.push(action);
            match &self.failure {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }
    }

    impl BasicMediaControls for FakeControls {
        fn play(&mut self) -> Result<(), String> {
            self.record(BasicMediaAction::Play)
        }

        fn pause(&mut self) -> Result<(), String> {
            self.record(BasicMediaAction::Pause)
        }

        fn play_pause(&mut self) -> Result<(), String> {
            self.record(BasicMediaAction::PlayPause)
        }

        fn next(&mut self) -> Result<(), String> {
            self.record(BasicMediaAction::Next)
        }

        fn previous(&mut self) -> Result<(), String> {
            self.record(BasicMediaAction::Previous)
        }
    }

    #[test]
    fn basic_actions_dispatch_to_the_existing_audio_controls() {
        let mut controls = FakeControls::default();
        let actions = [
            BasicMediaAction::Play,
            BasicMediaAction::Pause,
            BasicMediaAction::PlayPause,
            BasicMediaAction::Next,
            BasicMediaAction::Previous,
        ];

        for action in actions {
            apply_basic_action(&mut controls, action).expect("ação simulada deve funcionar");
        }

        assert_eq!(controls.actions, actions);
    }

    #[test]
    fn basic_action_errors_are_not_reported_as_success() {
        let mut controls = FakeControls {
            actions: Vec::new(),
            failure: Some("ACK simulado".into()),
        };

        assert_eq!(
            apply_basic_action(&mut controls, BasicMediaAction::Next),
            Err("ACK simulado".into())
        );
    }

    #[test]
    fn lifecycle_starts_only_once() {
        let mut lifecycle = ServiceLifecycle::new();

        assert!(lifecycle.begin_start());
        assert!(!lifecycle.begin_start());
    }

    #[test]
    fn shutdown_while_starting_prevents_late_installation() {
        let mut lifecycle = ServiceLifecycle::new();

        assert!(lifecycle.begin_start());
        let (server, task) = lifecycle.begin_shutdown();
        assert!(server.is_none());
        assert!(task.is_none());
        assert_eq!(lifecycle.phase, LifecyclePhase::Stopping);
    }

    #[test]
    fn failed_start_is_not_retried() {
        let mut lifecycle = ServiceLifecycle::new();

        assert!(lifecycle.begin_start());
        lifecycle.start_failed();
        assert_eq!(lifecycle.phase, LifecyclePhase::Failed);
        assert!(!lifecycle.begin_start());
    }
}
