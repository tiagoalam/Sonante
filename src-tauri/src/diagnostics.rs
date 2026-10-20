//! Log-only error summaries. IPC errors retain their original contracts and messages.
use std::fmt::Display;

pub(crate) fn error_summary(error: impl Display) -> String {
    let text = error.to_string();
    let lower = text.to_lowercase();
    // Hints describe text matches, not proven causes. Only fixed labels and parsed
    // numeric codes reach the terminal. Raw errors may
    // contain authenticated URLs, filenames, metadata or application UI messages.
    let categories = [
        ("timeout", &["timeout", "timed out", "tempo limite"][..]),
        (
            "permission_denied",
            &["permission denied", "permissão", "permissao"][..],
        ),
        (
            "not_found",
            &[
                "not found",
                "no such file",
                "não encontrado",
                "não encontrada",
            ][..],
        ),
        (
            "connection_refused",
            &["connection refused", "conexão recusada"][..],
        ),
        ("connection_reset", &["connection reset", "broken pipe"][..]),
        (
            "unexpected_eof",
            &["eof", "unexpected end", "resposta incompleta"][..],
        ),
        ("busy", &["busy", "ocupado", "em uso"][..]),
        (
            "invalid_state",
            &["invalid", "inválid", "incompat", "inconsisten"][..],
        ),
        (
            "conflict",
            &["conflict", "conflito", "identidade", "identity"][..],
        ),
        ("lock_poisoned", &["poison", "corrompido"][..]),
        (
            "authentication",
            &["unauthorized", "forbidden", "autenticação"][..],
        ),
        ("task_panicked", &["panic"][..]),
        ("not_seekable", &["not seekable"][..]),
        (
            "uncertain",
            &["uncertain", "unconfirmed", "incerto", "não confirmado"][..],
        ),
        ("unsupported", &["unsupported", "não suportad"][..]),
        ("unavailable", &["unavailable", "indisponível"][..]),
    ];
    let kind = categories
        .iter()
        .find(|(_, terms)| terms.iter().any(|term| lower.contains(term)))
        .map(|(kind, _)| *kind)
        .unwrap_or("unclassified");
    let mut summary = format!("error_hint={kind}");
    if let Some(code) = numeric_code(&text, "(os error ", ")") {
        summary.push_str(&format!(" os_code={code}"));
    }
    if let Some(code) = numeric_code(&text, "ACK [", "@") {
        summary.push_str(&format!(" mpd_ack_code={code}"));
    }
    summary
}

