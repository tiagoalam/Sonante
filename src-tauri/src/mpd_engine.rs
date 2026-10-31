//! Pinned package-owned runtimes, with explicit debug-only development overrides.
//! This identifies a build; it does not certify a live audio route.
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Child;

const PRIVATE_SHA256: &str = "46ea7b1eaa1969f377698155b041589e9475998a3859a4c96f7937565fda7841";
const INSTALLED_DIRECT_SHA256: &str = "7bf2a6c96fad672ee6a056335c1b8054a71107975c637b76f549751b43d0d4b1";
const INSTALLED_EQ_SHA256: &str = "6b9cb9820c1704175af312377fae6f20ad80486f69ee17cd190bc6390bc1d5ff";
const MAX_ENGINE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum EngineEvidence {
    Identified {
        evidence_id: &'static str,
        scope: &'static str,
        pcm_eof_flush: bool,
    },
}

pub(crate) fn sample_evidence<T: PartialEq>(
    active: bool,
    before: Option<&T>,
    mut current: impl FnMut() -> Option<T>,
    read: impl FnOnce(&T) -> Option<EngineEvidence>,
) -> Option<EngineEvidence> {
    if !active {
        return None;
    }
    let before = before?;
    if current().as_ref() != Some(before) {
        return None;
    }
    let evidence = read(before);
    if current().as_ref() != Some(before) {
        return None;
    }
    evidence
}

pub(crate) struct MpdEngine {
    program: PathBuf,
    private_sha256: Option<String>,
}

impl MpdEngine {
    pub(crate) fn resolve() -> Result<Self, String> {
        if !cfg!(debug_assertions) {
            return Ok(Self { program: crate::installed_runtimes::release(crate::installed_runtimes::Kind::Direct,"SONANTE_DEV_MPD_PATH")?, private_sha256: Some(INSTALLED_DIRECT_SHA256.into()) });
        }
        Self::resolve_override(
            std::env::var_os("SONANTE_DEV_MPD_PATH").as_deref(),
            cfg!(all(debug_assertions, target_os = "linux")),
            PRIVATE_SHA256,
        )
    }

    /// EQ-only variant: the proven Direct binary and its evidence remain unchanged.
    pub(crate) fn resolve_eq() -> Result<Self,String> {
        if !cfg!(debug_assertions) {
            return Ok(Self { program: crate::installed_runtimes::release(crate::installed_runtimes::Kind::Eq,"SONANTE_EQ_MPD_PATH")?, private_sha256: Some(INSTALLED_EQ_SHA256.into()) });
        }
        let value=std::env::var_os("SONANTE_EQ_MPD_PATH").ok_or("EQ_MPD_RUNTIME_MISSING")?;
        Self::resolve_override(Some(value.as_os_str()),cfg!(all(debug_assertions,target_os="linux")),"6b9cb9820c1704175af312377fae6f20ad80486f69ee17cd190bc6390bc1d5ff")
    }

