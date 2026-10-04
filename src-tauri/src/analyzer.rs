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
const WINDOW_LABEL: &str = "now-playing";
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

pub struct AnalyzerState(pub std::sync::Mutex<AudioAnalyzer>);

pub struct AudioAnalyzer {
    socket_path: String,
    fifo_path: PathBuf,
    active_session: Option<String>,
    controller_stop: Option<Arc<AtomicBool>>,
    reader_stop: Option<Arc<AtomicBool>>,
    reader: Option<JoinHandle<()>>,
    controller: Option<JoinHandle<()>>,
}

impl AudioAnalyzer {
    pub fn new(socket_path: String, fifo_path: PathBuf) -> Self {
        Self {
            socket_path,
            fifo_path,
            active_session: None,
            controller_stop: None,
            reader_stop: None,
            reader: None,
            controller: None,
        }
    }

    pub fn start_for_window(&mut self, session_id: String, app: AppHandle) -> Result<(), String> {
        if self.session_matches(&session_id) {
            return self.start(app);
        }
        if self.active_session.is_some() {
            self.stop(Some(&app))?;
        }
        self.start(app)?;
        self.active_session = Some(session_id);
        Ok(())
    }

    pub fn stop_for_window(&mut self, session_id: &str, app: &AppHandle) -> Result<(), String> {
        if !self.session_matches(session_id) {
            return Ok(());
        }
        self.active_session = None;
        self.stop(Some(app))
    }

    pub fn stop_for_window_close(&mut self, app: &AppHandle) -> Result<(), String> {
        self.active_session = None;
        self.stop(Some(app))
    }

    fn session_matches(&self, session_id: &str) -> bool {
        self.active_session.as_deref() == Some(session_id)
    }

    pub fn start(&mut self, app: AppHandle) -> Result<(), String> {
        if self.controller_stop.is_some() {
            println!("[Analyzer] Start requested while analyzer is already active.");
            return Ok(());
        }

        let fifo = open_fifo_reader(&self.fifo_path).map_err(|error| {
            analyzer_failed(&app, "fifo_open_failed", &error);
            error
        })?;
        println!("[Analyzer] Existing supervisor-owned FIFO opened for reading; inode preserved.");
        let controller_stop = Arc::new(AtomicBool::new(false));
        let reader_stop = Arc::new(AtomicBool::new(false));
        let reader_stop_thread = Arc::clone(&reader_stop);
        let reader_controller_stop = Arc::clone(&controller_stop);
        let reader_app = app.clone();
        let reader_socket_path = self.socket_path.clone();
        self.reader = Some(thread::spawn(move || {
            read_pcm(
                fifo,
                reader_stop_thread,
                reader_controller_stop,
                reader_app,
                reader_socket_path,
            );
        }));
        println!("[Analyzer] FIFO reader thread started.");

        let socket_path = self.socket_path.clone();
        if let Err(error) = reconcile_output(&socket_path, &app, &controller_stop) {
            analyzer_failed(&app, "initial_reconcile_failed", &error);
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
            monitor_player(socket_path, controller_thread_stop, controller_app);
        }));
        println!("[Analyzer] Player controller thread started.");
        self.controller_stop = Some(controller_stop);
        self.reader_stop = Some(reader_stop);
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.controller_stop.is_some()
    }

    pub fn has_window_session(&self) -> bool {
        self.active_session.is_some()
    }

    pub fn stop(&mut self, app: Option<&AppHandle>) -> Result<(), String> {
        let Some(controller_stop) = self.controller_stop.take() else {
            if let Some(app) = app {
                emit_status(app, false, Some("stopped"));
            }
            return Ok(());
        };

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
            emit_status(app, false, Some("stopped"));
        }
        if let Some(controller) = self.controller.take() {
            let _ = controller.join();
        }
        if let Some(reader_stop) = self.reader_stop.take() {
            reader_stop.store(true, Ordering::Release);
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        println!(
            "[Analyzer] Controller and reader cleanup completed; supervisor-owned FIFO preserved."
        );

        disable_result
    }
}