fn numeric_code(text: &str, prefix: &str, suffix: &str) -> Option<u32> {
    let value = text.split_once(prefix)?.1.split_once(suffix)?.0;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

// Narrow, opt-in DEV collection for the Now Playing handoff. No extra worker,
// MPD query or raw error/host/session/lease string is retained or printed.
fn now_playing_enabled() -> bool {
    cfg!(debug_assertions) && std::env::var("SONANTE_NOW_PLAYING_DIAGNOSTICS").as_deref() == Ok("1")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MpdErrorShape {
    class: &'static str,
    output_hint: &'static str,
}

fn mpd_error_shape(error: &str) -> MpdErrorShape {
    // MPD nests output errors with "; ". Classify allowlisted clauses only;
    // a filename/URL containing a plugin name is not output attribution.
    let clauses: Vec<_> = error.split("; ").collect();
    let class = if clauses
        .iter()
        .any(|s| s.starts_with("Failed to write to FIFO "))
    {
        "fifo_write"
    } else if clauses
        .iter()
        .any(|s| s.starts_with("Could not open FIFO "))
    {
        "fifo_open"
    } else if clauses
        .iter()
        .any(|s| s.starts_with("Failed to open ALSA device "))
    {
        "alsa_open"
    } else if error == "All audio outputs are disabled" {
        "all_outputs_disabled"
    } else if error.starts_with("Failed to decode ") {
        "decode"
    } else {
        "unclassified"
    };
    let output_hint = if clauses
        .iter()
        .any(|s| s.starts_with("Failed to open \"Sonante Analyzer\" (fifo)"))
    {
        "analyzer_fifo_named"
    } else if clauses.iter().any(|s| {
        s.starts_with("Failed to open \"Sonante Output\" (alsa)")
            || s.starts_with("Failed to open \"Sonante Shared\" (alsa)")
    }) {
        "main_alsa_named"
    } else if class.starts_with("fifo_") {
        "fifo_unspecified"
    } else if class == "alsa_open" {
        "alsa_unspecified"
    } else {
        "unattributed"
    };
    MpdErrorShape { class, output_hint }
}

fn now_playing_log_host(host: Option<&str>) -> &'static str {
    match host {
        Some("main") => "main",
        Some("now-playing") => "now-playing",
        None => "none",
        _ => "other",
    }
}

struct NowPlayingObservation {
    step: u64,
    phase: &'static str,
    host: &'static str,
    generation: u64,
    lease_present: bool,
    phase_at: Instant,
    auxiliary: &'static str,
    last_error: Option<(MpdErrorShape, String, u64)>,
    next_seek: u64,
    seek: Option<SeekObservation>,
    latest_status: Option<(String, Instant)>,
}

struct SeekObservation {
    id: u64,
    origin: &'static str,
    finished: Option<Instant>,
    samples: u8,
    last_sample: Option<Instant>,
}

impl SeekObservation {
    fn recent(&self) -> bool {
        self.finished
            .is_none_or(|time| time.elapsed() <= Duration::from_secs(2))
    }
    fn sample_due(&self) -> bool {
        self.finished.is_some()
            && self.recent()
            && self.samples < 3
            && self
                .last_sample
                .is_none_or(|time| time.elapsed() >= Duration::from_millis(250))
    }
}

fn seek_number(value: Option<&str>) -> String {
    match value {
        None => "missing".into(),
        Some(value) => match value.parse::<f64>() {
            Ok(number) if number.is_finite() && number >= 0.0 => format!("{number:.3}"),
            _ => "invalid".into(),
        },
    }
}

fn seek_status_fields(lines: &[String]) -> String {
    let field = |prefix: &str| {
        lines
            .iter()
            .filter_map(|line| line.strip_prefix(prefix))
            .last()
    };
    let state = match field("state: ") {
        Some("play") => "play",
        Some("pause") => "pause",
        Some("stop") => "stop",
        None => "missing",
        _ => "invalid",
    };
    let song = match field("song: ") {
        None => "missing".into(),
        Some(value) => value
            .parse::<usize>()
            .map(|index| index.to_string())
            .unwrap_or_else(|_| "invalid".into()),
    };
    let error = field("error: ").map(mpd_error_shape);
    format!(
        "state={} elapsed_s={} duration_s={} song={} status_error={}",
        state,
        seek_number(field("elapsed: ")),
        seek_number(field("duration: ")),
        song,
        error.map(|shape| shape.class).unwrap_or("none")
    )
}

fn seek_command_fields(command: &str, result: &Result<Vec<String>, String>) -> String {
    let kind = match command {
        "status" => "status",
        "pause 1" => "pause_1",
        "pause 0" => "pause_0",
        value if value.starts_with("seekcur ") => "seekcur",
        _ => "other",
    };
    let outcome = match result {
        Ok(lines) if kind == "status" => format!("terminal=OK {}", seek_status_fields(lines)),
        Ok(_) => "terminal=OK".into(),
        Err(error) => format!(
            "terminal={} {}",
            if numeric_code(error, "ACK [", "@").is_some() {
                "ACK"
            } else {
                "error"
            },
            error_summary(error)
        ),
    };
    let target = if kind == "seekcur" {
        format!(
            " wire_target_s={}",
            seek_number(command.strip_prefix("seekcur "))
        )
    } else {
        String::new()
    };
    format!("command={kind}{target} {outcome}")
}

impl NowPlayingObservation {
    fn new() -> Self {
        Self {
            step: 0,
            phase: "none",
            host: "none",
            generation: 0,
            lease_present: false,
            phase_at: Instant::now(),
            auxiliary: "none",
            last_error: None,
            next_seek: 0,
            seek: None,
            latest_status: None,
        }
    }
    fn context(&self) -> String {
        format!("step={} phase={} host={} generation={} lease_present={} since_phase_ms={} auxiliary={}",
            self.step, self.phase, self.host, self.generation, self.lease_present,
            self.phase_at.elapsed().as_millis(), self.auxiliary)
    }
    fn error_changed(&mut self, error: Option<&str>) -> Option<String> {
        let Some(error) = error else {
            self.last_error = None;
            return None;
        };
        let shape = mpd_error_shape(error);
        let summary = error_summary(error);
        let key = (shape, summary.clone(), self.step);
        if self.last_error.as_ref() == Some(&key) {
            return None;
        }
        self.last_error = Some(key);
        Some(format!(
            "mpd_error class={} output_hint={} {} {}",
            shape.class,
            shape.output_hint,
            summary,
            self.context()
        ))
    }
}

static NOW_PLAYING_OBSERVATION: OnceLock<Mutex<NowPlayingObservation>> = OnceLock::new();
fn now_playing_observation() -> &'static Mutex<NowPlayingObservation> {
    NOW_PLAYING_OBSERVATION.get_or_init(|| Mutex::new(NowPlayingObservation::new()))
}