    fn resolve_override(
        value: Option<&OsStr>,
        allowed: bool,
        expected: &str,
    ) -> Result<Self, String> {
        let Some(value) = value else {
            return Ok(Self {
                program: PathBuf::from("mpd"),
                private_sha256: None,
            });
        };
        if !allowed {
            return Err(
                "Motor MPD experimental disponível apenas no desenvolvimento Linux.".into(),
            );
        }
        let path = Path::new(value);
        if !path.is_absolute() {
            return Err("O motor MPD experimental exige um caminho absoluto.".into());
        }
        let program = path
            .canonicalize()
            .map_err(|_| "Motor MPD experimental indisponível.".to_string())?;
        let metadata = program.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err("O motor MPD experimental precisa ser um arquivo executável.".into());
        }
        let actual = Self::validated_override_hash(hash_file(&program)?, expected)?;
        Ok(Self {
            program,
            private_sha256: Some(actual),
        })
    }

    fn validated_override_hash(actual: String, expected: &str) -> Result<String, String> {
        // Explicit Linux debug override only. No path fallback or release change.
        let installed_direct = cfg!(all(debug_assertions, target_os = "linux"))
            && expected == PRIVATE_SHA256 && actual == INSTALLED_DIRECT_SHA256;
        if actual != expected && !installed_direct {
            return Err(
                "O motor MPD experimental não corresponde à compilação privada validada.".into(),
            );
        }
        Ok(actual)
    }

    pub(crate) fn program(&self) -> &Path {
        &self.program
    }

    // Cached only after the owned child's executable has been verified.
    // This is build provenance, never a current-stream bit comparison.
    pub(crate) fn evidence(&self, is_shared: bool) -> Option<EngineEvidence> {
        (self.private_sha256.as_deref() == Some(PRIVATE_SHA256) || self.private_sha256.as_deref() == Some(INSTALLED_DIRECT_SHA256)).then_some(
            EngineEvidence::Identified {
                evidence_id: if self.private_sha256.as_deref() == Some(INSTALLED_DIRECT_SHA256) { "mpd-strict-ubuntu2204-remap-20261018" } else { "mpd-strict-route-20261017" },
                scope: "pcm_dsd_route_strict",
                pcm_eof_flush: !is_shared,
            },
        )
    }

    pub(crate) fn configure_output(
        &self,
        section: String,
        is_shared: bool,
    ) -> Result<String, String> {
        if self.private_sha256.is_none() || is_shared {
            return Ok(section);
        }
        let prefix = section
            .strip_suffix("\n}")
            .ok_or_else(|| "Seção ALSA inválida para o motor experimental.".to_string())?;
        Ok(format!("{prefix}\n    pcm_eof_flush \"yes\"\n    sonante_direct_strict \"yes\"\n}}"))
    }

    pub(crate) fn verify_child(&self, child: &Child) -> Result<(), String> {
        if let Some(expected) = &self.private_sha256 {
            let actual = hash_file(&PathBuf::from(format!("/proc/{}/exe", child.id())))?;
            if &actual != expected {
                return Err(
                    "A identidade do processo MPD experimental divergiu da validação.".into(),
                );
            }
        }
        Ok(())
    }
}