impl Drop for AudioAnalyzer {
    fn drop(&mut self) {
        if let Err(error) = self.stop(None) {
            eprintln!("[Analyzer] Falha no cleanup final: {}", error);
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
        "[Analyzer] Opened existing FIFO: path={}, inode={}; no unlink/recreate performed.",
        path.display(),
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
                    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
                        match window.emit(LEVEL_EVENT, frame) {
                            Ok(()) if !logged_first_frame => {
                                println!(
                                    "[Analyzer] First RMS/peak/spectrum frame emitted to window '{}'.",
                                    WINDOW_LABEL
                                );
                                logged_first_frame = true;
                            }
                            Ok(()) => {}
                            Err(error) => eprintln!(
                                "[Analyzer] Failed to emit level frame to window '{}': {}",
                                WINDOW_LABEL, error
                            ),
                        }
                    } else if !logged_first_frame {
                        eprintln!(
                            "[Analyzer] Window '{}' is absent; level frame was not emitted.",
                            WINDOW_LABEL
                        );
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                eprintln!("[Analyzer] Falha ao ler PCM do FIFO: {}", error);
                reader_stop.store(true, Ordering::Release);
                controller_stop.store(true, Ordering::Release);
                if let Err(disable_error) = set_output_enabled(
                    &socket_path,
                    crate::supervisor::MpdSupervisor::ANALYZER_OUTPUT_NAME,
                    false,
                ) {
                    eprintln!(
                        "[Analyzer] Falha ao desabilitar output após erro de leitura: {}",
                        disable_error
                    );
                }
                emit_status(&app, false, Some("reader_error"));
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

fn monitor_player(socket_path: String, stop: Arc<AtomicBool>, app: AppHandle) {
    let mut needs_reconcile = false;
    while !stop.load(Ordering::Acquire) {
        if needs_reconcile {
            match reconcile_output(&socket_path, &app, &stop) {
                Ok(()) => needs_reconcile = false,
                Err(_) => {
                    emit_status(&app, false, Some("mpd_unavailable"));
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            }
        }
        match wait_for_player_change(&socket_path) {
            Ok(true) => {
                if let Err(error) = set_output_enabled(
                    &socket_path,
                    crate::supervisor::MpdSupervisor::ANALYZER_OUTPUT_NAME,
                    false,
                ) {
                    eprintln!(
                        "[Analyzer] Falha ao desabilitar output durante troca de faixa: {}",
                        error
                    );
                    emit_status(&app, false, Some("output_disable_failed"));
                    continue;
                }
                if !stop.load(Ordering::Acquire) {
                    if let Err(error) = reconcile_output(&socket_path, &app, &stop) {
                        eprintln!("[Analyzer] Falha ao reavaliar a faixa atual: {}", error);
                        emit_status(&app, false, Some("reconcile_failed"));
                    }
                }
            }
            Ok(false) => {}
            Err(_) => {
                emit_status(&app, false, Some("mpd_unavailable"));
                needs_reconcile = true;
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn reconcile_output(socket_path: &str, app: &AppHandle, stop: &AtomicBool) -> Result<(), String> {
    let incompatibility = current_track_incompatibility(socket_path)?;
    if stop.load(Ordering::Acquire) || incompatibility.is_some() {
        set_output_enabled(
            socket_path,
            crate::supervisor::MpdSupervisor::ANALYZER_OUTPUT_NAME,
            false,
        )?;
        let reason = incompatibility.unwrap_or("stopped");
        println!(
            "[Analyzer] Analyzer inactive: current playback is incompatible (reason={}).",
            reason
        );
        emit_status(app, false, Some(reason));
        return Ok(());
    }
    println!("[Analyzer] Current playback classified as PCM-compatible.");
    set_output_enabled(
        socket_path,
        crate::supervisor::MpdSupervisor::ANALYZER_OUTPUT_NAME,
        true,
    )?;
    println!("[Analyzer] Output enabled; waiting for PCM bytes.");
    emit_status(app, true, None);
    Ok(())
}

fn emit_status(app: &AppHandle, available: bool, reason: Option<&str>) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let status = AnalyzerStatus {
            available,
            reason: reason.map(str::to_string),
        };
        if let Err(error) = window.emit(STATUS_EVENT, status) {
            eprintln!(
                "[Analyzer] Failed to emit status to window '{}': {}",
                WINDOW_LABEL, error
            );
        }
    } else {
        eprintln!(
            "[Analyzer] Window '{}' is absent; status available={} reason={:?} was not emitted.",
            WINDOW_LABEL, available, reason
        );
    }
}

fn analyzer_failed(app: &AppHandle, reason: &str, error: &str) {
    eprintln!("[Analyzer] Analyzer inactive: reason={}: {}", reason, error);
    emit_status(app, false, Some(reason));
}

fn current_track_incompatibility(socket_path: &str) -> Result<Option<&'static str>, String> {
    let status = send_command(socket_path, "status")?;
    let current_song = send_command(socket_path, "currentsong")?;
    Ok(dsd_or_dop_reason(&status, &current_song))
}

fn dsd_or_dop_reason(status: &[String], current_song: &[String]) -> Option<&'static str> {
    let format_is_dsd = status.iter().any(|line| {
        line.strip_prefix("audio: ")
            .is_some_and(|value| value.to_ascii_lowercase().contains("dsd"))
    });
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

fn wait_for_player_change(socket_path: &str) -> Result<bool, String> {
    let response =
        send_command_with_timeout(socket_path, "idle player", Duration::from_millis(500));
    match response {
        Ok(lines) => Ok(lines.iter().any(|line| line == "changed: player")),
        Err(error) if error.contains("temporariamente indisponível") => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn set_output_enabled(socket_path: &str, name: &str, enabled: bool) -> Result<(), String> {
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

    #[test]
    fn window_sessions_do_not_match_stale_cleanup_and_inactive_stop_is_idempotent() {
        let mut analyzer = AudioAnalyzer::new("/missing/mpd.socket".into(), "/missing/analyzer.pcm".into());
        analyzer.active_session = Some("new-window".into());
        assert!(!analyzer.session_matches("old-window"));
        assert!(analyzer.session_matches("new-window"));
        analyzer.stop(None).unwrap();
        analyzer.stop(None).unwrap();
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
