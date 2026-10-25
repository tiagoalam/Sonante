use crate::config::ConfigSaveError;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

#[cfg(unix)]
const PRIVATE_DIR_MODE: u32 = 0o700;
#[cfg(unix)]
const PRIVATE_FILE_MODE: u32 = 0o600;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) fn sonante_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("sonante")
}

pub(crate) fn ensure_sonante_config_dir() -> Result<PathBuf, String> {
    let path = sonante_config_dir();
    ensure_private_directory(&path)?;
    Ok(path)
}

pub(crate) fn prepare_private_file_for_load(path: &Path, file_label: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Arquivo persistido do Sonante sem diretório pai.".to_string())?;
    ensure_private_directory(parent)?;
    harden_existing_private_file(path, file_label)
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Falha ao preparar diretório de configuração: {}", e))?;
    }

    #[cfg(unix)]
    let mut created = false;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(
                    "O diretório de configuração do Sonante não é um diretório privado válido."
                        .to_string(),
                );
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(PRIVATE_DIR_MODE);
            match builder.create(path) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        created = true;
                    }
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!(
                        "Falha ao criar diretório privado do Sonante: {}",
                        error
                    ));
                }
            }
        }
        Err(error) => {
            return Err(format!(
                "Falha ao verificar diretório privado do Sonante: {}",
                error
            ));
        }
    }

    #[cfg(unix)]
    if created {
        fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIR_MODE)).map_err(|e| {
            format!(
                "Falha ao aplicar permissões 0700 ao novo diretório do Sonante: {}",
                e
            )
        })?;
    }

    harden_existing_private_directory(path)
}

#[cfg(unix)]
fn harden_existing_private_directory(path: &Path) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
    let directory = options
        .open(path)
        .map_err(|e| format!("Falha ao abrir diretório privado do Sonante: {}", e))?;
    let metadata = directory
        .metadata()
        .map_err(|e| format!("Falha ao validar diretório privado do Sonante: {}", e))?;
    if !metadata.is_dir() {
        return Err("O diretório de configuração do Sonante não é válido.".to_string());
    }
    // chmod changes ctime even when its mode is unchanged; normal reads
    // must not invalidate a retained readonly profile/source snapshot.
    // Other modes/owners retain the previous chmod and error path.
    if metadata.uid() == unsafe { libc::geteuid() }
        && metadata.mode() & 0o7777 == PRIVATE_DIR_MODE { return Ok(()); }
    directory
        .set_permissions(fs::Permissions::from_mode(PRIVATE_DIR_MODE))
        .map_err(|e| {
            format!(
                "Falha ao aplicar permissões 0700 ao diretório do Sonante: {}",
                e
            )
        })
}

#[cfg(not(unix))]
fn harden_existing_private_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn harden_existing_private_file(path: &Path, file_label: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "{} não é um arquivo persistido privado válido.",
                file_label
            ));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!("{} não é um arquivo regular.", file_label));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!("Falha ao verificar {}: {}", file_label, error));
        }
    }

    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(path)
        .map_err(|e| format!("Falha ao abrir {} para proteção: {}", file_label, e))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Falha ao validar {}: {}", file_label, e))?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(format!(
            "{} não é um arquivo privado exclusivo do Sonante.",
            file_label
        ));
    }
    if metadata.uid() == unsafe { libc::geteuid() }
        && metadata.mode() & 0o7777 == PRIVATE_FILE_MODE { return Ok(()); }
    file.set_permissions(fs::Permissions::from_mode(PRIVATE_FILE_MODE))
        .map_err(|e| format!("Falha ao aplicar permissões 0600 a {}: {}", file_label, e))
}

#[cfg(not(unix))]
fn harden_existing_private_file(_path: &Path, _file_label: &str) -> Result<(), String> {
    Ok(())
}

fn create_private_temp_file(path: &Path, file_label: &str) -> Result<(File, PathBuf), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Arquivo persistido do Sonante sem diretório pai.".to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Nome inválido para {}.", file_label))?;

    for _ in 0..128 {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(".{}.{}.{}.tmp", file_name, std::process::id(), id));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options
            .mode(PRIVATE_FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW);

        match options.open(&temp_path) {
            Ok(file) => {
                #[cfg(unix)]
                file.set_permissions(fs::Permissions::from_mode(PRIVATE_FILE_MODE))
                    .map_err(|e| {
                        let _ = fs::remove_file(&temp_path);
                        format!(
                            "Falha ao aplicar permissões 0600 ao temporário de {}: {}",
                            file_label, e
                        )
                    })?;
                return Ok((file, temp_path));
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Falha ao criar arquivo temporário privado para {}: {}",
                    file_label, error
                ));
            }
        }
    }

    Err(format!(
        "Não foi possível reservar arquivo temporário privado para {}.",
        file_label
    ))
}

