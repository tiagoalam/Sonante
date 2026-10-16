use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub const LEVEL_EVENT: &str = "now-playing://audio-level";
pub const STATUS_EVENT: &str = "now-playing://analyzer-status";

const SAMPLE_RATE: usize = 48_000;
const CHANNELS: usize = 2;
const BYTES_PER_SAMPLE: usize = 2;
const FRAMES_PER_EVENT: usize = SAMPLE_RATE / 30;
const FFT_SIZE: usize = 2048;
const SPECTRUM_BANDS: usize = 48;
const SPECTRUM_MIN_HZ: f32 = 40.0;
const SPECTRUM_MAX_HZ: f32 = 20_000.0;
const SPECTRUM_FLOOR_DB: f32 = -90.0;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioLevelFrame {
    pub left_rms: f32,
    pub right_rms: f32,
    pub left_peak: f32,
    pub right_peak: f32,
    pub spectrum: Vec<f32>,
}

struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    hann: Vec<f32>,
    hann_sum: f32,
    fft_buffer: Vec<Complex32>,
    history: Vec<f32>,
    write_index: usize,
    samples_seen: usize,
    band_bin_ranges: Vec<(usize, usize)>,
    bands: Vec<f32>,
}

impl SpectrumAnalyzer {
    fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let hann: Vec<f32> = (0..FFT_SIZE)
            .map(|index| {
                let phase = 2.0 * std::f32::consts::PI * index as f32 / (FFT_SIZE - 1) as f32;
                0.5 * (1.0 - phase.cos())
            })
            .collect();
        let hann_sum = hann.iter().sum();
        let band_bin_ranges = (0..SPECTRUM_BANDS).map(band_bin_range).collect();
        Self {
            fft,
            hann,
            hann_sum,
            fft_buffer: vec![Complex32::new(0.0, 0.0); FFT_SIZE],
            history: vec![0.0; FFT_SIZE],
            write_index: 0,
            samples_seen: 0,
            band_bin_ranges,
            bands: vec![0.0; SPECTRUM_BANDS],
        }
    }

    fn analyze_pcm(&mut self, bytes: &[u8]) -> Vec<f32> {
        for frame in bytes.chunks_exact(CHANNELS * BYTES_PER_SAMPLE) {
            let left = i16::from_le_bytes([frame[0], frame[1]]) as f32 / 32768.0;
            let right = i16::from_le_bytes([frame[2], frame[3]]) as f32 / 32768.0;
            self.history[self.write_index] = (left + right) * 0.5;
            self.write_index = (self.write_index + 1) % FFT_SIZE;
            self.samples_seen = self.samples_seen.saturating_add(1);
        }
        self.analyze_history()
    }

    fn analyze_history(&mut self) -> Vec<f32> {
        self.bands.fill(0.0);
        if self.samples_seen < FFT_SIZE {
            return self.bands.clone();
        }

        for index in 0..FFT_SIZE {
            let sample = self.history[(self.write_index + index) % FFT_SIZE];
            self.fft_buffer[index] = Complex32::new(sample * self.hann[index], 0.0);
        }
        self.fft.process(&mut self.fft_buffer);

        for (band, (start, end)) in self.band_bin_ranges.iter().copied().enumerate() {
            for bin in start..end {
                let amplitude = 2.0 * self.fft_buffer[bin].norm() / self.hann_sum;
                let dbfs = 20.0 * amplitude.max(f32::MIN_POSITIVE).log10();
                let normalized = ((dbfs - SPECTRUM_FLOOR_DB) / -SPECTRUM_FLOOR_DB).clamp(0.0, 1.0);
                self.bands[band] = self.bands[band].max(normalized);
            }
        }
        self.bands.clone()
    }
}

#[cfg(test)]
fn frequency_to_band(frequency: f32) -> Option<usize> {
    if !frequency.is_finite() || !(SPECTRUM_MIN_HZ..=SPECTRUM_MAX_HZ).contains(&frequency) {
        return None;
    }
    let position = (frequency / SPECTRUM_MIN_HZ).ln() / (SPECTRUM_MAX_HZ / SPECTRUM_MIN_HZ).ln();
    Some(((position * SPECTRUM_BANDS as f32).floor() as usize).min(SPECTRUM_BANDS - 1))
}

