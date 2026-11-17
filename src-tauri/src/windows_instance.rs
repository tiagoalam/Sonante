//! One participating Sonante process per app-local profile, before any workers.
//! No PID, stale-file deletion, process lookup, focus forwarding or IPC server.
use std::{
    fs::{File, OpenOptions, TryLockError},
    path::Path,
    sync::OnceLock,
};
// Statics are intentionally not dropped during Tauri-managed-state teardown.
// A timed-out store worker must not outlive the profile lock in a live process.
static PROFILE: OnceLock<File> = OnceLock::new();
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Busy,
    Unavailable,
}
impl Failure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Busy => "windows_profile_busy",
            Self::Unavailable => "windows_profile_unavailable",
        }
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for Failure {}
fn allowed(metadata: &std::fs::Metadata) -> bool {
    if !metadata.is_file() || metadata.len() != 0 || crate::persistence::linked(metadata) {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_RECALL_ON_OPEN,
        };
        if metadata.file_attributes()
            & (FILE_ATTRIBUTE_OFFLINE
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
            != 0
        {
            return false;
        }
    }
    true
}
fn acquire(path: &Path) -> Result<File, Failure> {
    if !path.is_absolute() {
        return Err(Failure::Unavailable);
    }
    crate::persistence::prepare_private_file_for_load(path, "windows-profile")
        .map_err(|_| Failure::Unavailable)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };
        // Other participating processes may open the same inode to attempt the
        // nonblocking lock. Deny delete/rename, and never inherit this handle.
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL);
    }
    let file = options.open(path).map_err(|_| Failure::Unavailable)?;
    if !allowed(&file.metadata().map_err(|_| Failure::Unavailable)?) {
        return Err(Failure::Unavailable);
    }
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(Failure::Busy),
        Err(TryLockError::Error(_)) => return Err(Failure::Unavailable),
    }
    if !allowed(&file.metadata().map_err(|_| Failure::Unavailable)?) {
        return Err(Failure::Unavailable);
    }
    Ok(file)
}
fn admit_into(slot: &OnceLock<File>, path: &Path) -> Result<(), Failure> {
    if slot.get().is_some() {
        return Err(Failure::Unavailable);
    }
    let file = acquire(path)?;
    slot.set(file).map_err(|_| Failure::Unavailable)
}
pub fn admit(path: &Path) -> Result<(), Failure> {
    admit_into(&PROFILE, path)
}
pub(crate) fn validate() -> Result<(), Failure> {
    let file = PROFILE.get().ok_or(Failure::Unavailable)?;
    if !allowed(&file.metadata().map_err(|_|Failure::Unavailable)?) { return Err(Failure::Unavailable); }
    Ok(())
}
pub fn message(failure: Failure, portuguese: bool) -> &'static str {
    match (failure,portuguese){
 (Failure::Busy,true)=>"O Sonante já está aberto com estes dados. Feche a outra janela do aplicativo e tente novamente. A fila e as playlists não foram alteradas por esta tentativa.",
 (Failure::Busy,false)=>"Sonante is already open with this data. Close the other copy of the application and try again. This attempt did not change your queue or playlists.",
 (Failure::Unavailable,true)=>"Não foi possível abrir os dados do Sonante com segurança. Verifique o acesso ao disco e tente novamente. Os arquivos existentes não foram apagados ou redefinidos.",
 (Failure::Unavailable,false)=>"Sonante could not safely open its data. Check access to the disk and try again. Existing files were not deleted or reset."
}
}
#[cfg(windows)]
pub fn notify(failure: Failure) {
    let language = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    let _ = rfd::MessageDialog::new()
        .set_title("Sonante")
        .set_level(rfd::MessageLevel::Warning)
        .set_description(message(failure, language & 0x03ff == 0x16))
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut random = [0; 12];
            getrandom::fill(&mut random).unwrap();
            let p = std::env::temp_dir().join(format!("sonante-instance-{random:x?}"));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
        fn lock(&self) -> PathBuf {
            self.0.join("windows-profile.lock")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn busy_profile_never_admits_second_stale_journal_or_changes_data() {
        use crate::windows_mpd::library::Track;
        // Keep the end-to-end fixture independent from Tauri and media I/O.
        let f = Fixture::new();
        let slot = OnceLock::new();
        admit_into(&slot, &f.lock()).unwrap();
        let file = f.0.join("queue.json");
        let mut first = crate::windows_mpd::TestJournal::open(file.clone());
        let track = Track {
            id: "track".into(),
            title: "Fixture".into(),
            artist: String::new(),
            album: String::new(),
            disc: 1,
            number: 1,
            file_name: "fixture.flac".into(),
            catalog_album_id: Some("album".into()),
            media_root: None,
            radio: None,upnp:None,
            remote_source: None,
            remote_connection_id: None,
            plex_image:None,remote_lease: None,
            path: f.0.join("fixture.flac"),
        };
        first
            .replace_logical(Some(crate::windows_mpd::TestSavedQueue::capture(
                &f.0,
                &[track],
                0,
                0.,
                0.,
                50,
                None,
            )))
            .unwrap();
        let before = fs::read(&file).unwrap();
        let other = OnceLock::new();
        let second = admit_into(&other, &f.lock());
        assert_eq!(second, Err(Failure::Busy));
        if second.is_ok() {
            crate::windows_mpd::TestJournal::open(file.clone())
                .clear()
                .unwrap();
        }
        assert_eq!(fs::read(&file).unwrap(), before);
        assert!(crate::windows_mpd::TestJournal::open(file).saved.is_some());
        drop(slot);
    }
    #[test]
    fn empty_stale_lock_is_not_a_pidfile_and_reopen_after_owner_close_succeeds() {
        let f = Fixture::new();
        fs::write(f.lock(), []).unwrap();
        let file = acquire(&f.lock()).unwrap();
        assert_eq!(acquire(&f.lock()).unwrap_err(), Failure::Busy);
        assert_eq!(fs::metadata(f.lock()).unwrap().len(), 0);
        drop(file);
        drop(acquire(&f.lock()).unwrap());
        assert!(f.lock().exists());
    }
    #[test]
    fn retained_guard_survives_initialization_scope_and_unrelated_object_teardown() {
        let f = Fixture::new();
        let slot = OnceLock::new();
        {
            admit_into(&slot, &f.lock()).unwrap();
            let _unrelated = Vec::<u8>::new();
        }
        assert_eq!(acquire(&f.lock()).unwrap_err(), Failure::Busy);
        assert_eq!(admit_into(&slot, &f.lock()), Err(Failure::Unavailable));
        drop(slot);
        drop(acquire(&f.lock()).unwrap());
    }
    #[test]
    fn separate_profiles_do_not_share_a_global_named_lock() {
        let a = Fixture::new();
        let b = Fixture::new();
        let one = acquire(&a.lock()).unwrap();
        let two = acquire(&b.lock()).unwrap();
        assert_eq!(acquire(&a.lock()).unwrap_err(), Failure::Busy);
        assert_eq!(acquire(&b.lock()).unwrap_err(), Failure::Busy);
        drop(one);
        drop(two);
    }
    #[test]
    fn invalid_lock_directory_and_nonempty_content_are_preserved_not_reported_as_another_instance()
    {
        let f = Fixture::new();
        fs::write(f.lock(), b"foreign fixture").unwrap();
        assert_eq!(acquire(&f.lock()).unwrap_err(), Failure::Unavailable);
        assert_eq!(fs::read(f.lock()).unwrap(), b"foreign fixture");
        fs::remove_file(f.lock()).unwrap();
        fs::create_dir(f.lock()).unwrap();
        assert_eq!(acquire(&f.lock()).unwrap_err(), Failure::Unavailable);
        assert!(f.lock().is_dir());
        assert_eq!(
            acquire(Path::new("relative.lock")).unwrap_err(),
            Failure::Unavailable
        );
    }
    #[cfg(unix)]
    #[test]
    fn symbolic_lock_is_refused_and_hardlink_alias_cannot_obtain_another_lock() {
        let f = Fixture::new();
        let target = f.0.join("foreign");
        fs::write(&target, b"foreign fixture").unwrap();
        std::os::unix::fs::symlink(&target, f.lock()).unwrap();
        assert_eq!(acquire(&f.lock()).unwrap_err(), Failure::Unavailable);
        assert_eq!(fs::read(target).unwrap(), b"foreign fixture");
        fs::remove_file(f.lock()).unwrap();
        let owner = acquire(&f.lock()).unwrap();
        let alias = f.0.join("alias.lock");
        fs::hard_link(f.lock(), &alias).unwrap();
        assert_eq!(acquire(&alias).unwrap_err(), Failure::Busy);
        drop(owner);
    }
    #[cfg(windows)]
    #[test]
    fn native_guard_is_not_inherited_and_name_cannot_be_replaced_while_held() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};
        let f = Fixture::new();
        let owner = acquire(&f.lock()).unwrap();
        let mut flags = 0;
        assert_ne!(
            unsafe { GetHandleInformation(owner.as_raw_handle(), &mut flags) },
            0
        );
        assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
        assert!(fs::remove_file(f.lock()).is_err());
        assert!(fs::rename(f.lock(), f.0.join("replacement.lock")).is_err());
        assert_eq!(acquire(&f.lock()).unwrap_err(), Failure::Busy);
        drop(owner);
    }
    #[test]
    #[ignore = "child helper invoked only by owned_child_exit_releases_profile_lock"]
    fn child_lock_holder() {
        let root =
            std::env::var_os("SONANTE_INSTANCE_TEST_ROOT").expect("isolated child fixture root");
        let root = PathBuf::from(root);
        let _owned = acquire(&root.join("windows-profile.lock")).unwrap();
        fs::write(root.join("ready"), b"ready").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
    #[test]
    #[ignore = "Run separately: spawning a process may temporarily inherit parallel Linux fixture locks before exec"]
    fn owned_child_exit_releases_profile_lock_without_deleting_a_stale_file() {
        use std::{
            process::{Child, Command, Stdio},
            time::{Duration, Instant},
        };
        struct Owned(Child);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let f = Fixture::new();
        let mut child = Owned(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "windows_instance::tests::child_lock_holder",
                    "--ignored",
                ])
                .env("SONANTE_INSTANCE_TEST_ROOT", &f.0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while !f.0.join("ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "isolated child failed"
            );
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(acquire(&f.lock()).unwrap_err(), Failure::Busy);
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match acquire(&f.lock()) {
                Ok(file) => {
                    drop(file);
                    break;
                }
                Err(Failure::Busy) => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("{e:?}"),
            }
        }
        assert!(f.lock().exists());
    }
}
