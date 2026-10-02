use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
    let parent = path
        .parent()
        .ok_or_else(|| "Arquivo persistido do Sonante sem diretório pai.".to_string())?;
    ensure_private_directory(parent)?;

    let (mut file, temp_path) = create_private_temp_file(path, file_label)?;
    let result = (|| {
        file.write_all(content)
            .map_err(|e| format!("Falha ao gravar temporário de {}: {}", file_label, e))?;
        file.flush()
            .map_err(|e| format!("Falha ao concluir temporário de {}: {}", file_label, e))?;
        drop(file);
        fs::rename(&temp_path, path)
            .map_err(|e| format!("Falha ao substituir {} atomicamente: {}", file_label, e))?;
        harden_existing_private_file(path, file_label)
    })();

    if result.is_err() {
        if let Err(cleanup_error) = fs::remove_file(&temp_path) {
            if cleanup_error.kind() != ErrorKind::NotFound {
                return Err(format!(
                    "{} Falha adicional ao remover temporário privado: {}",
                    result.unwrap_err(),
                    cleanup_error
                ));
            }
        }
    }

    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
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
}
