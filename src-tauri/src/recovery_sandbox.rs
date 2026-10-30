//! Linux-only validation-child confinement using existing kernel APIs. No
//! dependency installation, privilege escalation, namespace change, or fallback
//! to an unconfined daemon if the kernel refuses the rules.
use std::{
    fs::File,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};
const UNAVAILABLE: &str = "LIBRARY_RECOVERY_VALIDATION_SANDBOX_UNAVAILABLE";
#[repr(C)]
struct Ruleset {
    fs: u64,
}
#[repr(C, packed)]
struct Beneath {
    allowed: u64,
    parent: i32,
}
pub(crate) struct Sandbox {
    rules: File,
}
impl Sandbox {
    pub fn prepare(root: &crate::recovery_fs::Dir, program: &Path) -> Result<Self, String> {
        let abi = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<u8>(),
                0usize,
                1u32,
            )
        };
        if abi < 1 {
            return Err(UNAVAILABLE.into());
        }
        let mut access = (1u64 << 13) - 1; // ABI1 execute/read/write/remove/make actions.
        if abi >= 2 {
            access |= 1 << 13;
        } // REFER
        if abi >= 3 {
            access |= 1 << 14;
        } // TRUNCATE
        let attr = Ruleset { fs: access };
        let fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr,
                std::mem::size_of::<Ruleset>(),
                0u32,
            )
        };
        if fd < 0 {
            return Err(UNAVAILABLE.into());
        }
        let rules = unsafe { File::from_raw_fd(fd as i32) };
        if unsafe { libc::fcntl(rules.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(UNAVAILABLE.into());
        }
        let add = |path: &Path, allowed: u64| -> Result<(), String> {
            let path = path.canonicalize().map_err(|_| UNAVAILABLE)?;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)
                .map_err(|_| UNAVAILABLE)?;
            let rule = Beneath {
                allowed,
                parent: file.as_raw_fd(),
            };
            if unsafe {
                libc::syscall(
                    libc::SYS_landlock_add_rule,
                    rules.as_raw_fd(),
                    1u32,
                    &rule,
                    0u32,
                )
            } < 0
            {
                return Err(UNAVAILABLE.into());
            }
            Ok(())
        };
        root.validate()?;
        add(&root.path, access)?;
        for path in ["/usr", "/lib", "/lib64", "/bin"] {
            if Path::new(path).exists() {
                add(Path::new(path), 1 | 4 | 8)?;
            }
        }
        if Path::new("/etc").exists() {
            add(Path::new("/etc"), 4 | 8)?;
        }
        add(Path::new("/dev/null"), 2 | 4)?;
        add(Path::new("/dev/urandom"), 4)?;
        let program = if program.is_absolute() {
            program.to_path_buf()
        } else {
            std::env::split_paths(&std::env::var_os("PATH").ok_or(UNAVAILABLE)?)
                .map(|p| p.join(program))
                .find(|p| p.is_file())
                .ok_or(UNAVAILABLE)?
        };
        add(&program, 1 | 4)?;
        root.validate()?;
        Ok(Self { rules })
    }
    /// Called only from Command::pre_exec; stack data and raw syscalls only.
    pub unsafe fn enter(&self) -> std::io::Result<()> {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::syscall(
            libc::SYS_landlock_restrict_self,
            self.rules.as_raw_fd(),
            0u32,
        ) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        #[cfg(target_arch = "x86_64")]
        const ARCH: u32 = 0xc000003e;
        #[cfg(target_arch = "aarch64")]
        const ARCH: u32 = 0xc00000b7;
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        return Err(std::io::Error::from_raw_os_error(libc::ENOSYS));
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        {
            let ins = |code: u16, jt: u8, jf: u8, k: u32| libc::sock_filter { code, jt, jf, k };
            let reject = 0x00050000u32 | libc::EPERM as u32;
            let mut filter = [
                ins(0x20, 0, 0, 4), // seccomp_data.arch
                ins(0x15, 1, 0, ARCH),
                ins(0x06, 0, 0, reject),
                ins(0x20, 0, 0, 0), // syscall nr
                ins(0x45, 0, 1, 0x40000000),
                ins(0x06, 0, 0, reject), // reject x32 ABI
                ins(0x15, 0, 1, libc::SYS_connect as u32),
                ins(0x06, 0, 0, reject),
                ins(0x15, 0, 3, libc::SYS_socket as u32),
                ins(0x20, 0, 0, 16),
                ins(0x15, 1, 0, libc::AF_UNIX as u32),
                ins(0x06, 0, 0, reject),
                ins(0x06, 0, 0, 0x7fff0000),
            ];
            let program = libc::sock_fprog {
                len: filter.len() as u16,
                filter: filter.as_mut_ptr(),
            };
            if libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    #[test]
    #[ignore = "kernel confinement child; only private synthetic paths and denied sockets"]
    fn confined_child_cannot_read_outside_fixture_or_connect() {
        let Some(base) = std::env::var_os("SONANTE_CONFINEMENT_TEST") else {
            return;
        };
        let base = PathBuf::from(base);
        let allowed = base.join("allowed");
        let dir = crate::recovery_fs::Dir::open(&allowed).unwrap();
        let sandbox = Sandbox::prepare(&dir, &std::env::current_exe().unwrap()).unwrap();
        unsafe {
            sandbox.enter().unwrap();
        }
        assert_eq!(std::fs::read(allowed.join("inside")).unwrap(), b"inside");
        assert_eq!(
            std::fs::read(base.join("outside")).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let tcp = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert_eq!(tcp, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EPERM)
        );
        let unix = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        assert!(unix >= 0);
        let address = unsafe { std::mem::zeroed::<libc::sockaddr_un>() };
        let result = unsafe {
            libc::connect(
                unix,
                &address as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_un>() as u32,
            )
        };
        assert_eq!(result, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EPERM)
        );
        unsafe {
            libc::close(unix);
        }
    }
    #[test]
    #[ignore = "spawn a separately confined test child; no personal files or real connections"]
    fn kernel_rules_confine_only_the_validation_child() {
        use std::os::unix::fs::DirBuilderExt;
        let base = std::env::temp_dir().join(format!("sonante-confinement-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&base)
            .unwrap();
        let root = crate::recovery_fs::Dir::open(&base).unwrap();
        let allowed = root.create("allowed").unwrap();
        allowed.write_new("inside", b"inside").unwrap();
        root.write_new("outside", b"outside").unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("recovery_sandbox::tests::confined_child_cannot_read_outside_fixture_or_connect")
            .arg("--ignored")
            .env("SONANTE_CONFINEMENT_TEST", &base)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{} {}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert_eq!(std::fs::read(base.join("outside")).unwrap(), b"outside");
        std::fs::remove_dir_all(base).unwrap();
    }
}