fn hash_file(path: &Path) -> Result<String, String> {
    let file = File::open(path)
        .map_err(|_| "Não foi possível conferir a identidade do MPD.".to_string())?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length == 0 || length > MAX_ENGINE_BYTES {
        return Err("Tamanho inválido para o motor MPD.".into());
    }
    let mut reader = file.take(MAX_ENGINE_BYTES + 1);
    let mut digest = Sha256::new();
    let mut bytes = [0u8; 65536];
    let mut total = 0;
    loop {
        let count = reader.read(&mut bytes).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_ENGINE_BYTES {
            return Err("Motor MPD excedeu o limite de leitura.".into());
        }
        digest.update(&bytes[..count]);
    }
    if total != length {
        return Err("Motor MPD mudou durante a leitura.".into());
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn sampling_discards_changed_or_missing_context_and_never_reads_when_inactive() {
        let proof = EngineEvidence::Identified {
            evidence_id: "test",
            scope: "test",
            pcm_eof_flush: true,
        };
        assert!(sample_evidence(
            false,
            Some(&7),
            || panic!("inactive context read"),
            |_| panic!("inactive evidence read")
        )
        .is_none());
        assert!(sample_evidence::<u32>(
            true,
            None,
            || panic!("missing context read"),
            |_| panic!("missing evidence read")
        )
        .is_none());
        assert!(sample_evidence(
            true,
            Some(&7),
            || Some(8),
            |_| panic!("changed evidence read")
        )
        .is_none());
        let mut changes = [Some(7), None].into_iter();
        assert!(sample_evidence(
            true,
            Some(&7),
            || changes.next().flatten(),
            |_| Some(proof.clone())
        )
        .is_none());
        assert_eq!(
            sample_evidence(true, Some(&7), || Some(7), |_| Some(proof.clone())),
            Some(proof)
        );
        assert!(sample_evidence(true, Some(&7), || Some(7), |_| None).is_none());
    }

    #[test]
    fn evidence_is_scoped_to_the_pinned_build_and_output_configuration() {
        let system = MpdEngine::resolve_override(None, false, PRIVATE_SHA256).unwrap();
        assert!(system.evidence(false).is_none());
        let private = MpdEngine {
            program: PathBuf::from("/unused"),
            private_sha256: Some(PRIVATE_SHA256.into()),
        };
        let direct = serde_json::to_value(private.evidence(false).unwrap()).unwrap();
        assert_eq!(direct["scope"], "pcm_dsd_route_strict");
        assert_eq!(direct["evidence_id"], "mpd-strict-route-20261017");
        assert_eq!(direct["pcm_eof_flush"], true);
        assert_eq!(
            serde_json::to_value(private.evidence(true).unwrap()).unwrap()["pcm_eof_flush"],
            false
        );
        let changed = MpdEngine {
            program: PathBuf::from("/unused"),
            private_sha256: Some("0".repeat(64)),
        };
        assert!(changed.evidence(false).is_none());
        assert!(!direct.to_string().contains("/unused"));
        assert!(direct.get("bitperfect").is_none());
    }

    #[test]
    fn live_child_identity_is_checked_without_trusting_its_name() {
        let program = Path::new("/usr/bin/cat");
        let expected = hash_file(program).unwrap();
        let engine =
            MpdEngine::resolve_override(Some(program.as_os_str()), true, &expected).unwrap();
        // Keep stdin open so this owned fixture cannot expire while a busy
        // runner is descheduled between spawn and the executable hash checks.
        let mut child = std::process::Command::new(engine.program())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // A successful spawn alone is not the fixture's readiness proof.
        // Echo a bounded challenge before sampling /proc/<pid>/exe so this
        // test observes the running auxiliary, not its startup transition.
        let challenge = b"owned-fixture-ready\n";
        let sent = std::io::Write::write_all(child.stdin.as_mut().unwrap(), challenge);
        let mut output = child.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut echoed = [0u8; 20];
            let result = output.read_exact(&mut echoed).map(|_| echoed);
            let _ = send.send(result);
        });
        let ready = receive.recv_timeout(std::time::Duration::from_secs(5));
        let accepted = engine.verify_child(&child);
        let wrong = MpdEngine {
            program: program.into(),
            private_sha256: Some("0".repeat(64)),
        };
        let rejected = wrong.verify_child(&child);
        let _ = child.kill();
        child.wait().unwrap();
        reader.join().unwrap();
        assert!(sent.is_ok(), "owned fixture challenge: {sent:?}");
        assert!(matches!(&ready, Ok(Ok(bytes)) if bytes.as_slice() == challenge),
            "owned fixture must echo its readiness challenge: {ready:?}");
        assert!(accepted.is_ok(), "owned fixture identity: {accepted:?}");
        assert!(rejected.is_err());
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "sonante-engine-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            let path = dir.join("candidate");
            std::fs::write(&path, b"private fixture").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    #[test]
    fn explicit_debug_direct_hash_admission_preserves_the_matched_identity() {
        let accepted = MpdEngine::validated_override_hash(
            INSTALLED_DIRECT_SHA256.into(), PRIVATE_SHA256,
        );
        if cfg!(all(debug_assertions, target_os = "linux")) {
            assert_eq!(accepted.unwrap(), INSTALLED_DIRECT_SHA256);
        } else {
            assert!(accepted.is_err());
        }
        assert_eq!(MpdEngine::validated_override_hash(
            PRIVATE_SHA256.into(), PRIVATE_SHA256,
        ).unwrap(), PRIVATE_SHA256);
        assert_eq!(MpdEngine::validated_override_hash(
            INSTALLED_EQ_SHA256.into(), INSTALLED_EQ_SHA256,
        ).unwrap(), INSTALLED_EQ_SHA256);
        for (actual, expected) in [
            (INSTALLED_EQ_SHA256, PRIVATE_SHA256),
            (INSTALLED_DIRECT_SHA256, INSTALLED_EQ_SHA256),
            (PRIVATE_SHA256, INSTALLED_EQ_SHA256),
            (&INSTALLED_DIRECT_SHA256[..63], PRIVATE_SHA256),
            ("changed candidate", PRIVATE_SHA256),
        ] {
            assert!(MpdEngine::validated_override_hash(actual.into(), expected).is_err());
        }
    }

    #[test]
    fn explicit_installed_direct_identity_keeps_its_build_evidence_and_output_policy() {
        if !cfg!(all(debug_assertions, target_os = "linux")) { return; }
        let actual = MpdEngine::validated_override_hash(
            INSTALLED_DIRECT_SHA256.into(), PRIVATE_SHA256,
        ).unwrap();
        let engine = MpdEngine {
            program: "/not-executed/explicit-installed-direct".into(),
            private_sha256: Some(actual),
        };
        let evidence = serde_json::to_value(engine.evidence(false).unwrap()).unwrap();
        assert_eq!(evidence["evidence_id"], "mpd-strict-ubuntu2204-remap-20261018");
        assert_eq!(evidence["pcm_eof_flush"], true);
        assert!(evidence.get("bitperfect").is_none());
        let shared = "audio_output {\n type \"alsa\"\n}".to_string();
        assert_eq!(engine.configure_output(shared.clone(), true).unwrap(), shared);
        let direct = engine.configure_output(shared, false).unwrap();
        assert_eq!(direct.matches("pcm_eof_flush").count(), 1);
        assert_eq!(direct.matches("sonante_direct_strict").count(), 1);
    }

    #[test]
    fn default_uses_system_and_preserves_config() {
        let engine = MpdEngine::resolve_override(None, false, PRIVATE_SHA256).unwrap();
        assert_eq!(engine.program(), Path::new("mpd"));
        let section = "audio_output {\n type \"alsa\"\n}".to_string();
        assert_eq!(
            engine.configure_output(section.clone(), false).unwrap(),
            section
        );
    }
    #[test]
    fn release_override_is_rejected_without_fallback() {
        assert!(
            MpdEngine::resolve_override(Some(OsStr::new("/unused")), false, PRIVATE_SHA256)
                .is_err()
        );
    }
    #[test]
    fn relative_missing_and_wrong_hash_are_rejected() {
        assert!(
            MpdEngine::resolve_override(Some(OsStr::new("mpd")), true, PRIVATE_SHA256).is_err()
        );
        assert!(MpdEngine::resolve_override(
            Some(OsStr::new("/missing/private-mpd")),
            true,
            PRIVATE_SHA256
        )
        .is_err());
        let f = Fixture::new();
        assert!(MpdEngine::resolve_override(Some(f.0.as_os_str()), true, PRIVATE_SHA256).is_err());
    }
    #[test]
    fn exact_candidate_enables_only_direct_output() {
        let f = Fixture::new();
        let expected = hash_file(&f.0).unwrap();
        let engine = MpdEngine::resolve_override(Some(f.0.as_os_str()), true, &expected).unwrap();
        let section = "audio_output {\n type \"alsa\"\n dop \"yes\"\n}".to_string();
        assert_eq!(
            engine.configure_output(section.clone(), true).unwrap(),
            section
        );
        let direct = engine.configure_output(section, false).unwrap();
        assert_eq!(direct.matches("pcm_eof_flush").count(), 1);
        assert_eq!(direct.matches("sonante_direct_strict \"yes\"").count(), 1);
        assert!(direct.contains("dop \"yes\""));
        assert!(engine.configure_output("invalid".into(), false).is_err());
    }
    #[test]
    fn non_executable_and_changed_candidate_are_rejected() {
        let f = Fixture::new();
        let expected = hash_file(&f.0).unwrap();
        std::fs::set_permissions(&f.0, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(MpdEngine::resolve_override(Some(f.0.as_os_str()), true, &expected).is_err());
        std::fs::set_permissions(&f.0, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&f.0, b"changed").unwrap();
        assert!(MpdEngine::resolve_override(Some(f.0.as_os_str()), true, &expected).is_err());
    }
}