pub(crate) fn now_playing_phase(
    phase: &'static str,
    host: Option<&str>,
    generation: u64,
    lease_present: bool,
) {
    if !now_playing_enabled() {
        return;
    }
    if let Ok(mut state) = now_playing_observation().lock() {
        state.step += 1;
        state.phase = phase;
        state.host = now_playing_log_host(host);
        state.generation = generation;
        state.lease_present = lease_present;
        state.phase_at = Instant::now();
        eprintln!("[Sonante] dev.now-playing {}", state.context());
    }
}

pub(crate) fn now_playing_auxiliary(action: &'static str) {
    if !now_playing_enabled() {
        return;
    }
    if let Ok(mut state) = now_playing_observation().lock() {
        state.step += 1;
        state.auxiliary = action;
        eprintln!("[Sonante] dev.now-playing {}", state.context());
    }
}

pub(crate) fn now_playing_mpd_error(error: Option<&str>) {
    if !now_playing_enabled() {
        return;
    }
    if let Ok(mut state) = now_playing_observation().lock() {
        if let Some(line) = state.error_changed(error) {
            eprintln!("[Sonante] dev.now-playing {}", line);
        }
    }
}

// Called after AudioState is locked; one IPC seek owns this observation until
// completion. Subsequent samples use existing telemetry, never a new query/timer.
pub(crate) fn now_playing_seek_begin(origin: &str, seconds: f64, shared: bool) -> Option<u64> {
    if !now_playing_enabled() {
        return None;
    }
    let mut state = now_playing_observation().lock().ok()?;
    state.next_seek += 1;
    let id = state.next_seek;
    let origin = now_playing_log_host(Some(origin));
    state.seek = Some(SeekObservation {
        id,
        origin,
        finished: None,
        samples: 0,
        last_sample: None,
    });
    let target = seek_number(Some(&seconds.to_string()));
    eprintln!("[Sonante] dev.now-playing seek id={id} stage=ipc_request origin={origin} target_s={target} shared={shared} {}", state.context());
    if let Some((fields, time)) = &state.latest_status {
        eprintln!(
            "[Sonante] dev.now-playing seek id={id} stage=pre_telemetry snapshot_age_ms={} {}",
            time.elapsed().as_millis(),
            fields
        );
    } else {
        eprintln!("[Sonante] dev.now-playing seek id={id} stage=pre_telemetry unavailable=true");
    }
    Some(id)
}

pub(crate) fn now_playing_seek_command(command: &str, result: &Result<Vec<String>, String>) {
    if !now_playing_enabled() {
        return;
    }
    if let Ok(state) = now_playing_observation().lock() {
        let Some(seek) = state.seek.as_ref().filter(|seek| seek.finished.is_none()) else {
            return;
        };
        eprintln!(
            "[Sonante] dev.now-playing seek id={} stage=command origin={} {}",
            seek.id,
            seek.origin,
            seek_command_fields(command, result)
        );
    }
}

