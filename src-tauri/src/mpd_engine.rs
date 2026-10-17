//! Explicit development-only selection of the private PCM EOF candidate.
//! This identifies a build; it does not certify a live audio route.
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Child;

const PRIVATE_SHA256: &str = "a8a49a518beea9ff11599b53d493b92c02d93563f8102e4fec73aab077966582";
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
        Self::resolve_override(
            std::env::var_os("SONANTE_DEV_MPD_PATH").as_deref(),
            cfg!(all(debug_assertions, target_os = "linux")),
            PRIVATE_SHA256,
        )
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
        if hash_file(&program)? != expected {
            return Err(
                "O motor MPD experimental não corresponde à compilação privada validada.".into(),
            );
        }
        Ok(Self {
            program,
            private_sha256: Some(expected.into()),
        })
    }

    pub(crate) fn program(&self) -> &Path {
        &self.program
    }

    // Cached only after the owned child's executable has been verified.
    // This is build provenance, never a current-stream bit comparison.
    pub(crate) fn evidence(&self, is_shared: bool) -> Option<EngineEvidence> {
        (self.private_sha256.as_deref() == Some(PRIVATE_SHA256)).then_some(
            EngineEvidence::Identified {
                evidence_id: "mpd-route-20261017",
                scope: "pcm_dsd_route",
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
        Ok(format!("{prefix}\n    pcm_eof_flush \"yes\"\n}}"))
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
        assert_eq!(direct["scope"], "pcm_dsd_route");
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
        let program = Path::new("/usr/bin/sleep");
        let expected = hash_file(program).unwrap();
        let engine =
            MpdEngine::resolve_override(Some(program.as_os_str()), true, &expected).unwrap();
        let mut child = std::process::Command::new(engine.program())
            .arg("2")
            .spawn()
            .unwrap();
        let accepted = engine.verify_child(&child);
        let wrong = MpdEngine {
            program: program.into(),
            private_sha256: Some("0".repeat(64)),
        };
        let rejected = wrong.verify_child(&child);
        let _ = child.kill();
        child.wait().unwrap();
        assert!(accepted.is_ok());
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