fn band_bin_range(band: usize) -> (usize, usize) {
    let ratio = SPECTRUM_MAX_HZ / SPECTRUM_MIN_HZ;
    let lower = SPECTRUM_MIN_HZ * ratio.powf(band as f32 / SPECTRUM_BANDS as f32);
    let upper = SPECTRUM_MIN_HZ * ratio.powf((band + 1) as f32 / SPECTRUM_BANDS as f32);
    let bin_hz = SAMPLE_RATE as f32 / FFT_SIZE as f32;
    let start = ((lower / bin_hz).ceil() as usize).clamp(1, FFT_SIZE / 2);
    let end = ((upper / bin_hz).ceil() as usize)
        .max(start + 1)
        .min(FFT_SIZE / 2 + 1);
    (start, end)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzerStatus {
    available: bool,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerOwner {
    pub host: String,
    pub session_id: String,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
}

#[derive(Default)]
struct AnalyzerOwnership {
    owner: Option<AnalyzerOwner>,
    generation: u64,
}

pub fn authorized_analyzer_host(host: &str) -> bool {
    matches!(host, "main" | "now-playing")
}

impl AnalyzerOwnership {
    fn claim(&mut self, host: &str, session_id: String) -> AnalyzerOwner {
        self.generation += 1;
        let owner = AnalyzerOwner {
            host: host.into(),
            session_id,
            generation: self.generation,
            lease_id: None,
        };
        self.owner = Some(owner.clone());
        owner
    }
    fn clear_if_host(&mut self, host: &str) -> bool {
        if !self.owner.as_ref().is_some_and(|owner| owner.host == host) {
            return false;
        }
        self.owner = None;
        true
    }
    fn matches(&self, host: &str, session_id: &str, generation: u64) -> bool {
        self.owner.as_ref().is_some_and(|owner| {
            owner.host == host && owner.session_id == session_id && owner.generation == generation
        })
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzerEnvelope<T: Serialize + Clone> {
    owner: AnalyzerOwner,
    data: T,
}

pub struct AnalyzerState(pub std::sync::Mutex<AudioAnalyzer>);

pub struct AudioAnalyzer {
    socket_path: String,
    fifo_path: PathBuf,
    ownership: AnalyzerOwnership,
    active_lease: Option<String>,
    controller_stop: Option<Arc<AtomicBool>>,
    reader_stop: Option<Arc<AtomicBool>>,
    reader: Option<JoinHandle<()>>,
    controller: Option<JoinHandle<()>>,
}

impl AudioAnalyzer {
    fn observe_phase(&self, phase: &'static str) {
        crate::diagnostics::now_playing_phase(
            phase,
            self.owner_host(),
            self.ownership.generation,
            self.active_lease.is_some(),
        );
    }

    pub fn new(socket_path: String, fifo_path: PathBuf) -> Self {
        Self {
            socket_path,
            fifo_path,
            ownership: AnalyzerOwnership::default(),
            active_lease: None,
            controller_stop: None,
            reader_stop: None,
            reader: None,
            controller: None,
        }
    }

    // Only main can reserve a host (checked at IPC). Reservation invalidates all
    // old starts/cleanup before the new host receives its generation.
    pub fn claim_for_host(
        &mut self,
        host: &str,
        session_id: String,
        app: &AppHandle,
    ) -> Result<AnalyzerOwner, String> {
        if !authorized_analyzer_host(host) || session_id.is_empty() || session_id.len() > 128 {
            return Err("Proprietário do analyzer inválido.".into());
        }
        self.observe_phase(if host == "main" {
            "reserve_main_begin"
        } else {
            "reserve_detached_begin"
        });
        self.stop(Some(app))?;
        self.active_lease = None;
        let owner = self.ownership.claim(host, session_id);
        self.observe_phase("reserved");
        Ok(owner)
    }

    pub fn start_for_host(
        &mut self,
        host: &str,
        session_id: &str,
        generation: u64,
        lease_id: String,
        app: AppHandle,
    ) -> Result<(), String> {
        if !self.ownership.matches(host, session_id, generation)
            || lease_id.is_empty()
            || lease_id.len() > 128
        {
            self.observe_phase("start_stale_rejected");
            return Err("Sessão do analyzer obsoleta ou não autorizada.".into());
        }
        self.observe_phase("start_requested");
        if self.active_lease.as_deref() != Some(&lease_id) {
            self.stop(Some(&app))?;
        }
        self.active_lease = Some(lease_id);
        self.start(app)
    }

    fn capture_matches(
        &self,
        host: &str,
        session_id: &str,
        generation: u64,
        lease_id: &str,
    ) -> bool {
        self.ownership.matches(host, session_id, generation)
            && self.active_lease.as_deref() == Some(lease_id)
    }

    pub fn stop_for_host(
        &mut self,
        host: &str,
        session_id: &str,
        generation: u64,
        lease_id: &str,
        app: &AppHandle,
    ) -> Result<(), String> {
        if !self.capture_matches(host, session_id, generation, lease_id) {
            self.observe_phase("cleanup_stale_ignored");
            return Ok(());
        }
        self.observe_phase("cleanup_current");
        let result = self.stop(Some(app));
        self.active_lease = None;
        result
    }

    fn event_owner(&self) -> Option<AnalyzerOwner> {
        self.ownership.owner.clone().map(|mut owner| {
            owner.lease_id = self.active_lease.clone();
            owner
        })
    }

    pub fn stop_for_host_close(&mut self, host: &str, app: &AppHandle) -> Result<(), String> {
        if self.owner_host() != Some(host) {
            self.observe_phase("close_old_host_ignored");
            return Ok(());
        }
        self.observe_phase("close_current_host");
        let result = self.stop(Some(app));
        self.ownership.clear_if_host(host);
        self.active_lease = None;
        result
    }

    pub fn release_owner(
        &mut self,
        session_id: &str,
        generation: u64,
        app: &AppHandle,
    ) -> Result<(), String> {
        if !self
            .ownership
            .owner
            .as_ref()
            .is_some_and(|owner| owner.session_id == session_id && owner.generation == generation)
        {
            self.observe_phase("release_stale_ignored");
            return Ok(());
        }
        self.observe_phase("release_current");
        let result = self.stop(Some(app));
        self.ownership.owner = None;
        self.active_lease = None;
        result
    }

    pub fn owner_host(&self) -> Option<&str> {
        self.ownership
            .owner
            .as_ref()
            .map(|owner| owner.host.as_str())
    }

    pub fn owner_is_present(&self, exists: impl FnOnce(&str) -> bool) -> bool {
        self.owner_host().is_some_and(exists)
    }

    pub fn start(&mut self, app: AppHandle) -> Result<(), String> {
        if self.controller_stop.is_some() {
            println!("[Analyzer] Start requested while analyzer is already active.");
            return Ok(());
        }

        let owner = self
            .event_owner()
            .ok_or("Analyzer sem proprietário autorizado.")?;
        let fifo = open_fifo_reader(&self.fifo_path).map_err(|error| {
            analyzer_failed(&app, &owner, "fifo_open_failed", &error);
            error
        })?;
        self.observe_phase("reader_opened");
        println!("[Analyzer] Existing supervisor-owned FIFO opened for reading; inode preserved.");
        let controller_stop = Arc::new(AtomicBool::new(false));
        let reader_stop = Arc::new(AtomicBool::new(false));
        let reader_stop_thread = Arc::clone(&reader_stop);
        let reader_controller_stop = Arc::clone(&controller_stop);
        let reader_app = app.clone();
        let reader_socket_path = self.socket_path.clone();
        let reader_owner = owner.clone();
        self.reader = Some(thread::spawn(move || {
            read_pcm(
                fifo,
                reader_stop_thread,
                reader_controller_stop,
                reader_app,
                reader_socket_path,
                reader_owner,
            );
        }));
        println!("[Analyzer] FIFO reader thread started.");

        let socket_path = self.socket_path.clone();
        self.observe_phase("initial_reconcile");
        if let Err(error) = reconcile_output(&socket_path, &app, &controller_stop, &owner) {
            analyzer_failed(&app, &owner, "initial_reconcile_failed", &error);
            controller_stop.store(true, Ordering::Release);
            reader_stop.store(true, Ordering::Release);
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
            return Err(error);
        }

        let controller_thread_stop = Arc::clone(&controller_stop);
        let controller_app = app.clone();
        self.controller = Some(thread::spawn(move || {
            monitor_player(socket_path, controller_thread_stop, controller_app, owner);
        }));
        println!("[Analyzer] Player controller thread started.");
        self.controller_stop = Some(controller_stop);
        self.reader_stop = Some(reader_stop);
        self.observe_phase("capture_started");
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.controller_stop.is_some()
    }

    pub fn stop(&mut self, app: Option<&AppHandle>) -> Result<(), String> {
        let Some(controller_stop) = self.controller_stop.take() else {
            if let Some(app) = app {
                if let Some(owner) = self.event_owner() {
                    emit_status(app, &owner, false, Some("stopped"));
                }
            }
            return Ok(());
        };

        self.observe_phase("stop_requested");
        println!("[Analyzer] Stop requested; disabling MPD output before reader cleanup.");
        // Invalida apenas o controller: o reader continua drenando o FIFO até o
        // MPD confirmar o disable.
        controller_stop.store(true, Ordering::Release);
        let disable_result = set_output_enabled(
            &self.socket_path,
            crate::supervisor::MpdSupervisor::ANALYZER_OUTPUT_NAME,
            false,
        );
        if let Some(app) = app {
            if let Some(owner) = self.event_owner() {
                emit_status(app, &owner, false, Some("stopped"));
            }
        }
        if let Some(controller) = self.controller.take() {
            let _ = controller.join();
        }
        self.observe_phase("stop_controller_joined");
        if let Some(reader_stop) = self.reader_stop.take() {
            reader_stop.store(true, Ordering::Release);
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.observe_phase("stop_reader_joined");
        println!(
            "[Analyzer] Controller and reader cleanup completed; supervisor-owned FIFO preserved."
        );

        disable_result
    }
}

impl Drop for AudioAnalyzer {
    fn drop(&mut self) {
        if let Err(error) = self.stop(None) {
            eprintln!(
                "[Analyzer] Final cleanup failed: {}",
                crate::diagnostics::error_summary(&error)
            );
        }
    }
}

fn open_fifo_reader(path: &Path) -> Result<File, String> {
    let path_metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "FIFO do analyzer preparado pelo supervisor não está disponível ({}): {}",
            path.display(),
            error
        )
    })?;
    if !path_metadata.file_type().is_fifo() {
        return Err(format!(
            "O caminho preparado pelo supervisor não é um FIFO: {}",
            path.display()
        ));
    }
    let expected_inode = path_metadata.ino();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("Falha ao abrir FIFO do analyzer para leitura: {}", error))?;
    let opened_inode = file
        .metadata()
        .map_err(|error| format!("Falha ao validar descritor do FIFO: {}", error))?
        .ino();
    if opened_inode != expected_inode {
        return Err(format!(
            "O inode do FIFO mudou durante a abertura (esperado {}, aberto {}).",
            expected_inode, opened_inode
        ));
    }
    println!(
        "[Analyzer] Opened existing FIFO: inode={}; no unlink/recreate performed.",
        opened_inode
    );
    Ok(file)
}

fn read_pcm(
    mut fifo: File,
    reader_stop: Arc<AtomicBool>,
    controller_stop: Arc<AtomicBool>,
    app: AppHandle,
    socket_path: String,
    owner: AnalyzerOwner,
) {
    let mut pending = Vec::with_capacity(FRAMES_PER_EVENT * CHANNELS * BYTES_PER_SAMPLE * 2);
    let mut chunk = [0_u8; 8192];
    let window_bytes = FRAMES_PER_EVENT * CHANNELS * BYTES_PER_SAMPLE;
    let mut logged_waiting_for_writer = false;
    let mut logged_first_bytes = false;
    let mut logged_first_frame = false;
    let mut spectrum_analyzer = SpectrumAnalyzer::new();

    while !reader_stop.load(Ordering::Acquire) {
        match fifo.read(&mut chunk) {
            Ok(0) => {
                // Um FIFO O_NONBLOCK retorna 0 enquanto ainda não há writer. Isso
                // não é EOF definitivo: o MPD só abre o writer após enableoutput.
                if !logged_waiting_for_writer {
                    println!("[Analyzer] FIFO reader is waiting for the MPD writer.");
                    logged_waiting_for_writer = true;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Ok(count) => {
                if !logged_first_bytes {
                    println!("[Analyzer] First PCM bytes received ({} bytes).", count);
                    logged_first_bytes = true;
                }
                pending.extend_from_slice(&chunk[..count]);
                while pending.len() >= window_bytes {
                    let mut frame = calculate_levels(&pending[..window_bytes]);
                    frame.spectrum = spectrum_analyzer.analyze_pcm(&pending[..window_bytes]);
                    pending.drain(..window_bytes);
                    if let Some(window) = app.get_webview_window(&owner.host) {
                        match window.emit(
                            LEVEL_EVENT,
                            AnalyzerEnvelope {
                                owner: owner.clone(),
                                data: frame,
                            },
                        ) {
                            Ok(()) if !logged_first_frame => {
                                println!(
                                    "[Analyzer] First RMS/peak/spectrum frame emitted to window '{}'.",
                                    owner.host
                                );
                                logged_first_frame = true;
                            }
                            Ok(()) => {}
                            Err(error) => eprintln!(
                                "[Analyzer] Failed to emit level frame to window '{}': {}",
                                owner.host,
                                crate::diagnostics::error_summary(&error)
                            ),
                        }
                    } else if !logged_first_frame {
                        eprintln!(
                            "[Analyzer] Window '{}' is absent; level frame was not emitted.",
                            owner.host
                        );
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                eprintln!(
                    "[Analyzer] Failed to read PCM from FIFO: {}",
                    crate::diagnostics::error_summary(&error)
                );
                reader_stop.store(true, Ordering::Release);
                controller_stop.store(true, Ordering::Release);
                if let Err(disable_error) = set_output_enabled(
                    &socket_path,
                    crate::supervisor::MpdSupervisor::ANALYZER_OUTPUT_NAME,
                    false,
                ) {
                    eprintln!(
                        "[Analyzer] Failed to disable output after read error: {}",
                        crate::diagnostics::error_summary(&disable_error)
                    );
                }
                emit_status(&app, &owner, false, Some("reader_error"));
                break;
            }
        }
    }
}

fn calculate_levels(bytes: &[u8]) -> AudioLevelFrame {
    let mut sum_squares = [0.0_f64; 2];
    let mut peaks = [0.0_f32; 2];
    let mut counts = [0_usize; 2];
    for (index, sample) in bytes.chunks_exact(2).enumerate() {
        let channel = index % 2;
        let value = i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0;
        let magnitude = value.abs().min(1.0);
        sum_squares[channel] += f64::from(value) * f64::from(value);
        peaks[channel] = peaks[channel].max(magnitude);
        counts[channel] += 1;
    }
    let rms = |channel: usize| {
        if counts[channel] == 0 {
            0.0
        } else {
            (sum_squares[channel] / counts[channel] as f64)
                .sqrt()
                .min(1.0) as f32
        }
    };
    AudioLevelFrame {
        left_rms: rms(0),
        right_rms: rms(1),
        left_peak: peaks[0],
        right_peak: peaks[1],
        spectrum: Vec::new(),
    }
}

fn monitor_player(
    socket_path: String,
    stop: Arc<AtomicBool>,
    app: AppHandle,
    owner: AnalyzerOwner,
) {
    let mut needs_reconcile = false;
    while !stop.load(Ordering::Acquire) {
        if needs_reconcile {
            match reconcile_output(&socket_path, &app, &stop, &owner) {
                Ok(()) => needs_reconcile = false,
                Err(_) => {
                    emit_status(&app, &owner, false, Some("mpd_unavailable"));
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            }
        }
        match wait_for_analyzer_change(&socket_path) {
            Ok(Some(change)) => {
                if change == AnalyzerChange::Player {
                    crate::diagnostics::now_playing_seek_controller(&owner.host, owner.generation);
                }
                if !stop.load(Ordering::Acquire) {
                    if let Err(error) = reconcile_output(&socket_path, &app, &stop, &owner) {
                        eprintln!(
                            "[Analyzer] Failed to reevaluate current track: {}",
                            crate::diagnostics::error_summary(&error)
                        );
                        emit_status(&app, &owner, false, Some("reconcile_failed"));
                    }
                }
            }
            Ok(None) => {}
            Err(_) => {
                emit_status(&app, &owner, false, Some("mpd_unavailable"));
                needs_reconcile = true;
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn reconcile_output(
    socket_path: &str,
    app: &AppHandle,
    stop: &AtomicBool,
    owner: &AnalyzerOwner,
) -> Result<(), String> {
    let reason = reconcile_output_with_io(stop, &mut |command| send_command(socket_path, command))?;
    emit_status(app, owner, reason.is_none(), reason);
    Ok(())
}

// A player event includes seek, pause and tags. Re-read compatibility on every
// event, but change only an auxiliary output whose actual state is incorrect.
fn reconcile_output_with_io(
    stop: &AtomicBool,
    send: &mut impl FnMut(&str) -> Result<Vec<String>, String>,
) -> Result<Option<&'static str>, String> {
    let assessment = (|| {
        if stop.load(Ordering::Acquire) {
            Ok(Some("stopped"))
        } else {
            let status = send("status")?;
            let song = send("currentsong")?;
            Ok(playback_incompatibility(&status, &song))
        }
    })();
    let mut reason = assessment.map_err(|error| disable_after_failure(error, stop, send))?;
    if let Err(error) = ensure_auxiliary_output(reason.is_none(), stop, send) {
        return Err(if reason.is_none() {
            disable_after_failure(error, stop, send)
        } else {
            error // Do not retry a refused disable or obscure its failure.
        });
    }
    if stop.load(Ordering::Acquire) && reason.is_none() {
        // Covers stop/reader failure arriving while enable's ACK is in flight.
        // The old controller is joined before any new owner starts.
        ensure_auxiliary_output(false, stop, send)?;
        reason = Some("stopped");
    }
    Ok(reason)
}

fn disable_after_failure(
    error: String,
    stop: &AtomicBool,
    send: &mut impl FnMut(&str) -> Result<Vec<String>, String>,
) -> String {
    // Fail closed after failed reads/enable, while preserving the original
    // failure. This is a disable, never another attempt to enable or seek.
    match ensure_auxiliary_output(false, stop, send) {
        Ok(()) => error,
        Err(cleanup) => format!("{}; falha ao desativar analyzer: {}", error, cleanup),
    }
}

fn ensure_auxiliary_output(
    requested: bool,
    stop: &AtomicBool,
    send: &mut impl FnMut(&str) -> Result<Vec<String>, String>,
) -> Result<(), String> {
    let name = crate::supervisor::MpdSupervisor::ANALYZER_OUTPUT_NAME;
    let (id, actual) = auxiliary_output_state(&send("outputs")?, name)?;
    let desired = requested && !stop.load(Ordering::Acquire);
    if actual == desired {
        return Ok(());
    }
    crate::diagnostics::now_playing_auxiliary(if desired {
        "enable_begin"
    } else {
        "disable_begin"
    });
    let result = send(&format!(
        "{} {}",
        if desired {
            "enableoutput"
        } else {
            "disableoutput"
        },
        id
    ));
    crate::diagnostics::now_playing_auxiliary(match (desired, result.is_ok()) {
        (true, true) => "enable_accepted",
        (true, false) => "enable_failed",
        (false, true) => "disable_accepted",
        (false, false) => "disable_failed",
    });
    result?;
    let (_, confirmed) = auxiliary_output_state(&send("outputs")?, name)?;
    if confirmed != desired && !(desired && !confirmed && stop.load(Ordering::Acquire)) {
        return Err("Estado da saída auxiliar não confirmou a transição aceita pelo MPD.".into());
    }
    Ok(())
}

fn auxiliary_output_state(lines: &[String], name: &str) -> Result<(u32, bool), String> {
    let mut matches = lines
        .split(|line| line.starts_with("outputid: "))
        .skip(1)
        .zip(
            lines
                .iter()
                .filter_map(|line| line.strip_prefix("outputid: ")),
        )
        .filter(|(block, _)| {
            block
                .iter()
                .any(|line| line.strip_prefix("outputname: ") == Some(name))
        });
    let (block, id) = matches.next().ok_or("Saída auxiliar MPD não encontrada.")?;
    if matches.next().is_some() {
        return Err("Identidade da saída auxiliar MPD ambígua.".into());
    }
    let id = id
        .parse()
        .map_err(|_| "Identificador inválido da saída auxiliar MPD.")?;
    if block
        .iter()
        .filter(|line| line.starts_with("outputname: "))
        .count()
        != 1
        || lines
            .iter()
            .filter_map(|line| line.strip_prefix("outputid: "))
            .filter(|value| value.parse::<u32>().ok() == Some(id))
            .count()
            != 1
    {
        return Err("Identidade da saída auxiliar MPD ambígua.".into());
    }
    let mut states = block
        .iter()
        .filter_map(|line| line.strip_prefix("outputenabled: "));
    let enabled = match states.next() {
        Some("0") => false,
        Some("1") => true,
        _ => return Err("Estado da saída auxiliar MPD ausente ou inválido.".into()),
    };
    if states.next().is_some() {
        return Err("Estado da saída auxiliar MPD ambíguo.".into());
    }
    Ok((id, enabled))
}

fn emit_status(app: &AppHandle, owner: &AnalyzerOwner, available: bool, reason: Option<&str>) {
    if let Some(window) = app.get_webview_window(&owner.host) {
        let status = AnalyzerStatus {
            available,
            reason: reason.map(str::to_string),
        };
        if let Err(error) = window.emit(
            STATUS_EVENT,
            AnalyzerEnvelope {
                owner: owner.clone(),
                data: status,
            },
        ) {
            eprintln!(
                "[Analyzer] Failed to emit status to window '{}': {}",
                owner.host,
                crate::diagnostics::error_summary(&error)
            );
        }
    } else {
        eprintln!(
            "[Analyzer] Window '{}' is absent; status available={} reason={:?} was not emitted.",
            owner.host, available, reason
        );
    }
}

fn analyzer_failed(app: &AppHandle, owner: &AnalyzerOwner, reason: &str, error: &str) {
    eprintln!(
        "[Analyzer] Analyzer inactive: reason={}: {}",
        reason,
        crate::diagnostics::error_summary(&error)
    );
    emit_status(app, owner, false, Some(reason));
}

fn unique_value<'a>(lines: &'a [String], prefix: &str) -> Option<&'a str> {
    let mut values = lines.iter().filter_map(|line| line.strip_prefix(prefix));
    let value = values.next()?;
    (values.next().is_none() && !value.is_empty()).then_some(value)
}

fn playback_incompatibility(status: &[String], song: &[String]) -> Option<&'static str> {
    if let Some(reason) = dsd_or_dop_reason(status, song) {
        return Some(reason);
    }
    if unique_value(status, "state: ") == Some("stop") {
        return Some("stopped");
    }
    // Do not mistake missing/contradictory metadata for evidence of PCM. IDs
    // also detect a real track change between the two separate MPD queries.
    let status_id = unique_value(status, "songid: ").and_then(|id| id.parse::<u32>().ok());
    let song_id = unique_value(song, "Id: ").and_then(|id| id.parse::<u32>().ok());
    if !matches!(unique_value(status, "state: "), Some("play" | "pause"))
        || status_id.is_none()
        || status_id != song_id
        || unique_value(song, "file: ").is_none()
        || status.iter().any(|line| line.starts_with("error: "))
        || !unique_value(status, "audio: ").is_some_and(is_pcm_format)
    {
        return Some("reconcile_failed");
    }
    None
}

fn is_pcm_format(value: &str) -> bool {
    let fields: Vec<_> = value.split(':').collect();
    fields.len() == 3
        && fields[0].parse::<u32>().is_ok_and(|rate| rate > 0)
        && matches!(fields[1], "8" | "16" | "24" | "32" | "f")
        && fields[2].parse::<u8>().is_ok_and(|channels| channels > 0)
}

fn dsd_or_dop_reason(status: &[String], current_song: &[String]) -> Option<&'static str> {
    let format_is_dsd = status
        .iter()
        .any(|line| line.strip_prefix("audio: ").is_some_and(is_dsd_or_dop))
        || current_song
            .iter()
            .any(|line| line.strip_prefix("Format: ").is_some_and(is_dsd_or_dop));
    let file_is_dsd = current_song.iter().any(|line| {
        line.strip_prefix("file: ").is_some_and(|value| {
            let clean = value.split(['?', '#']).next().unwrap_or(value);
            matches!(
                clean
                    .rsplit('.')
                    .next()
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("dsf" | "dff" | "dsd")
            )
        })
    });
    if format_is_dsd {
        Some("current_audio_format_is_dsd")
    } else if file_is_dsd {
        Some("current_file_is_dsd")
    } else {
        None
    }
}

fn is_dsd_or_dop(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("dsd") || value.contains("dop")
}

#[derive(Debug, PartialEq, Eq)]
enum AnalyzerChange {
    Player,
    Output,
}

fn analyzer_change(lines: &[String]) -> Option<AnalyzerChange> {
    if lines.iter().any(|line| line == "changed: player") {
        Some(AnalyzerChange::Player)
    } else if lines.iter().any(|line| line == "changed: output") {
        Some(AnalyzerChange::Output)
    } else {
        None
    }
}

fn wait_for_analyzer_change(socket_path: &str) -> Result<Option<AnalyzerChange>, String> {
    let response = send_command_with_timeout(
        socket_path,
        "idle player output",
        Duration::from_millis(500),
    );
    match response {
        Ok(lines) => Ok(analyzer_change(&lines)),
        Err(error) if error.contains("temporariamente indisponível") => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn set_output_enabled(socket_path: &str, name: &str, enabled: bool) -> Result<(), String> {
    let observed = name == crate::supervisor::MpdSupervisor::ANALYZER_OUTPUT_NAME;
    if observed {
        crate::diagnostics::now_playing_auxiliary(if enabled {
            "enable_begin"
        } else {
            "disable_begin"
        });
    }
    let result = set_output_enabled_with_protocol(socket_path, name, enabled);
    if observed {
        crate::diagnostics::now_playing_auxiliary(match (enabled, result.is_ok()) {
            (true, true) => "enable_accepted",
            (true, false) => "enable_failed",
            (false, true) => "disable_accepted",
            (false, false) => "disable_failed",
        });
    }
    result
}

fn set_output_enabled_with_protocol(
    socket_path: &str,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let outputs = send_command(socket_path, "outputs")?;
    let output_id = find_output_id(&outputs, name)
        .ok_or_else(|| format!("Output MPD '{}' não encontrado.", name))?;
    println!(
        "[Analyzer] MPD output '{}' found with id {}.",
        name, output_id
    );
    let command = if enabled {
        "enableoutput"
    } else {
        "disableoutput"
    };
    send_command(socket_path, &format!("{} {}", command, output_id))?;
    println!(
        "[Analyzer] MPD accepted {} for output '{}' (id {}).",
        command, name, output_id
    );
    Ok(())
}

fn find_output_id(lines: &[String], target_name: &str) -> Option<u32> {
    let mut id = None;
    for line in lines {
        if let Some(value) = line.strip_prefix("outputid: ") {
            id = value.parse().ok();
        } else if line.strip_prefix("outputname: ") == Some(target_name) {
            return id;
        }
    }
    None
}

fn send_command(socket_path: &str, command: &str) -> Result<Vec<String>, String> {
    send_command_with_timeout(socket_path, command, Duration::from_millis(500))
}

fn send_command_with_timeout(
    socket_path: &str,
    command: &str,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|error| format!("Socket MPD do analyzer indisponível: {}", error))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("Falha ao configurar timeout do analyzer: {}", error))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| format!("Falha ao configurar timeout do analyzer: {}", error))?;
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|error| format!("Falha ao preparar conexão do analyzer: {}", error))?,
    );
    let mut greeting = String::new();
    reader
        .read_line(&mut greeting)
        .map_err(|error| format!("Falha no handshake MPD do analyzer: {}", error))?;
    if !greeting.starts_with("OK MPD ") {
        return Err("Handshake MPD inválido no analyzer.".to_string());
    }
    stream
        .write_all(format!("{}\n", command).as_bytes())
        .map_err(|error| format!("Falha ao enviar comando do analyzer: {}", error))?;
    stream
        .flush()
        .map_err(|error| format!("Falha ao concluir comando do analyzer: {}", error))?;

    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Err("EOF antes da confirmação MPD do analyzer.".to_string()),
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err("Resposta MPD do analyzer temporariamente indisponível.".to_string())
            }
            Err(error) => return Err(format!("Falha ao ler resposta MPD do analyzer: {}", error)),
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line == "OK" {
            return Ok(lines);
        }
        if line.starts_with("ACK") {
            return Err(format!("MPD recusou comando do analyzer: {}", line));
        }
        lines.push(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static NEXT_FIFO_TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct ReconcileIo {
        status: Vec<String>,
        song: Vec<String>,
        enabled: bool,
        commands: Vec<String>,
        fail: Option<&'static str>,
        ignore_enable: bool,
    }

    impl ReconcileIo {
        fn pcm(enabled: bool) -> Self {
            Self {
                status: vec![
                    "state: play".into(),
                    "songid: 42".into(),
                    "song: 0".into(),
                    "audio: 44100:16:2".into(),
                    "elapsed: 192.976".into(),
                    "duration: 445.215".into(),
                ],
                song: vec!["file: fixture/track.flac".into(), "Id: 42".into()],
                enabled,
                commands: Vec::new(),
                fail: None,
                ignore_enable: false,
            }
        }

        fn send(&mut self, command: &str) -> Result<Vec<String>, String> {
            self.commands.push(command.into());
            if self.fail == Some(command) {
                self.fail = None;
                return Err("ACK [5@0] {fixture} injected failure".into());
            }
            match command {
                "status" => Ok(self.status.clone()),
                "currentsong" => Ok(self.song.clone()),
                "outputs" => Ok(vec![
                    "outputid: 7".into(),
                    "outputname: Sonante Output".into(),
                    "outputenabled: 1".into(),
                    "outputid: 12".into(),
                    "outputname: Sonante Analyzer".into(),
                    format!("outputenabled: {}", u8::from(self.enabled)),
                ]),
                "enableoutput 12" => {
                    self.enabled = !self.ignore_enable;
                    Ok(vec![])
                }
                "disableoutput 12" => {
                    self.enabled = false;
                    Ok(vec![])
                }
                other => panic!("Unexpected transport or primary-output command: {other}"),
            }
        }

        fn reconcile(&mut self, stop: &AtomicBool) -> Result<Option<&'static str>, String> {
            reconcile_output_with_io(stop, &mut |command| self.send(command))
        }

        fn transitions(&self) -> Vec<&str> {
            self.commands
                .iter()
                .filter(|command| command.ends_with("output 12"))
                .map(String::as_str)
                .collect()
        }
    }

    #[test]
    fn observed_seek_events_and_frozen_tail_do_not_cycle_compatible_auxiliary_output() {
        let stop = AtomicBool::new(false);
        let mut io = ReconcileIo::pcm(false);
        assert_eq!(io.reconcile(&stop).unwrap(), None);
        // Seek 1 occurred before analyzer reservation. These are the received
        // seek 2-5 samples, with synthetic PCM format/IDs (absent from that log).
        // Repeated player events deliberately include pause-recovery/tag events.
        // They test output reconciliation, not audible or sustained progress.
        for samples in [
            &[198.982, 198.982, 199.788, 200.103, 200.847, 201.853][..],
            &[35.073, 35.157, 35.482][..],
            &[97.573, 97.657, 97.982, 97.982][..],
            &[210.573, 210.657, 210.982, 210.982][..],
        ] {
            for elapsed in samples {
                io.status[4] = format!("elapsed: {elapsed:.3}");
                let snapshot = (io.status.clone(), io.song.clone());
                assert_eq!(
                    analyzer_change(&["changed: player".into()]),
                    Some(AnalyzerChange::Player)
                );
                assert_eq!(io.reconcile(&stop).unwrap(), None);
                assert_eq!((io.status.clone(), io.song.clone()), snapshot);
            }
        }
        assert!(io.enabled);
        assert_eq!(io.status[4], "elapsed: 210.982");
        assert_eq!(io.transitions(), ["enableoutput 12"]);
    }

    #[test]
    fn pause_resume_tags_and_new_compatible_pcm_do_not_cycle_output() {
        let stop = AtomicBool::new(false);
        let mut io = ReconcileIo::pcm(true);
        for state in ["pause", "play", "pause", "play"] {
            io.status[0] = format!("state: {state}");
            assert_eq!(io.reconcile(&stop).unwrap(), None);
        }
        io.song.push("Title: synthetic changed tag".into());
        assert_eq!(io.reconcile(&stop).unwrap(), None);
        io.status[1] = "songid: 43".into();
        io.status[3] = "audio: 192000:24:2".into();
        io.song = vec![
            "file: https://fixture.invalid/stream".into(),
            "Id: 43".into(),
        ];
        assert_eq!(io.reconcile(&stop).unwrap(), None);
        assert!(io.transitions().is_empty());
    }

    #[test]
    fn real_pcm_dsd_dop_and_stopped_changes_apply_only_needed_transitions() {
        let stop = AtomicBool::new(false);
        let mut io = ReconcileIo::pcm(true);
        io.status[3] = "audio: dsd64:2".into();
        assert_eq!(
            io.reconcile(&stop).unwrap(),
            Some("current_audio_format_is_dsd")
        );
        assert_eq!(
            io.reconcile(&stop).unwrap(),
            Some("current_audio_format_is_dsd")
        );
        io.status[3] = "audio: 48000:24:2".into();
        io.song[0] = "file: fixture/track.DSF?redacted".into();
        assert_eq!(io.reconcile(&stop).unwrap(), Some("current_file_is_dsd"));
        io.song[0] = "file: fixture/track.flac".into();
        assert_eq!(io.reconcile(&stop).unwrap(), None);
        io.song.push("Format: DoP:2".into());
        assert_eq!(
            io.reconcile(&stop).unwrap(),
            Some("current_audio_format_is_dsd")
        );
        io.song.pop();
        assert_eq!(io.reconcile(&stop).unwrap(), None);
        io.status[0] = "state: stop".into();
        assert_eq!(io.reconcile(&stop).unwrap(), Some("stopped"));
        assert_eq!(io.reconcile(&stop).unwrap(), Some("stopped"));
        assert_eq!(
            io.transitions(),
            [
                "disableoutput 12",
                "enableoutput 12",
                "disableoutput 12",
                "enableoutput 12",
                "disableoutput 12"
            ]
        );
    }

    #[test]
    fn missing_malformed_or_incoherent_metadata_is_not_assumed_pcm() {
        let stop = AtomicBool::new(false);
        for field in [
            "audio",
            "state",
            "songid",
            "file",
            "Id",
            "mismatch",
            "invalid",
            "error",
            "duplicate",
        ] {
            let mut io = ReconcileIo::pcm(true);
            match field {
                "audio" | "state" | "songid" => io
                    .status
                    .retain(|line| !line.starts_with(&format!("{field}: "))),
                "file" | "Id" => io
                    .song
                    .retain(|line| !line.starts_with(&format!("{field}: "))),
                "mismatch" => io.song[1] = "Id: 99".into(),
                "invalid" => io.status[3] = "audio: 48000:*:2".into(),
                "error" => io.status.push("error: synthetic playback failure".into()),
                "duplicate" => io.status.push("audio: 96000:24:2".into()),
                _ => unreachable!(),
            }
            assert_eq!(
                io.reconcile(&stop).unwrap(),
                Some("reconcile_failed"),
                "{field}"
            );
            assert_eq!(io.reconcile(&stop).unwrap(), Some("reconcile_failed"));
            assert!(!io.enabled);
            assert_eq!(io.transitions(), ["disableoutput 12"]);
        }
        for format in ["44100:16:2", "192000:24:2", "48000:f:2"] {
            assert!(is_pcm_format(format));
        }
        for format in [
            "",
            "dsd64:2",
            "48000:dop:2",
            "0:16:2",
            "48000:16:0",
            "48000:16:2:1",
        ] {
            assert!(!is_pcm_format(format));
        }
    }

    #[test]
    fn external_output_changes_are_reconciled_by_actual_state_and_name() {
        let stop = AtomicBool::new(false);
        let mut io = ReconcileIo::pcm(true);
        assert_eq!(io.reconcile(&stop).unwrap(), None);
        assert_eq!(
            analyzer_change(&["changed: output".into()]),
            Some(AnalyzerChange::Output)
        );
        io.enabled = false;
        assert_eq!(io.reconcile(&stop).unwrap(), None);
        assert_eq!(io.reconcile(&stop).unwrap(), None); // our own output event
        io.status[3] = "audio: dsd128:2".into();
        io.enabled = true; // external enable while incompatible
        assert_eq!(
            io.reconcile(&stop).unwrap(),
            Some("current_audio_format_is_dsd")
        );
        assert_eq!(io.transitions(), ["enableoutput 12", "disableoutput 12"]);
        assert_eq!(analyzer_change(&["changed: mixer".into()]), None);
    }

    #[test]
    fn read_and_enable_failures_fail_closed_without_retrying_enable_or_transport() {
        let stop = AtomicBool::new(false);
        for command in ["status", "currentsong", "outputs"] {
            let mut io = ReconcileIo::pcm(true);
            io.fail = Some(command);
            let error = io.reconcile(&stop).unwrap_err();
            assert!(error.contains("ACK [5@0]"));
            assert!(!io.enabled);
            assert_eq!(io.transitions(), ["disableoutput 12"]);
        }
        let mut io = ReconcileIo::pcm(false);
        io.fail = Some("enableoutput 12");
        assert!(io.reconcile(&stop).unwrap_err().contains("ACK [5@0]"));
        assert!(!io.enabled);
        assert_eq!(io.transitions(), ["enableoutput 12"]);
        io.ignore_enable = true;
        assert!(io.reconcile(&stop).unwrap_err().contains("não confirmou"));
        assert!(!io.enabled);
    }

    #[test]
    fn refused_disable_remains_explicit_and_is_not_retried_as_success() {
        let stop = AtomicBool::new(false);
        let mut io = ReconcileIo::pcm(true);
        io.status[3] = "audio: dsd64:2".into();
        io.fail = Some("disableoutput 12");
        assert!(io.reconcile(&stop).unwrap_err().contains("ACK [5@0]"));
        assert!(io.enabled);
        assert_eq!(io.transitions(), ["disableoutput 12"]);
    }

    #[test]
    fn failure_to_confirm_enable_disables_it_and_preserves_transport_failure() {
        let stop = AtomicBool::new(false);
        let mut io = ReconcileIo::pcm(false);
        let mut output_reads = 0;
        let error = reconcile_output_with_io(&stop, &mut |command| {
            if command == "outputs" {
                output_reads += 1;
                if output_reads == 2 {
                    return Err("EOF before confirmation fixture".into());
                }
            }
            io.send(command)
        })
        .unwrap_err();
        assert!(error.contains("EOF before confirmation"));
        assert!(!io.enabled);
        assert_eq!(io.transitions(), ["enableoutput 12", "disableoutput 12"]);
    }

    #[test]
    fn persistent_unavailable_or_ambiguous_output_does_not_issue_guessed_commands() {
        let stop = AtomicBool::new(false);
        for unavailable in [true, false] {
            let mut io = ReconcileIo::pcm(true);
            let error = reconcile_output_with_io(&stop, &mut |command| {
                if command == "outputs" {
                    if unavailable {
                        return Err("timeout fixture".into());
                    }
                    return Ok(vec![
                        "outputid: 7".into(),
                        "outputname: Sonante Output".into(),
                        "outputenabled: 1".into(),
                    ]);
                }
                io.send(command)
            })
            .unwrap_err();
            assert!(error.contains("falha ao desativar analyzer"));
            assert!(io.transitions().is_empty());
        }
    }

    #[test]
    fn concurrent_stop_disable_is_not_mistaken_for_failed_enable_confirmation() {
        let stop = AtomicBool::new(false);
        let mut io = ReconcileIo::pcm(false);
        let result = reconcile_output_with_io(&stop, &mut |command| {
            let result = io.send(command);
            if command == "enableoutput 12" {
                // Model the explicit stop's disable completing before the
                // controller reads back its in-flight enable acknowledgement.
                stop.store(true, Ordering::Release);
                io.send("disableoutput 12").unwrap();
            }
            result
        })
        .unwrap();
        assert_eq!(result, Some("stopped"));
        assert!(!io.enabled);
        assert_eq!(io.transitions(), ["enableoutput 12", "disableoutput 12"]);
    }

    #[test]
    fn malformed_output_identity_or_state_never_targets_primary_output() {
        let base: Vec<String> = [
            "outputid: 12",
            "outputname: Sonante Analyzer",
            "outputenabled: 1",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        let name = crate::supervisor::MpdSupervisor::ANALYZER_OUTPUT_NAME;
        assert_eq!(auxiliary_output_state(&base, name).unwrap(), (12, true));
        let mut variants = vec![vec![], base[..2].to_vec()];
        let mut invalid = base.clone();
        invalid[2] = "outputenabled: unknown".into();
        variants.push(invalid);
        let mut duplicate = base.clone();
        duplicate.extend(base.clone());
        variants.push(duplicate);
        let mut duplicate = base.clone();
        duplicate.push("outputenabled: 0".into());
        variants.push(duplicate);
        let mut collision = base.clone();
        collision.extend([
            "outputid: 12".into(),
            "outputname: Sonante Output".into(),
            "outputenabled: 1".into(),
        ]);
        variants.push(collision);
        for lines in variants {
            assert!(auxiliary_output_state(&lines, name).is_err());
        }
    }

    #[test]
    fn shutdown_during_reads_or_enable_ack_cannot_leave_old_controller_enabled() {
        for stage in ["status", "currentsong", "outputs", "enableoutput 12"] {
            let stop = AtomicBool::new(false);
            let mut io = ReconcileIo::pcm(false);
            let result = reconcile_output_with_io(&stop, &mut |command| {
                let result = io.send(command);
                if command == stage {
                    stop.store(true, Ordering::Release);
                }
                result
            })
            .unwrap();
            assert_eq!(result, Some("stopped"));
            assert!(!io.enabled, "{stage}");
            if stage == "enableoutput 12" {
                assert_eq!(io.transitions(), ["enableoutput 12", "disableoutput 12"]);
            } else {
                assert!(io.transitions().is_empty());
            }
        }
    }

    #[test]
    fn transfer_keeps_explicit_disable_and_new_lease_rejects_stale_cleanup() {
        let mut analyzer = AudioAnalyzer::new("/missing/socket".into(), "/missing/fifo".into());
        let old = analyzer
            .ownership
            .claim("now-playing", "old-session".into());
        analyzer.active_lease = Some("old-lease".into());
        let old_stop = AtomicBool::new(true);
        let mut io = ReconcileIo::pcm(true);
        assert_eq!(io.reconcile(&old_stop).unwrap(), Some("stopped"));
        assert!(!io.enabled);
        let new = analyzer.ownership.claim("main", "new-session".into());
        analyzer.active_lease = Some("new-lease".into());
        let new_stop = AtomicBool::new(false);
        assert_eq!(io.reconcile(&new_stop).unwrap(), None);
        assert!(!analyzer.capture_matches(&old.host, &old.session_id, old.generation, "old-lease"));
        assert!(!analyzer.capture_matches(&new.host, &new.session_id, new.generation, "old-lease"));
        assert!(!analyzer.ownership.clear_if_host("now-playing"));
        assert!(analyzer.capture_matches(&new.host, &new.session_id, new.generation, "new-lease"));
        assert_eq!(io.reconcile(&new_stop).unwrap(), None);
        assert_eq!(io.transitions(), ["disableoutput 12", "enableoutput 12"]);
    }

    #[test]
    fn host_generations_reject_old_start_cleanup_and_close() {
        let mut ownership = AnalyzerOwnership::default();
        let old = ownership.claim("now-playing", "old".into());
        let new = ownership.claim("main", "new".into());
        assert!(!ownership.matches(&old.host, &old.session_id, old.generation));
        assert!(ownership.matches(&new.host, &new.session_id, new.generation));
        assert!(!ownership.matches("now-playing", &new.session_id, new.generation));
        let replay = ownership.claim("main", "new".into());
        assert!(!ownership.matches(&new.host, &new.session_id, new.generation));
        assert!(ownership.matches(&replay.host, &replay.session_id, replay.generation));
        assert!(!ownership.clear_if_host("now-playing"));
        assert!(ownership.matches(&replay.host, &replay.session_id, replay.generation));
        assert!(ownership.clear_if_host("main"));
        assert!(ownership.owner.is_none());
        assert!(authorized_analyzer_host("main"));
        assert!(authorized_analyzer_host("now-playing"));
        assert!(!authorized_analyzer_host("settings"));
    }

    #[test]
    fn closing_old_host_cannot_clear_new_owner_and_inactive_stop_is_idempotent() {
        let mut analyzer =
            AudioAnalyzer::new("/missing/mpd.socket".into(), "/missing/analyzer.pcm".into());
        analyzer.ownership.claim("main", "new-window".into());
        assert_ne!(analyzer.owner_host(), Some("now-playing"));
        assert_eq!(analyzer.owner_host(), Some("main"));
        analyzer.stop(None).unwrap();
        analyzer.stop(None).unwrap();
        assert_eq!(analyzer.owner_host(), Some("main"));
    }

    #[test]
    fn configuration_resume_follows_retained_owner_and_revocation() {
        let mut analyzer =
            AudioAnalyzer::new("/missing/mpd.socket".into(), "/missing/analyzer.pcm".into());
        analyzer.ownership.claim("main", "embedded".into());
        analyzer.stop(None).unwrap();
        assert!(analyzer.owner_is_present(|host| host == "main"));
        assert!(!analyzer.owner_is_present(|host| host == "now-playing"));
        assert!(!analyzer.ownership.clear_if_host("now-playing"));
        assert!(analyzer.owner_is_present(|host| host == "main"));
        analyzer.ownership.clear_if_host("main");
        assert!(!analyzer.owner_is_present(|_| true));
    }

    #[test]
    fn strict_mode_capture_lease_rejects_old_cleanup_and_tags_events() {
        let mut analyzer =
            AudioAnalyzer::new("/missing/mpd.socket".into(), "/missing/analyzer.pcm".into());
        let owner = analyzer.ownership.claim("main", "same-session".into());
        analyzer.active_lease = Some("new-cycle".into());
        assert!(!analyzer.capture_matches(
            "main",
            &owner.session_id,
            owner.generation,
            "old-cycle"
        ));
        assert!(analyzer.capture_matches("main", &owner.session_id, owner.generation, "new-cycle"));
        let target = analyzer.event_owner().unwrap();
        assert_eq!(target.host, "main");
        assert_eq!(target.lease_id.as_deref(), Some("new-cycle"));
        let payload = serde_json::to_value(AnalyzerEnvelope {
            owner: target,
            data: AnalyzerStatus {
                available: false,
                reason: Some("stopped".into()),
            },
        })
        .unwrap();
        assert_eq!(payload["owner"]["leaseId"], "new-cycle");
        assert_eq!(payload["owner"]["generation"], owner.generation);
    }

    fn pcm(samples: &[(i16, i16)]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|(left, right)| left.to_le_bytes().into_iter().chain(right.to_le_bytes()))
            .collect()
    }

    #[test]
    fn silence_has_zero_rms_and_peak() {
        assert_eq!(
            calculate_levels(&pcm(&[(0, 0); 8])),
            AudioLevelFrame {
                left_rms: 0.0,
                right_rms: 0.0,
                left_peak: 0.0,
                right_peak: 0.0,
                spectrum: Vec::new(),
            }
        );
    }

    fn sine_pcm(frequency: f32, amplitude: f32, frames: usize) -> Vec<u8> {
        (0..frames)
            .flat_map(|index| {
                let phase =
                    2.0 * std::f32::consts::PI * frequency * index as f32 / SAMPLE_RATE as f32;
                let sample = (phase.sin() * amplitude * i16::MAX as f32) as i16;
                sample.to_le_bytes().into_iter().chain(sample.to_le_bytes())
            })
            .collect()
    }

    #[test]
    fn one_kilohertz_sine_is_dominant_in_its_logarithmic_band() {
        let mut analyzer = SpectrumAnalyzer::new();
        let spectrum = analyzer.analyze_pcm(&sine_pcm(1_000.0, 0.8, FFT_SIZE));
        let dominant = spectrum
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(dominant, frequency_to_band(1_000.0).unwrap());
    }

    #[test]
    fn spectrum_silence_is_zero() {
        let mut analyzer = SpectrumAnalyzer::new();
        let spectrum = analyzer.analyze_pcm(&pcm(&vec![(0, 0); FFT_SIZE]));
        assert!(spectrum.iter().all(|value| *value == 0.0));
    }

    #[test]
    fn strong_sine_spectrum_is_finite_and_normalized() {
        let mut analyzer = SpectrumAnalyzer::new();
        let spectrum = analyzer.analyze_pcm(&sine_pcm(997.0, 1.0, FFT_SIZE));
        assert!(spectrum
            .iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(value)));
        assert!(spectrum.iter().any(|value| *value > 0.9));
    }

    #[test]
    fn frequency_mapping_uses_bounded_logarithmic_bands() {
        assert_eq!(frequency_to_band(39.9), None);
        assert_eq!(frequency_to_band(40.0), Some(0));
        assert_eq!(frequency_to_band(20_000.0), Some(SPECTRUM_BANDS - 1));
        assert_eq!(frequency_to_band(20_001.0), None);
        let low = frequency_to_band(100.0).unwrap();
        let middle = frequency_to_band(1_000.0).unwrap();
        let high = frequency_to_band(10_000.0).unwrap();
        assert!(low < middle && middle < high);
        assert!((middle - low).abs_diff(high - middle) <= 1);
    }

    #[test]
    fn known_samples_have_real_per_channel_rms_and_peak() {
        let levels = calculate_levels(&pcm(&[(16_384, 8_192), (-16_384, -8_192)]));
        assert!((levels.left_rms - 0.5).abs() < 0.0001);
        assert!((levels.right_rms - 0.25).abs() < 0.0001);
        assert!((levels.left_peak - 0.5).abs() < 0.0001);
        assert!((levels.right_peak - 0.25).abs() < 0.0001);
    }

    #[test]
    fn full_scale_and_clipping_boundaries_are_normalized() {
        let levels = calculate_levels(&pcm(&[(i16::MIN, i16::MAX), (i16::MIN, i16::MAX)]));
        assert_eq!(levels.left_rms, 1.0);
        assert_eq!(levels.left_peak, 1.0);
        assert!(levels.right_rms <= 1.0);
        assert!(levels.right_peak <= 1.0);
    }

    #[test]
    fn dsd_is_detected_from_format_or_file_without_treating_pcm_as_dsd() {
        assert_eq!(
            dsd_or_dop_reason(&["audio: dsd64:2".into()], &[]),
            Some("current_audio_format_is_dsd")
        );
        assert_eq!(
            dsd_or_dop_reason(&[], &["file: album/track.DSF?token=redacted".into()]),
            Some("current_file_is_dsd")
        );
        assert_eq!(
            dsd_or_dop_reason(
                &["audio: 48000:24:2".into()],
                &["file: album/track.flac".into()]
            ),
            None
        );
    }

    #[test]
    fn analyzer_output_is_found_by_name_not_position() {
        let lines = vec![
            "outputid: 7".into(),
            "outputname: Sonante Output".into(),
            "outputid: 12".into(),
            "outputname: Sonante Analyzer".into(),
        ];
        assert_eq!(find_output_id(&lines, "Sonante Analyzer"), Some(12));
    }

    #[test]
    fn opening_and_closing_reader_preserves_fifo_inode_and_survives_late_writer() {
        let id = NEXT_FIFO_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("analyzer-tests")
            .join(format!("{}-{}", std::process::id(), id));
        fs::create_dir_all(&directory).unwrap();
        let fifo_path = directory.join("analyzer.pcm");
        crate::supervisor::MpdSupervisor::prepare_analyzer_fifo_path(&fifo_path).unwrap();
        let inode_before = fs::metadata(&fifo_path).unwrap().ino();

        let mut reader = open_fifo_reader(&fifo_path).unwrap();
        assert_eq!(fs::metadata(&fifo_path).unwrap().ino(), inode_before);
        let mut sample = [0_u8; 4];
        assert_eq!(reader.read(&mut sample).unwrap(), 0);

        let mut writer = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo_path)
            .unwrap();
        writer.write_all(&[1, 2, 3, 4]).unwrap();
        assert_eq!(reader.read(&mut sample).unwrap(), 4);
        assert_eq!(sample, [1, 2, 3, 4]);

        drop(writer);
        drop(reader);
        assert!(fifo_path.exists());
        assert_eq!(fs::metadata(&fifo_path).unwrap().ino(), inode_before);
        fs::remove_dir_all(directory).unwrap();
    }
}