pub(crate) fn now_playing_seek_finish(id: Option<u64>, result: &Result<(), String>) {
    if !now_playing_enabled() {
        return;
    }
    if let Ok(mut state) = now_playing_observation().lock() {
        let Some(seek) = state.seek.as_mut().filter(|seek| Some(seek.id) == id) else {
            return;
        };
        seek.finished = Some(Instant::now());
        let outcome = match result {
            Ok(()) => "result=ok".into(),
            Err(error) => format!("result=error {}", error_summary(error)),
        };
        eprintln!(
            "[Sonante] dev.now-playing seek id={} stage=finish origin={} {}",
            seek.id, seek.origin, outcome
        );
    }
}

pub(crate) fn now_playing_seek_telemetry(lines: &[String]) {
    if !now_playing_enabled() {
        return;
    }
    if let Ok(mut state) = now_playing_observation().lock() {
        let fields = seek_status_fields(lines);
        state.latest_status = Some((fields.clone(), Instant::now()));
        let Some(seek) = state.seek.as_mut().filter(|seek| seek.sample_due()) else {
            return;
        };
        seek.samples += 1;
        seek.last_sample = Some(Instant::now());
        eprintln!(
            "[Sonante] dev.now-playing seek id={} stage=telemetry origin={} sample={} {}",
            seek.id, seek.origin, seek.samples, fields
        );
    }
}