pub(crate) fn atomic_write_private(
    path: &Path,
    content: &[u8],
    file_label: &str,
) -> Result<(), String> {
    atomic_write_private_with_sync(
        path,
        content,
        file_label,
        |_| Ok(()),
        harden_existing_private_file,
    )
    .map_err(ConfigSaveError::into_cause)
}

// Sincroniza o temporário e, após validar o rename, o diretório pai.
// Isso não implementa journal nem recuperação após crash.
pub(crate) fn atomic_write_private_synced(
    path: &Path,
    content: &[u8],
    file_label: &str,
) -> Result<(), ConfigSaveError> {
    atomic_write_private_with_syncs(
        path,
        content,
        file_label,
        File::sync_all,
        harden_existing_private_file,
        Some(&File::sync_all),
    )
}

/// Reuses the durable writer while retaining a transaction's proven parent.
/// A substituted pathname cannot redirect its temporary, rename or cleanup.
pub(crate) fn atomic_write_private_synced_in(
    path: &Path,
    parent: &File,
    content: &[u8],
    file_label: &str,
) -> Result<(), ConfigSaveError> {
    atomic_write_private_with_parent(
        path,
        content,
        file_label,
        File::sync_all,
        harden_existing_private_file,
        Some(&File::sync_all),
        Some(parent),
    )
}

fn atomic_write_private_with_sync(
    path: &Path,
    content: &[u8],
    file_label: &str,
    sync_temp: impl FnOnce(&File) -> std::io::Result<()>,
    validate_destination: impl FnOnce(&Path, &str) -> Result<(), String>,
) -> Result<(), ConfigSaveError> {
    atomic_write_private_with_syncs(
        path,
        content,
        file_label,
        sync_temp,
        validate_destination,
        None,
    )
}

