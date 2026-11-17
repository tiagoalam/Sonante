use super::*;
use crate::windows_mpd::session::{Action, Controller as Audio};
use ::windows::{
    core::{factory, Error, Result as WinResult, HSTRING},
    Foundation::TypedEventHandler,
    Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
        SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
    },
    Win32::{
        Foundation::{E_ABORT, E_NOTIMPL, HWND},
        System::WinRT::{
            ISystemMediaTransportControlsInterop, RoInitialize, RoUninitialize,
            RO_INIT_MULTITHREADED,
        },
    },
};
use std::{
    sync::{mpsc, Arc},
    time::Duration,
};
type Notify = Arc<dyn Fn(&'static str) + Send + Sync>;
impl Transport for Audio {
    fn current(&self, token: &str) -> bool {
        self.intent_current(token)
    }
    fn generation(&self) -> Result<u64> {
        self.status().map(|status| status.generation)
    }
    fn apply(&self, generation: u64, target: Target, token: &str) -> Result<()> {
        let action = match target {
            Target::Playing(want_play) => Action::SetPlaying { want_play },
            Target::Next => Action::Next,
            Target::Previous => Action::Previous,
        };
        self.action_admitted(generation, action, token).map(|_| ())
    }
}
struct Apartment;
impl Apartment {
    fn enter() -> WinResult<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}
struct Controls {
    controls: SystemMediaTransportControls,
    token: Option<i64>,
}
impl Controls {
    fn new(hwnd: isize, view: Arc<View>, audio: Audio, notify: Notify) -> WinResult<Self> {
        let interop =
            factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
        let controls = unsafe {
            interop.GetForWindow::<SystemMediaTransportControls>(HWND(
                hwnd as *mut core::ffi::c_void,
            ))?
        };
        let mut owned = Self {
            controls,
            token: None,
        };
        owned.controls.SetIsEnabled(false)?;
        owned.controls.SetIsPlayEnabled(false)?;
        owned.controls.SetIsPauseEnabled(false)?;
        owned.controls.SetIsNextEnabled(false)?;
        owned.controls.SetIsPreviousEnabled(false)?;
        // Match the original MPRIS surface: no Stop/seek/rate/record controls.
        owned.controls.SetIsStopEnabled(false)?;
        owned.controls.SetIsRecordEnabled(false)?;
        owned.controls.SetIsFastForwardEnabled(false)?;
        owned.controls.SetIsRewindEnabled(false)?;
        owned.controls.SetIsChannelUpEnabled(false)?;
        owned.controls.SetIsChannelDownEnabled(false)?;
        let handler = TypedEventHandler::<
            SystemMediaTransportControls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        >::new(move |_, args| {
            let args = args.as_ref().ok_or_else(|| Error::from_hresult(E_ABORT))?;
            let button = match args.Button()? {
                SystemMediaTransportControlsButton::Play => Button::Play,
                SystemMediaTransportControlsButton::Pause => Button::Pause,
                SystemMediaTransportControlsButton::Next => Button::Next,
                SystemMediaTransportControlsButton::Previous => Button::Previous,
                _ => {
                    notify("windows_feature_unavailable");
                    return Err(Error::from_hresult(E_NOTIMPL));
                }
            };
            if let Err(code) = view.enqueue(button, || audio.admit_intent()) {
                if code != "shutting_down" {
                    notify(code);
                }
                return Err(Error::from_hresult(E_ABORT));
            }
            Ok(())
        });
        owned.token = Some(owned.controls.ButtonPressed(&handler)?);
        Ok(owned)
    }
    fn update(&self, projection: &Projection) -> WinResult<()> {
        let updater = self.controls.DisplayUpdater()?;
        match projection {
            Projection::Unavailable => {
                self.controls.SetIsEnabled(false)?;
                self.controls
                    .SetPlaybackStatus(MediaPlaybackStatus::Closed)?;
                // ClearAll resets the media type. Do not publish an Unknown
                // type after disabling; the next track sets Music before Update.
                updater.ClearAll()?;
            }
            Projection::Track {
                playback,
                title,
                artist,
                album,
                ..
            } => {
                updater.ClearAll()?;
                updater.SetType(MediaPlaybackType::Music)?;
                let music = updater.MusicProperties()?;
                music.SetTitle(&HSTRING::from(title))?;
                music.SetArtist(&HSTRING::from(artist))?;
                music.SetAlbumTitle(&HSTRING::from(album))?;
                updater.Update()?;
                self.controls.SetPlaybackStatus(match playback {
                    Playback::Playing => MediaPlaybackStatus::Playing,
                    Playback::Paused => MediaPlaybackStatus::Paused,
                    Playback::Stopped => MediaPlaybackStatus::Stopped,
                })?;
                self.controls.SetIsPlayEnabled(true)?;
                self.controls.SetIsPauseEnabled(true)?;
                self.controls.SetIsNextEnabled(true)?;
                self.controls.SetIsPreviousEnabled(true)?;
                self.controls.SetIsEnabled(true)?;
            }
        }
        Ok(())
    }
}
impl Drop for Controls {
    fn drop(&mut self) {
        if self.controls.SetIsEnabled(false).is_err() {
            eprintln!("Windows system media: disable_failed");
        }
        if let Some(token) = self.token.take() {
            if self.controls.RemoveButtonPressed(token).is_err() {
                eprintln!("Windows system media: unregister_failed");
            }
        }
        if self.update(&Projection::Unavailable).is_err() {
            eprintln!("Windows system media: clear_failed");
        }
    }
}
fn run(hwnd: isize, view: &Arc<View>, audio: &Audio, notify: &Notify) -> Result<()> {
    let _apartment = Apartment::enter().map_err(|_| UNAVAILABLE)?;
    if view.stopping.load(Ordering::Acquire) {
        return Ok(());
    }
    let controls = Controls::new(hwnd, view.clone(), audio.clone(), notify.clone())
        .map_err(|_| UNAVAILABLE)?;
    if view.stopping.load(Ordering::Acquire) {
        return Ok(());
    }
    // Created after Controls, so callbacks are fenced before its potentially
    // blocking native disable/unregister/clear destructor runs on every exit.
    let _readiness = ReadyOnDrop(view.clone());
    view.ready(true);
    let mut seen = u64::MAX;
    while let Some(work) = view.work(seen)? {
        // Snapshot publication never reads the engine. Reject an already stale
        // snapshot before entering native I/O, then consume the latest next.
        if work.revision != seen && !view.stopping.load(Ordering::Acquire) {
            let current = view.inner.lock().map_err(|_| UNAVAILABLE)?.revision == work.revision;
            if current {
                controls.update(&work.snapshot).map_err(|_| UNAVAILABLE)?;
            }
            seen = work.revision;
        }
        if let Some(request) = work.request {
            if view.stopping.load(Ordering::Acquire) {
                break;
            }
            if let Err(code) = execute(audio, &request) {
                if code != "PLAYBACK_SUPERSEDED" && code != "shutting_down" {
                    notify(code);
                    eprintln!("Windows system media: control_failed {code}");
                }
            }
        }
    }
    view.ready(false);
    Ok(())
}
pub struct Controller {
    view: Arc<View>,
    done: Mutex<Option<mpsc::Receiver<Result<()>>>>,
}
impl Controller {
    pub fn new(
        hwnd: isize,
        audio: Audio,
        notify: impl Fn(&'static str) + Send + Sync + 'static,
    ) -> Result<Self> {
        let view = audio.system_media_view();
        let worker = view.clone();
        let notify: Notify = Arc::new(notify);
        let (done_tx, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("sonante-windows-system-media".into())
            .spawn(move || {
                let result = run(hwnd, &worker, &audio, &notify);
                worker.ready(false);
                if let Err(code) = result {
                    notify(code);
                    eprintln!("Windows system media: {code}");
                }
                if done_tx.send(result).is_err() {
                    eprintln!("Windows system media: shutdown_receiver_closed");
                }
            })
            .map_err(|_| UNAVAILABLE)?;
        Ok(Self {
            view,
            done: Mutex::new(Some(done)),
        })
    }
    pub fn begin_shutdown(&self) {
        self.view.stop();
    }
    pub fn shutdown(&self) -> Result<()> {
        self.begin_shutdown();
        let receiver = self
            .done
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .take()
            .ok_or("shutting_down")?;
        receiver
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| UNAVAILABLE)?
    }
}