pub(crate) fn now_playing_seek_controller(host: &str, generation: u64) {
    if !now_playing_enabled() {
        return;
    }
    if let Ok(state) = now_playing_observation().lock() {
        let Some(seek) = state.seek.as_ref().filter(|seek| seek.recent()) else {
            return;
        };
        // Temporal correlation only; the player event may have another cause.
        eprintln!("[Sonante] dev.now-playing seek id={} stage=controller_player_event controller_host={} controller_generation={}", seek.id, now_playing_log_host(Some(host)), generation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_playing_seek_command_ack_and_transport_errors_never_echo_private_text() {
        let ack = Err("ACK [50@0] {seekcur} Not seekable /private/SECRET https://user:password@private.example?X-Plex-Token=SECRET".into());
        assert_eq!(
            seek_command_fields("seekcur 62.5", &ack),
            "command=seekcur wire_target_s=62.500 terminal=ACK error_hint=not_seekable mpd_ack_code=50"
        );
        assert_eq!(
            seek_command_fields("seekcur 62.5", &Ok(vec![])),
            "command=seekcur wire_target_s=62.500 terminal=OK"
        );
        assert_eq!(
            seek_command_fields("unknown /private/SECRET", &Err("timeout SECRET".into())),
            "command=other terminal=error error_hint=timeout"
        );
        assert_eq!(
            seek_command_fields("pause 1", &Ok(vec![])),
            "command=pause_1 terminal=OK"
        );
        assert_eq!(
            seek_command_fields("pause 0", &Ok(vec![])),
            "command=pause_0 terminal=OK"
        );
    }

    #[test]
    fn now_playing_seek_status_keeps_only_state_progress_and_numeric_context() {
        let lines = vec![
            "state: play".into(),
            "elapsed: 62.5".into(),
            "duration: 185".into(),
            "song: 1".into(),
            "file: /private/track?X-Plex-Token=SECRET".into(),
            "Title: Private title".into(),
        ];
        assert_eq!(
            seek_status_fields(&lines),
            "state=play elapsed_s=62.500 duration_s=185.000 song=1 status_error=none"
        );
        let invalid = vec![
            "state: SECRET".into(),
            "elapsed: NaN".into(),
            "duration: /private/SECRET".into(),
            "song: SECRET".into(),
            "error: Failed to decode /private/SECRET".into(),
        ];
        let safe = seek_status_fields(&invalid);
        assert_eq!(
            safe,
            "state=invalid elapsed_s=invalid duration_s=invalid song=invalid status_error=decode"
        );
        for value in ["SECRET", "/private", "Title", "X-Plex-Token", "NaN"] {
            assert!(!safe.contains(value));
        }
        assert_eq!(seek_number(Some("-1")), "invalid");
        assert_eq!(seek_number(None), "missing");
    }

    #[test]
    fn now_playing_seek_passive_samples_are_bounded_and_expire_without_a_worker() {
        let mut seek = SeekObservation {
            id: 1,
            origin: "main",
            finished: None,
            samples: 0,
            last_sample: None,
        };
        assert!(!seek.sample_due());
        seek.finished = Some(Instant::now());
        assert!(seek.sample_due());
        seek.last_sample = Some(Instant::now());
        assert!(!seek.sample_due());
        seek.last_sample = Some(Instant::now() - Duration::from_millis(251));
        assert!(seek.sample_due());
        seek.samples = 3;
        assert!(!seek.sample_due());
        seek.samples = 0;
        seek.finished = Some(Instant::now() - Duration::from_secs(3));
        assert!(!seek.recent());
        assert!(!seek.sample_due());
    }

    #[test]
    fn now_playing_error_classification_keeps_output_attribution_explicit() {
        for (raw, class, output_hint) in [
            ("Failed to open \"Sonante Analyzer\" (fifo); Could not open FIFO \"/private/fifo\" for writing: Permission denied", "fifo_open", "analyzer_fifo_named"),
            ("Failed to write to FIFO /private/fifo: Broken pipe", "fifo_write", "fifo_unspecified"),
            ("Failed to open \"Sonante Output\" (alsa); Failed to open ALSA device \"hw:private\": Invalid argument", "alsa_open", "main_alsa_named"),
            ("Failed to open \"Sonante Shared\" (alsa); Failed to open ALSA device \"default\": Device or resource busy", "alsa_open", "main_alsa_named"),
            ("Failed to open ALSA device \"hw:private\": Device or resource busy", "alsa_open", "alsa_unspecified"),
            ("All audio outputs are disabled", "all_outputs_disabled", "unattributed"),
            ("Failed to decode /private/Sonante Analyzer fifo.flac", "decode", "unattributed"),
            ("Unexpected /private/Sonante Analyzer fifo", "unclassified", "unattributed"),
        ] {
            assert_eq!(mpd_error_shape(raw), MpdErrorShape { class, output_hint });
        }
    }

    #[test]
    fn now_playing_error_collection_is_bounded_and_deduplicates_status_polling() {
        let mut state = NowPlayingObservation::new();
        state.host = "now-playing";
        state.generation = 4;
        state.phase = "stop_requested";
        state.lease_present = true;
        state.auxiliary = "disable_accepted";
        let raw = "Failed to write to FIFO /private/fifo: Broken pipe";
        let first = state.error_changed(Some(raw)).unwrap();
        assert!(first.contains("class=fifo_write output_hint=fifo_unspecified"));
        assert!(first.contains("phase=stop_requested host=now-playing generation=4"));
        assert!(first.contains("auxiliary=disable_accepted"));
        assert!(state.error_changed(Some(raw)).is_none());
        // New main reservation/capture is a new correlation, not an attribution
        // of the persistent error to the new host or the primary audio output.
        state.host = "main";
        state.generation = 5;
        state.phase = "reserved";
        state.step += 1;
        assert!(state
            .error_changed(Some(raw))
            .unwrap()
            .contains("host=main generation=5"));
        assert!(state.error_changed(None).is_none());
        assert!(state.error_changed(Some(raw)).is_some());
    }

    #[test]
    fn now_playing_observations_never_retain_or_print_private_data() {
        let raw = "Failed to write to FIFO /home/Private/Album https://user:password@private.invalid/file?X-Plex-Token=SECRET: Broken pipe (os error 32)";
        let mut state = NowPlayingObservation::new();
        state.host = now_playing_log_host(Some("/private/session-SECRET"));
        let line = state.error_changed(Some(raw)).unwrap();
        let retained = format!("{:?}", state.last_error);
        for private in [
            "/home/Private",
            "Album",
            "https://",
            "password",
            "private.invalid",
            "X-Plex-Token",
            "SECRET",
            "session-",
        ] {
            assert!(!line.contains(private));
            assert!(!retained.contains(private));
        }
        assert!(line.contains("error_hint=connection_reset os_code=32"));
        assert!(line.contains("host=other"));
        assert_eq!(now_playing_log_host(Some("main")), "main");
        assert_eq!(now_playing_log_host(Some("now-playing")), "now-playing");
    }

    #[test]
    fn errors_keep_safe_context_without_private_data() {
        let error = "Falha: ACK [5@0] {add} https://user:password@server/private/song?X-Plex-Token=secret /home/private/library (os error 13)";
        assert_eq!(
            error_summary(error),
            "error_hint=unclassified os_code=13 mpd_ack_code=5"
        );
        assert_eq!(
            error_summary("Permission denied (os error 13)"),
            "error_hint=permission_denied os_code=13"
        );
        assert_eq!(
            error_summary("Falha: timeout /mnt/private"),
            "error_hint=timeout"
        );
    }

    #[test]
    fn raw_messages_and_malformed_codes_are_never_echoed() {
        for error in [
            "unknown secret",
            "ACK [secret@0]",
            "(os error token)",
            "(os error 99999999999999999999999)",
            "Falha na operação",
        ] {
            assert_eq!(error_summary(error), "error_hint=unclassified");
        }
    }

    #[test]
    fn portuguese_application_errors_use_english_log_hints_only() {
        assert_eq!(
            error_summary("Estado indisponível /private"),
            "error_hint=unavailable"
        );
        assert_eq!(
            error_summary("Falha de autenticação token=secret"),
            "error_hint=authentication"
        );
        assert_eq!(
            error_summary("Cache da fila inválido"),
            "error_hint=invalid_state"
        );
    }

    #[test]
    fn internal_log_templates_are_english_and_prefixed() {
        let sources = [
            include_str!("lib.rs"),
            include_str!("audio.rs"),
            include_str!("supervisor.rs"),
            include_str!("mpris.rs"),
            include_str!("analyzer.rs"),
            include_str!("local_watcher.rs"),
            include_str!("artwork_progress.rs"),
            include_str!("local_media.rs"),
        ];
        for source in sources {
            // All existing terminal sites use println!/eprintln! with a literal template.
            for call in source.split("println!(").skip(1) {
                let template = call.split_once('"').unwrap().1.split_once('"').unwrap().0;
                assert!(template.is_ascii(), "Non-English log template: {template}");
                assert!(
                    template.starts_with('[') || template == "{}",
                    "Missing log prefix: {template}"
                );
                for portuguese in [
                    "Falha",
                    "indispon",
                    "Iniciando",
                    "Progresso",
                    "Usando",
                    "encerr",
                    "dispositivo=",
                ] {
                    assert!(
                        !template.contains(portuguese),
                        "Portuguese log template: {template}"
                    );
                }
            }
        }
    }
}

// Opt-in bounded observations. The reporting thread does not share Tokio's
// blocking pool, inspect the filesystem, or hold this mutex during application I/O.
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub(crate) fn dev_enabled() -> bool {
    cfg!(debug_assertions)
        && std::env::var("SONANTE_EXPLORE_DIAGNOSTICS").as_deref() == Ok("1")
}

#[derive(serde::Serialize)]
struct Active {
    operation: &'static str,
    ipc: Option<u64>,
    phase: &'static str,
    #[serde(skip)]
    started: Instant,
    #[serde(skip)]
    phase_started: Instant,
    #[serde(skip)]
    phase_times_ms: BTreeMap<&'static str, u128>,
}
#[derive(Default, serde::Serialize)]
struct Counts {
    started: u64,
    completed: u64,
    failed: u64,
    slow: u64,
}
#[derive(serde::Serialize)]
struct Completed {
    id: u64,
    ipc: Option<u64>,
    operation: &'static str,
    phase: &'static str,
    elapsed_ms: u128,
    failed: bool,
    phase_times_ms: BTreeMap<&'static str, u128>,
}
#[derive(Default)]
struct Observations {
    next: u64,
    active: BTreeMap<u64, Active>,
    counts: BTreeMap<&'static str, Counts>,
    completed: VecDeque<Completed>,
    omitted: u64,
    main_pending: Option<Instant>,
    main_completed: u64,
    main_schedule_failed: u64,
}
impl Observations {
    fn start(&mut self, operation: &'static str, ipc: Option<u64>, now: Instant) -> Option<u64> {
        if self.active.len() >= 128 || (!self.counts.contains_key(operation) && self.counts.len() >= 64) {
            self.omitted += 1;
            return None;
        }
        self.next += 1;
        let id = self.next;
        self.counts.entry(operation).or_default().started += 1;
        self.active.insert(id, Active { operation, ipc, phase: "entered", started: now, phase_started: now, phase_times_ms: BTreeMap::new() });
        Some(id)
    }
    fn phase(&mut self, id: u64, phase: &'static str, now: Instant) {
        if let Some(active) = self.active.get_mut(&id) {
            if active.phase != phase {
                *active.phase_times_ms.entry(active.phase).or_default() += now.saturating_duration_since(active.phase_started).as_millis();
                active.phase = phase;
                active.phase_started = now;
            }
        }
    }
    fn finish(&mut self, id: u64, failed: bool, now: Instant) {
        if let Some(mut active) = self.active.remove(&id) {
            *active.phase_times_ms.entry(active.phase).or_default() += now.saturating_duration_since(active.phase_started).as_millis();
            let elapsed_ms = now.saturating_duration_since(active.started).as_millis();
            let counts = self.counts.entry(active.operation).or_default();
            counts.completed += 1;
            counts.failed += u64::from(failed);
            counts.slow += u64::from(elapsed_ms >= 1000);
            // Healthy anonymous suboperations are counters only. IPC completions
            // keep a bounded correlation history, including fast requests.
            if active.ipc.is_some() || failed || elapsed_ms >= 1000 {
                if self.completed.len() == 16 { self.completed.pop_front(); }
                self.completed.push_back(Completed { id, ipc: active.ipc, operation: active.operation,
                    phase: active.phase, elapsed_ms, failed, phase_times_ms: active.phase_times_ms });
            }
        }
    }
    fn snapshot(&mut self, now: Instant) -> serde_json::Value {
        let pending: Vec<_> = self.active.iter().take(16).map(|(id, active)| serde_json::json!({
            "id": id, "ipc": active.ipc, "operation": active.operation, "phase": active.phase,
            "elapsed_ms": now.saturating_duration_since(active.started).as_millis(),
            "phase_ms": now.saturating_duration_since(active.phase_started).as_millis(), "phase_times_ms": active.phase_times_ms
        })).collect();
        let mut active_phases = BTreeMap::<&str, usize>::new();
        for active in self.active.values() { *active_phases.entry(active.phase).or_default() += 1; }
        let value = serde_json::json!({
            "active_phases": active_phases,
            "counts": self.counts, "pending": pending, "active_count": self.active.len(),
            "completed": self.completed, "omitted": self.omitted,
            "main_loop": { "pending_ms": self.main_pending.map(|start| now.saturating_duration_since(start).as_millis()),
                "completed": self.main_completed, "schedule_failed": self.main_schedule_failed }
        });
        self.counts.clear(); self.completed.clear(); self.omitted = 0;
        value
    }
}
static OBSERVATIONS: OnceLock<Mutex<Observations>> = OnceLock::new();
static MAIN_APP: OnceLock<tauri::AppHandle> = OnceLock::new();
fn observations() -> &'static Mutex<Observations> {
    OBSERVATIONS.get_or_init(|| {
        // A single DEV thread, one terminal summary per 10 seconds. Never enqueue
        // another main-loop probe while the previous callback has not executed.
        let _ = std::thread::Builder::new().name("sonante-dev-observer".into()).spawn(|| loop {
            std::thread::sleep(Duration::from_secs(10));
            let Some(state) = OBSERVATIONS.get() else { continue };
            if let Some(app) = MAIN_APP.get() {
                let schedule = if let Ok(mut state) = state.lock() {
                    if state.main_pending.is_none() { state.main_pending = Some(Instant::now()); true } else { false }
                } else { false };
                if schedule && app.run_on_main_thread(|| {
                    if let Some(state) = OBSERVATIONS.get() {
                        if let Ok(mut state) = state.lock() { state.main_pending = None; state.main_completed += 1; }
                    }
                }).is_err() {
                    if let Ok(mut state) = state.lock() { state.main_pending = None; state.main_schedule_failed += 1; }
                }
            }
            let snapshot = state.lock().ok().map(|mut state| state.snapshot(Instant::now()));
            if let Some(snapshot) = snapshot {
                let timestamp_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default().as_millis();
                eprintln!("[Sonante] dev.summary timestamp_ms={timestamp_ms} {snapshot}");
            }
        });
        Mutex::new(Observations::default())
    })
}
pub(crate) fn install_main_thread_probe(app: tauri::AppHandle) {
    if dev_enabled() { let _ = MAIN_APP.set(app); let _ = observations(); }
}

pub(crate) struct DevSpan { id: Option<u64>, failed: bool }
impl DevSpan {
    pub(crate) fn new(operation: &'static str) -> Self { Self::ipc(operation, None) }
    pub(crate) fn ipc(operation: &'static str, ipc: Option<u64>) -> Self {
        let id = if dev_enabled() { observations().lock().ok().and_then(|mut state|
            state.start(operation, ipc.filter(|id| *id <= 9_007_199_254_740_991), Instant::now())) } else { None };
        Self { id, failed: false }
    }
    pub(crate) fn phase(&self, phase: &'static str) {
        if let Some(id) = self.id { if let Ok(mut state) = observations().lock() { state.phase(id, phase, Instant::now()); } }
    }
    pub(crate) fn result<T, E>(&mut self, result: &Result<T, E>) { self.failed = result.is_err(); }
}
impl Drop for DevSpan {
    fn drop(&mut self) {
        if let Some(id) = self.id { if let Ok(mut state) = observations().lock() { state.finish(id, self.failed, Instant::now()); } }
    }
}
#[cfg(test)]
mod observation_tests {
    use super::*;
    #[test]
    fn healthy_frequency_is_aggregated_and_history_is_bounded() {
        let mut state = Observations::default(); let now = Instant::now();
        for _ in 0..10000 { let id = state.start("health", None, now).unwrap(); state.finish(id, false, now); }
        assert!(state.completed.is_empty());
        let snapshot = state.snapshot(now); assert_eq!(snapshot["counts"]["health"]["completed"], 10000);
        assert!(state.counts.is_empty());
        for ipc in 0..1000 { let id = state.start("ipc", Some(ipc), now).unwrap(); state.finish(id, true, now); }
        assert_eq!(state.completed.len(), 16); assert_eq!(state.completed.back().unwrap().ipc, Some(999));
    }
    #[test]
    fn pending_phases_correlate_without_releasing_physical_work() {
        let mut state = Observations::default(); let now = Instant::now();
        let id = state.start("cover", Some(123), now).unwrap();
        state.phase(id, "wait.config", now + Duration::from_secs(1));
        let snapshot = state.snapshot(now + Duration::from_secs(4));
        assert_eq!(snapshot["pending"][0]["ipc"], 123); assert_eq!(snapshot["pending"][0]["phase"], "wait.config");
        assert_eq!(snapshot["pending"][0]["phase_ms"], 3000); assert_eq!(state.active.len(), 1);
        state.phase(id, "executor.queued", now + Duration::from_secs(5));
        state.phase(id, "filesystem", now + Duration::from_secs(7));
        state.finish(id, true, now + Duration::from_secs(9));
        assert!(state.active.is_empty()); assert_eq!(state.completed.back().unwrap().elapsed_ms, 9000);
        assert_eq!(state.completed.back().unwrap().phase, "filesystem");
        assert_eq!(state.completed.back().unwrap().phase_times_ms["wait.config"], 4000);
        assert_eq!(state.completed.back().unwrap().phase_times_ms["executor.queued"], 2000);
        assert_eq!(state.completed.back().unwrap().phase_times_ms["filesystem"], 2000);
    }
    #[test]
    fn pending_and_label_volume_cannot_grow_without_bound() {
        let mut state = Observations::default(); let now = Instant::now();
        for _ in 0..128 { assert!(state.start("cover", None, now).is_some()); }
        for _ in 0..1000 { assert!(state.start("cover", None, now).is_none()); }
        let snapshot = state.snapshot(now); assert_eq!(snapshot["active_count"], 128);
        assert_eq!(snapshot["pending"].as_array().unwrap().len(), 16); assert_eq!(snapshot["omitted"], 1000);
    }
}