#[cfg(unix)]
pub(crate) fn validate_sync_directory(path: &Path, directory: &File) -> Result<(), String> {
    let opened = directory
        .metadata()
        .map_err(|e| format!("Falha ao validar diretório para sincronização: {e}"))?;
    let current = fs::symlink_metadata(path).map_err(|e| {
        format!("Falha ao verificar identidade do diretório para sincronização: {e}")
    })?;
    if !opened.is_dir()
        || opened.uid() != unsafe { libc::geteuid() }
        || opened.permissions().mode() & 0o777 != PRIVATE_DIR_MODE
        || !current.is_dir()
        || current.dev() != opened.dev()
        || current.ino() != opened.ino()
    {
        return Err(
            "O diretório privado para sincronização foi substituído ou não é seguro.".to_string(),
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn open_sync_directory(path: &Path) -> Result<File, String> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
        .map_err(|e| format!("Falha ao abrir diretório privado para sincronização: {e}"))?;
    validate_sync_directory(path, &directory)?;
    Ok(directory)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn open_sync_directory(_path: &Path) -> Result<File, String> {
    Err("Sincronização segura do diretório não suportada nesta plataforma.".to_string())
}

fn sync_directory_destination(directory: &File, path: &Path) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        let name = path
            .file_name()
            .ok_or_else(|| "Arquivo persistido do Sonante sem nome válido.".to_string())?;
        // O link do procfs referencia o descritor que permanece aberto durante
        // toda a escrita. Uma troca do path original não redireciona operações
        // nem cleanup; O_NOFOLLOW continua aplicado ao arquivo final/temporário.
        Ok(PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (directory, path);
        Err("Sincronização segura do diretório não suportada nesta plataforma.".to_string())
    }
}

#[cfg(not(unix))]
pub(crate) fn validate_sync_directory(_path: &Path, _directory: &File) -> Result<(), String> {
    Err("Sincronização segura do diretório não suportada nesta plataforma.".to_string())
}

fn atomic_write_private_with_syncs(
    path: &Path,
    content: &[u8],
    file_label: &str,
    sync_temp: impl FnOnce(&File) -> std::io::Result<()>,
    validate_destination: impl FnOnce(&Path, &str) -> Result<(), String>,
    sync_directory: Option<&dyn Fn(&File) -> std::io::Result<()>>,
) -> Result<(), ConfigSaveError> {
    atomic_write_private_with_parent(
        path,
        content,
        file_label,
        sync_temp,
        validate_destination,
        sync_directory,
        None,
    )
}

fn atomic_write_private_with_parent(
    path: &Path,
    content: &[u8],
    file_label: &str,
    sync_temp: impl FnOnce(&File) -> std::io::Result<()>,
    validate_destination: impl FnOnce(&Path, &str) -> Result<(), String>,
    sync_directory: Option<&dyn Fn(&File) -> std::io::Result<()>>,
    expected_parent: Option<&File>,
) -> Result<(), ConfigSaveError> {
    let parent = path
        .parent()
        .ok_or_else(|| "Arquivo persistido do Sonante sem diretório pai.".to_string())?;
    // Retém a identidade do pai antes do rename; nunca sincroniza um diretório
    // reaberto pelo path depois de uma possível substituição concorrente.
    let directory = if let Some(expected) = expected_parent {
        validate_sync_directory(parent, expected)?;
        Some(
            expected
                .try_clone()
                .map_err(|_| "Falha ao reter o diretório privado da transação.".to_string())?,
        )
    } else {
        ensure_private_directory(parent)?;
        sync_directory
            .map(|_| open_sync_directory(parent))
            .transpose()?
    };
    let destination = directory
        .as_ref()
        .map(|directory| sync_directory_destination(directory, path))
        .transpose()?;
    let destination = destination.as_deref().unwrap_or(path);

    let (mut file, temp_path) = create_private_temp_file(destination, file_label)?;
    let mut renamed = false;
    let result = (|| {
        file.write_all(content)
            .map_err(|e| format!("Falha ao gravar temporário de {}: {}", file_label, e))?;
        file.flush()
            .map_err(|e| format!("Falha ao concluir temporário de {}: {}", file_label, e))?;
        sync_temp(&file)
            .map_err(|e| format!("Falha ao sincronizar temporário de {}: {}", file_label, e))?;
        drop(file);
        if let Some(directory) = &directory {
            validate_sync_directory(parent, directory)?;
        }
        fs::rename(&temp_path, destination)
            .map_err(|e| format!("Falha ao substituir {} atomicamente: {}", file_label, e))?;
        renamed = true;
        validate_destination(destination, file_label)?;
        if let (Some(directory), Some(sync_directory)) = (&directory, sync_directory) {
            validate_sync_directory(parent, directory)?;
            sync_directory(directory).map_err(|e| {
                format!(
                    "Falha ao sincronizar diretório pai de {}: {}",
                    file_label, e
                )
            })?;
            validate_sync_directory(parent, directory)?;
        }
        Ok(())
    })();

    if result.is_err() {
        if let Err(cleanup_error) = fs::remove_file(&temp_path) {
            if cleanup_error.kind() != ErrorKind::NotFound {
                let cause = format!(
                    "{} Falha adicional ao remover temporário privado: {}",
                    result.unwrap_err(),
                    cleanup_error
                );
                return Err(if renamed {
                    ConfigSaveError::AfterRenameUnconfirmed(cause)
                } else {
                    ConfigSaveError::BeforeRename(cause)
                });
            }
        }
    }

    result.map_err(|cause| {
        if renamed {
            ConfigSaveError::AfterRenameUnconfirmed(cause)
        } else {
            ConfigSaveError::BeforeRename(cause)
        }
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn test_dir(name: &str) -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sonante-persistence-{}-{}-{}",
            std::process::id(),
            name,
            id
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    fn mode(path: &std::path::Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn transaction_parent_is_retained_through_temporary_sync_and_rejects_substitution() {
        let root = test_dir("transaction-parent");
        let parent = root.join("private");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let directory = open_sync_directory(&parent).unwrap();
        let path = parent.join("queue_cache.json");
        atomic_write_private_synced_in(&path, &directory, b"previous", "queue_cache.json").unwrap();
        let moved = root.join("original");
        let result = atomic_write_private_with_parent(
            &path,
            b"unconfirmed",
            "queue_cache.json",
            |file| {
                fs::rename(&parent, &moved)?;
                fs::create_dir(&parent)?;
                fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
                file.sync_all()
            },
            harden_existing_private_file,
            Some(&File::sync_all),
            Some(&directory),
        );
        assert!(matches!(result, Err(ConfigSaveError::BeforeRename(_))));
        assert_eq!(
            fs::read(moved.join("queue_cache.json")).unwrap(),
            b"previous"
        );
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&moved).unwrap().count(), 1);
        assert!(matches!(
            atomic_write_private_synced_in(&path, &directory, b"refused", "queue_cache.json"),
            Err(ConfigSaveError::BeforeRename(_))
        ));
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn new_file_is_created_with_private_permissions_and_same_content() {
        let dir = test_dir("new-file");
        let path = dir.join("config.json");
        let content = br#"{"first_run":true}"#;

        atomic_write_private(&path, content, "config.json").unwrap();

        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read(&path).unwrap(), content);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_file_is_hardened_from_0644_to_0600() {
        let dir = test_dir("legacy-file");
        let path = dir.join("favorites.json");
        fs::write(&path, b"[]").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        harden_existing_private_file(&path, "favorites.json").unwrap();

        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read(&path).unwrap(), b"[]");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn controlled_directory_is_hardened_to_0700() {
        let dir = test_dir("private-directory");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

        ensure_private_directory(&dir).unwrap();

        assert_eq!(mode(&dir), 0o700);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomic_temp_file_is_private_before_rename() {
        let dir = test_dir("temp-file");
        let path = dir.join("queue_cache.json");

        let (file, temp_path) = create_private_temp_file(&path, "queue_cache.json").unwrap();

        assert_eq!(mode(&temp_path), 0o600);
        drop(file);
        fs::remove_file(temp_path).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomic_replace_keeps_final_mode_and_roundtrip() {
        let dir = test_dir("atomic-replace");
        let path = dir.join("config.json");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let replacement = br#"{"audio_output_type":"alsa"}"#;

        atomic_write_private(&path, replacement, "config.json").unwrap();

        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read(&path).unwrap(), replacement);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn synced_write_creates_and_replaces_private_file() {
        let dir = test_dir("synced-write");
        let path = dir.join("config.json");

        for content in [b"initial".as_slice(), b"replacement".as_slice()] {
            atomic_write_private_synced(&path, content, "config.json").unwrap();
            assert_eq!(fs::read(&path).unwrap(), content);
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn directory_sync_follows_temp_sync_rename_and_validation() {
        let dir = test_dir("sync-order");
        let path = dir.join("config.json");
        fs::write(&path, b"original").unwrap();
        let parent_identity = fs::metadata(&dir).unwrap();
        let stage = Cell::new(0);

        atomic_write_private_with_syncs(
            &path,
            b"replacement",
            "config.json",
            |file| {
                assert_eq!(stage.replace(1), 0);
                assert_eq!(fs::read(&path).unwrap(), b"original");
                file.sync_all()
            },
            |destination, label| {
                assert_eq!(stage.replace(2), 1);
                assert_eq!(fs::read(&path).unwrap(), b"replacement");
                harden_existing_private_file(destination, label)
            },
            Some(&|directory: &File| {
                assert_eq!(stage.replace(3), 2);
                let opened = directory.metadata()?;
                assert!(opened.is_dir());
                assert_eq!(opened.dev(), parent_identity.dev());
                assert_eq!(opened.ino(), parent_identity.ino());
                assert_eq!(mode(&path), 0o600);
                directory.sync_all()
            }),
        )
        .unwrap();

        assert_eq!(stage.get(), 3);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn directory_sync_failure_is_after_rename_and_keeps_replacement() {
        for existing in [false, true] {
            let dir = test_dir("directory-sync-failure");
            let path = dir.join("config.json");
            let original = if existing {
                fs::write(&path, b"original").unwrap();
                Some(fs::metadata(&path).unwrap())
            } else {
                None
            };
            let validated = Cell::new(false);
            let synced = Cell::new(false);

            let error = atomic_write_private_with_syncs(
                &path,
                b"replacement",
                "config.json",
                File::sync_all,
                |destination, label| {
                    harden_existing_private_file(destination, label)?;
                    validated.set(true);
                    Ok(())
                },
                Some(&|directory: &File| {
                    assert!(validated.get());
                    assert!(directory.metadata()?.is_dir());
                    synced.set(true);
                    Err(std::io::Error::new(
                        ErrorKind::Other,
                        "simulated directory sync failure",
                    ))
                }),
            )
            .unwrap_err();

            assert!(synced.get());
            assert!(matches!(error, ConfigSaveError::AfterRenameUnconfirmed(_)));
            assert!(error
                .to_string()
                .contains("Falha ao sincronizar diretório pai de config.json"));
            assert!(error
                .to_string()
                .contains("simulated directory sync failure"));
            assert_eq!(fs::read(&path).unwrap(), b"replacement");
            if let Some(original) = original {
                assert_ne!(fs::metadata(&path).unwrap().ino(), original.ino());
            }
            assert_eq!(mode(&path), 0o600);
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn sync_directory_open_rejects_symlink_and_non_private_directory() {
        let dir = test_dir("unsafe-sync-directory");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(open_sync_directory(&link).is_err());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(open_sync_directory(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn directory_replacement_before_rename_cleans_only_original_directory() {
        let root = test_dir("parent-before-rename");
        let dir = root.join("private");
        let moved = root.join("moved");
        fs::create_dir(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(&path, b"original").unwrap();
        let mut decoy = None;

        let error = atomic_write_private_with_syncs(
            &path,
            b"replacement",
            "config.json",
            |file| {
                file.sync_all()?;
                let temp_name = fs::read_dir(&dir)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .find(|name| name.to_string_lossy().ends_with(".tmp"))
                    .unwrap();
                fs::rename(&dir, &moved).unwrap();
                fs::create_dir(&dir).unwrap();
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
                let decoy_path = dir.join(temp_name);
                fs::write(&decoy_path, b"unrelated").unwrap();
                decoy = Some(decoy_path);
                Ok(())
            },
            |_, _| panic!("não deve validar destino após troca do pai antes do rename"),
            Some(&|_: &File| panic!("não deve sincronizar pai substituído")),
        )
        .unwrap_err();

        assert!(matches!(error, ConfigSaveError::BeforeRename(_)));
        assert_eq!(fs::read(moved.join("config.json")).unwrap(), b"original");
        assert_eq!(fs::read_dir(&moved).unwrap().count(), 1);
        assert_eq!(fs::read(decoy.unwrap()).unwrap(), b"unrelated");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_replacement_after_rename_is_unconfirmed_and_never_syncs_replacement() {
        for symlink in [false, true] {
            let root = test_dir("parent-after-rename");
            let dir = root.join("private");
            let moved = root.join("moved");
            fs::create_dir(&dir).unwrap();
            let path = dir.join("config.json");

            let error = atomic_write_private_with_syncs(
                &path,
                b"replacement",
                "config.json",
                File::sync_all,
                |destination, label| {
                    fs::rename(&dir, &moved).unwrap();
                    if symlink {
                        std::os::unix::fs::symlink(&moved, &dir).unwrap();
                    } else {
                        fs::create_dir(&dir).unwrap();
                        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
                        fs::write(&path, b"unrelated").unwrap();
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
                    }
                    // A validação permanece vinculada ao diretório original.
                    harden_existing_private_file(destination, label)
                },
                Some(&|_: &File| panic!("não deve sincronizar pai substituído")),
            )
            .unwrap_err();

            assert!(matches!(error, ConfigSaveError::AfterRenameUnconfirmed(_)));
            assert_eq!(fs::read(moved.join("config.json")).unwrap(), b"replacement");
            assert_eq!(mode(&moved.join("config.json")), 0o600);
            if !symlink {
                assert_eq!(fs::read(&path).unwrap(), b"unrelated");
                assert_eq!(mode(&path), 0o644);
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn directory_replacement_during_sync_is_after_rename_unconfirmed() {
        let root = test_dir("parent-during-sync");
        let dir = root.join("private");
        let moved = root.join("moved");
        fs::create_dir(&dir).unwrap();
        let path = dir.join("config.json");
        let identity = fs::metadata(&dir).unwrap();

        let error = atomic_write_private_with_syncs(
            &path,
            b"replacement",
            "config.json",
            File::sync_all,
            harden_existing_private_file,
            Some(&|directory: &File| {
                fs::rename(&dir, &moved).unwrap();
                fs::create_dir(&dir).unwrap();
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
                assert_eq!(directory.metadata()?.ino(), identity.ino());
                directory.sync_all()
            }),
        )
        .unwrap_err();

        assert!(matches!(error, ConfigSaveError::AfterRenameUnconfirmed(_)));
        assert_eq!(fs::read(moved.join("config.json")).unwrap(), b"replacement");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sync_failure_prevents_rename_and_cleans_temp() {
        for existing in [false, true] {
            let dir = test_dir("sync-failure");
            let path = dir.join("config.json");
            let original = if existing {
                fs::write(&path, b"original").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                Some(fs::metadata(&path).unwrap())
            } else {
                None
            };
            let replacement = b"replacement";
            let mut sync_called = false;

            let error = atomic_write_private_with_syncs(
                &path,
                replacement,
                "config.json",
                |file| {
                    sync_called = true;
                    let metadata = file.metadata()?;
                    assert_eq!(metadata.len(), replacement.len() as u64);
                    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
                    assert_eq!(metadata.nlink(), 1);
                    if existing {
                        assert_eq!(fs::read(&path).unwrap(), b"original");
                    } else {
                        assert!(!path.exists());
                    }
                    Err(std::io::Error::new(
                        ErrorKind::Other,
                        "simulated sync failure",
                    ))
                },
                harden_existing_private_file,
                Some(&|_: &File| panic!("não deve sincronizar diretório após falha no temporário")),
            )
            .unwrap_err();

            assert!(sync_called);
            assert!(matches!(error, ConfigSaveError::BeforeRename(_)));
            assert!(error
                .to_string()
                .contains("Falha ao sincronizar temporário de config.json"));
            assert!(error.to_string().contains("simulated sync failure"));
            if let Some(original) = original {
                let preserved = fs::metadata(&path).unwrap();
                assert_eq!(preserved.dev(), original.dev());
                assert_eq!(preserved.ino(), original.ino());
                assert_eq!(mode(&path), 0o600);
                assert_eq!(fs::read(&path).unwrap(), b"original");
            } else {
                assert!(!path.exists());
            }
            assert_eq!(fs::read_dir(&dir).unwrap().count(), usize::from(existing));
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn rename_failure_is_before_rename_and_preserves_destination() {
        let dir = test_dir("rename-failure");
        let path = dir.join("config.json");
        fs::create_dir(&path).unwrap();
        let original = fs::metadata(&path).unwrap();

        let error = atomic_write_private_synced(&path, b"replacement", "config.json").unwrap_err();

        assert!(matches!(error, ConfigSaveError::BeforeRename(_)));
        assert_eq!(fs::metadata(&path).unwrap().ino(), original.ino());
        assert!(path.is_dir());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn validation_failure_after_rename_reports_uncertainty_and_keeps_replacement() {
        let dir = test_dir("validation-failure");
        let path = dir.join("config.json");
        fs::write(&path, b"original").unwrap();
        let original = fs::metadata(&path).unwrap();

        let error = atomic_write_private_with_syncs(
            &path,
            b"replacement",
            "config.json",
            File::sync_all,
            |path, label| {
                harden_existing_private_file(path, label)?;
                assert_eq!(fs::read(path).unwrap(), b"replacement");
                Err("simulated post-rename validation failure".to_string())
            },
            Some(&|_: &File| panic!("não deve sincronizar diretório após falha de validação")),
        )
        .unwrap_err();

        assert!(matches!(error, ConfigSaveError::AfterRenameUnconfirmed(_)));
        assert!(error
            .to_string()
            .contains("commit e durabilidade não confirmados"));
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        assert_ne!(fs::metadata(&path).unwrap().ino(), original.ino());
        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn post_rename_validation_still_rejects_symlinks_and_hardlinks() {
        for symlink in [false, true] {
            let dir = test_dir("post-rename-links");
            let path = dir.join("config.json");
            let other = dir.join("other.json");
            fs::write(&other, b"unrelated").unwrap();
            fs::set_permissions(&other, fs::Permissions::from_mode(0o644)).unwrap();

            let error = atomic_write_private_with_syncs(
                &path,
                b"replacement",
                "config.json",
                File::sync_all,
                |path, label| {
                    if symlink {
                        fs::remove_file(path).unwrap();
                        std::os::unix::fs::symlink(&other, path).unwrap();
                    } else {
                        fs::remove_file(&other).unwrap();
                        fs::hard_link(path, &other).unwrap();
                    }
                    harden_existing_private_file(path, label)
                },
                Some(&|_: &File| panic!("não deve sincronizar diretório após destino inseguro")),
            )
            .unwrap_err();

            assert!(matches!(error, ConfigSaveError::AfterRenameUnconfirmed(_)));
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
            if symlink {
                assert_eq!(fs::read(&other).unwrap(), b"unrelated");
                assert_eq!(mode(&other), 0o644);
                assert!(fs::symlink_metadata(&path)
                    .unwrap()
                    .file_type()
                    .is_symlink());
            } else {
                assert_eq!(fs::read(&path).unwrap(), b"replacement");
                assert_eq!(fs::metadata(&path).unwrap().nlink(), 2);
            }
            fs::remove_dir_all(dir).unwrap();
        }
    }
}
