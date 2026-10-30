//! One cooperative app owner per profile; ledger flock is a separate resource.
use crate::recovery_fs::{Dir, CHANGED, UNSAFE};
use std::{
    fs::{self, File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::Path,
};
const NAME: &str = ".sonante-profile.lock";
pub(crate) const BUSY: &str = "LIBRARY_RECOVERY_PROFILE_BUSY";
pub(crate) struct Lease {
    root: Dir,
    file: File,
    dev: u64,
    ino: u64,
}
impl Lease {
    pub fn acquire(path: &Path) -> Result<Self, String> {
        if !path.try_exists().map_err(|_| UNSAFE)? {
            // A new XDG_CONFIG_HOME may not exist before AppConfig is loaded.
            // Existing directories are still checked by Dir::open below.
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(|_| UNSAFE)?;
        }
        let root = Dir::open(path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(root.entry(NAME)?)
            .map_err(|_| UNSAFE)?;
        let m = file.metadata().map_err(|_| UNSAFE)?;
        if !m.is_file()
            || m.nlink() != 1
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o777 != 0o600
        {
            return Err(UNSAFE.into());
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(BUSY.into());
        }
        let result = Self {
            root,
            file,
            dev: m.dev(),
            ino: m.ino(),
        };
        result.validate()?;
        result.file.sync_all().map_err(|_| UNSAFE)?;
        result.root.sync()?;
        Ok(result)
    }
    pub fn root(&self) -> &Dir {
        &self.root
    }
    pub fn validate(&self) -> Result<(), String> {
        self.root.validate()?;
        let a = fs::symlink_metadata(self.root.entry(NAME)?).map_err(|_| CHANGED)?;
        let b = self.file.metadata().map_err(|_| CHANGED)?;
        if !a.is_file()
            || a.dev() != self.dev
            || a.ino() != self.ino
            || b.nlink() != 1
            || b.uid() != unsafe { libc::geteuid() }
            || b.mode() & 0o777 != 0o600
        {
            return Err(CHANGED.into());
        }
        Ok(())
    }
}
/// Never claim global absence from procfs. A bundle binds the creator's PID
/// namespace/boot; recovery requires that same scope, a dead creator, no other
/// visible Sonante, and no surviving profile runtime. Unreadable/hidden/nested
/// process inventory is Unknown and cannot authorize effects.
pub(crate) fn process_scope() -> Result<String, String> {
    let status = fs::read_to_string("/proc/self/status").map_err(|_| UNSAFE)?;
    let pid = status
        .lines()
        .find_map(|l| l.strip_prefix("NSpid:"))
        .ok_or(UNSAFE)?;
    if pid.split_whitespace().count() != 1 {
        return Err("LIBRARY_RECOVERY_OWNERSHIP_UNKNOWN".into());
    }
    let mounts = fs::read_to_string("/proc/self/mountinfo").map_err(|_| UNSAFE)?;
    if mounts
        .lines()
        .filter(|l| l.contains(" - proc "))
        .any(|l| l.contains("hidepid=") && !l.contains("hidepid=0"))
    {
        return Err("LIBRARY_RECOVERY_OWNERSHIP_UNKNOWN".into());
    }
    let scope = fs::read_link("/proc/self/ns/pid").map_err(|_| UNSAFE)?;
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(|_| UNSAFE)?;
    Ok(format!("{}:{}", boot.trim(), scope.to_string_lossy()))
}
pub(crate) fn creator_start() -> Result<String, String> {
    let stat = fs::read_to_string("/proc/self/stat").map_err(|_| UNSAFE)?;
    Ok(stat
        .rsplit_once(')')
        .ok_or(UNSAFE)?
        .1
        .split_whitespace()
        .nth(19)
        .ok_or(UNSAFE)?
        .to_string())
}
pub(crate) fn legacy_fence(
    root: &Dir,
    scope: &str,
    creator: u32,
    start: &str,
    runtime: &Path,
) -> Result<(), String> {
    let creator_may_exist = scope_after_reboot(scope, &process_scope()?)?;
    if creator_may_exist && creator != std::process::id() {
        match fs::read_to_string(format!("/proc/{creator}/stat")) {
            Ok(stat) => {
                if stat
                    .rsplit_once(')')
                    .and_then(|(_, s)| s.split_whitespace().nth(19))
                    == Some(start)
                {
                    return Err(BUSY.into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("LIBRARY_RECOVERY_OWNERSHIP_UNKNOWN".into()),
        }
    }
    inspect_inventory(Path::new("/proc"), std::process::id())?;
    runtime_absent(runtime)?;
    root.validate()
}
fn scope_after_reboot(previous: &str, current: &str) -> Result<bool, String> {
    fn parse(s: &str) -> Result<(&str, &str), String> {
        let (boot, namespace) = s
            .split_once(':')
            .ok_or("LIBRARY_RECOVERY_OWNERSHIP_UNKNOWN")?;
        if boot.len() != 36
            || !boot.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
            || !namespace.starts_with("pid:[")
            || !namespace.ends_with(']')
        {
            return Err("LIBRARY_RECOVERY_OWNERSHIP_UNKNOWN".into());
        }
        Ok((boot, namespace))
    }
    let (a_boot, a_ns) = parse(previous)?;
    let (b_boot, b_ns) = parse(current)?;
    if a_boot != b_boot {
        return Ok(false);
    }
    if a_ns != b_ns {
        return Err("LIBRARY_RECOVERY_OWNERSHIP_UNKNOWN".into());
    }
    Ok(true)
}
fn inspect_inventory(proc_root: &Path, own_pid: u32) -> Result<(), String> {
    for item in fs::read_dir(proc_root).map_err(|_| UNSAFE)? {
        let item = item.map_err(|_| UNSAFE)?;
        let name = item.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if pid == own_pid {
            continue;
        }
        let metadata = match fs::metadata(item.path()) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("LIBRARY_RECOVERY_OWNERSHIP_UNKNOWN".into()),
        };
        if metadata.uid() != unsafe { libc::geteuid() } {
            continue;
        }
        let comm = match fs::read_to_string(item.path().join("comm")) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !item.path().exists() => continue,
            Err(_) => return Err("LIBRARY_RECOVERY_OWNERSHIP_UNKNOWN".into()),
        };
        if comm.trim().eq_ignore_ascii_case("sonante") {
            return Err(BUSY.into());
        }
    }
    Ok(())
}
fn runtime_absent(runtime: &Path) -> Result<(), String> {
    // A stale runtime is not deleted or signalled. Its owner must be resolved
    // before this operation can proceed, even if its numeric PID was recycled.
    for p in [runtime.to_path_buf(), runtime.with_file_name("mpd.pid")] {
        match fs::symlink_metadata(p) {
            Ok(_) => return Err("LIBRARY_RECOVERY_OWNERSHIP_UNKNOWN".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(UNSAFE.into()),
        }
    }
    Ok(())
}
pub(crate) struct State {
    lease: std::sync::Mutex<Option<std::sync::Arc<Lease>>>,
    error: Option<String>,
    restart: std::sync::atomic::AtomicBool,
}
impl State {
    pub fn new(path: &Path) -> Self {
        let (lease, error) = match Lease::acquire(path) {
            Ok(v) => (Some(std::sync::Arc::new(v)), None),
            Err(e) => (None, Some(e)),
        };
        Self {
            lease: std::sync::Mutex::new(lease),
            error,
            restart: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn lease(&self) -> Result<std::sync::Arc<Lease>, String> {
        self.lease
            .lock()
            .map_err(|_| UNSAFE)?
            .clone()
            .ok_or_else(|| self.error.clone().unwrap_or_else(|| BUSY.into()))
    }
    pub fn request_reopen(&self) {
        self.restart
            .store(true, std::sync::atomic::Ordering::Release);
    }
    pub fn reopen_requested(&self) -> bool {
        self.restart.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn release(&self) {
        if let Ok(mut lease) = self.lease.lock() {
            *lease = None;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn fixture(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("sonante-profile-{}-{name}", std::process::id()));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        p
    }

    #[test]
    fn first_run_lease_absent_xdg_parent_is_admitted_before_other_profile_io() {
        let base = fixture("first-run-absent-xdg");
        let profile = base.join("new-xdg/nested/sonante");
        let state = State::new(&profile);
        let result = crate::managed_save_intent::guard_startup(&profile, || {
            state.lease()?.validate()?;
            crate::config::AppConfig::load_from_path(&profile.join("config.json"))
        });
        if result.is_err() {
            // Characterize why an empty directory seen after failure is not proof
            // that the lease was acquired when run() originally constructed State.
            crate::persistence::prepare_private_file_for_load(
                &profile.join("queue_cache.json"), "queue_cache.json"
            ).unwrap();
            eprintln!("first_run_counterproof: later_directory_created={}, cached_lease_invalid={}",
                profile.is_dir(), state.lease().is_err());
        }
        let config = result.expect("A genuinely new private XDG tree must reach first_run");
        assert!(config.first_run);
        let config_state = crate::ConfigState(std::sync::Mutex::new(config.clone()));
        let transactions = crate::ConfigTransactionState::new();
        let reloaded = crate::read_config_after_startup_reconciliation(
            &config_state, &transactions, &profile
        ).unwrap();
        assert_eq!(reloaded, config);
        assert!(!crate::should_start_mpd_on_startup(true, &config));
        assert!(!profile.join("config.json").exists());
        assert_eq!(fs::metadata(base.join("new-xdg")).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(base.join("new-xdg/nested")).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&profile).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(profile.join(NAME)).unwrap().mode() & 0o777, 0o600);
        assert_eq!(fs::read_dir(&profile).unwrap().count(), 1);
        drop(state);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn first_run_lease_existing_parent_and_empty_profile_keep_first_run() {
        for present in [false, true] {
            let base = fixture(if present {"first-run-empty-profile"} else {"first-run-parent"});
            let profile = base.join("sonante");
            if present {
                fs::DirBuilder::new().mode(0o700).create(&profile).unwrap();
            }
            let state = State::new(&profile);
            let config = crate::managed_save_intent::guard_startup(&profile, || {
                state.lease()?.validate()?;
                crate::config::AppConfig::load_from_path(&profile.join("config.json"))
            }).unwrap();
            assert!(config.first_run);
            assert!(!crate::should_start_mpd_on_startup(true, &config));
            assert!(!profile.join("config.json").exists());
            assert_eq!(fs::read_dir(&profile).unwrap().count(), 1);
            drop(state);
            fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn first_run_lease_existing_valid_configuration_is_not_reset() {
        let profile = fixture("first-run-existing-config");
        let mut expected = crate::config::AppConfig::default();
        expected.first_run = false;
        let bytes = serde_json::to_vec(&expected).unwrap();
        fs::write(profile.join("config.json"), &bytes).unwrap();
        fs::set_permissions(profile.join("config.json"), fs::Permissions::from_mode(0o600)).unwrap();
        let state = State::new(&profile);
        let loaded = crate::managed_save_intent::guard_startup(&profile, || {
            state.lease()?.validate()?;
            crate::config::AppConfig::load_from_path(&profile.join("config.json"))
        }).unwrap();
        assert_eq!(loaded, expected);
        assert_eq!(fs::read(profile.join("config.json")).unwrap(), bytes);
        drop(state);
        fs::remove_dir_all(profile).unwrap();
    }

    #[test]
    fn first_run_lease_unsafe_profile_never_becomes_first_run() {
        for case in ["file", "symlink", "writable"] {
            let base = fixture(&format!("first-run-refuse-{case}"));
            let profile = base.join("sonante");
            let target = base.join("target");
            fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
            match case {
                "file" => fs::write(&profile, b"preserve").unwrap(),
                "symlink" => std::os::unix::fs::symlink(&target, &profile).unwrap(),
                _ => {
                    fs::DirBuilder::new().mode(0o700).create(&profile).unwrap();
                    fs::set_permissions(&profile, fs::Permissions::from_mode(0o777)).unwrap();
                }
            }
            let state = State::new(&profile);
            assert!(state.lease().is_err());
            let entered = std::cell::Cell::new(false);
            let result = crate::managed_save_intent::guard_startup(&profile, || {
                state.lease()?.validate()?;
                entered.set(true);
                crate::config::AppConfig::load_from_path(&profile.join("config.json"))
            });
            assert!(result.is_err());
            assert!(!entered.get());
            assert!(!target.join(NAME).exists());
            if case=="file" {assert_eq!(fs::read(&profile).unwrap(), b"preserve");}
            if case=="writable" {assert_eq!(fs::metadata(&profile).unwrap().mode() & 0o777, 0o777);}
            drop(state);
            fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn cooperative_child_competitor() {
        if let Some(path) = std::env::var_os("SONANTE_LEASE_TEST_PROFILE") {
            assert!(Lease::acquire(Path::new(&path)).is_err());
            let state = State::new(Path::new(&path));
            let entered = std::cell::Cell::new(false);
            let result = crate::managed_save_intent::guard_startup(Path::new(&path), || {
                state.lease()?.validate()?;
                entered.set(true);
                Ok(())
            });
            assert!(result.is_err());
            assert!(!entered.get());
        }
    }
    #[test]
    fn two_processes_share_one_profile_lease_and_replacement_is_detected() {
        let path = fixture("two-processes");
        let lease = Lease::acquire(&path).unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("profile_lease::tests::cooperative_child_competitor")
            .env("SONANTE_LEASE_TEST_PROFILE", &path)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stdout)
        );
        assert!(Lease::acquire(&path).is_err());
        lease.validate().unwrap();
        fs::rename(path.join(NAME), path.join("original-lock")).unwrap();
        fs::write(path.join(NAME), b"").unwrap();
        assert!(lease.validate().is_err());
        drop(lease);
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn unsafe_lock_file_and_symlink_profile_are_refused() {
        for mode in ["symlink", "hardlink", "permissions"] {
            let path = fixture(mode);
            let lease = Lease::acquire(&path).unwrap();
            drop(lease);
            match mode {
                "symlink" => {
                    fs::rename(path.join(NAME), path.join("original")).unwrap();
                    std::os::unix::fs::symlink("original", path.join(NAME)).unwrap();
                }
                "hardlink" => fs::hard_link(path.join(NAME), path.join("extra")).unwrap(),
                _ => {
                    fs::set_permissions(path.join(NAME), fs::Permissions::from_mode(0o644)).unwrap()
                }
            };
            assert!(Lease::acquire(&path).is_err());
            fs::remove_dir_all(path).unwrap();
        }
    }
    #[test]
    fn older_or_unreadable_instance_inventory_and_unknown_runtime_refuse_ownership() {
        let path = fixture("legacy-inventory");
        let proc = path.join("proc");
        fs::create_dir(&proc).unwrap();
        let pid = proc.join("1234");
        fs::create_dir(&pid).unwrap();
        fs::write(pid.join("comm"), b"sonante\n").unwrap();
        assert!(inspect_inventory(&proc, 9999).is_err());
        fs::write(pid.join("comm"), b"other-process\n").unwrap();
        assert!(inspect_inventory(&proc, 9999).is_ok());
        fs::remove_file(pid.join("comm")).unwrap();
        assert!(inspect_inventory(&proc, 9999).is_err());
        let socket = path.join("socket");
        assert!(runtime_absent(&socket).is_ok());
        fs::write(&socket, b"unknown runtime marker").unwrap();
        assert!(runtime_absent(&socket).is_err());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn boot_change_revokes_old_pid_but_same_boot_namespace_change_is_unknown() {
        let old = "00000000-0000-0000-0000-000000000001:pid:[123]";
        assert_eq!(scope_after_reboot(old, old).unwrap(), true);
        assert_eq!(
            scope_after_reboot(old, "00000000-0000-0000-0000-000000000002:pid:[456]").unwrap(),
            false
        );
        assert!(scope_after_reboot(old, "00000000-0000-0000-0000-000000000001:pid:[456]").is_err());
        assert!(scope_after_reboot("unproven", old).is_err());
    }
}
