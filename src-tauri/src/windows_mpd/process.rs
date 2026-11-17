//! Owned Windows process. The child is suspended until a kill-on-close job owns
//! it. No PID files, taskkill, services, or unverified process termination.
use super::protocol::{self, Result};
use std::mem::{offset_of, size_of, zeroed};
use std::net::TcpStream;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle, FromRawHandle};
use std::sync::Arc;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, GENERIC_READ, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT, SetHandleInformation, HANDLE_FLAG_INHERIT,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
};
use windows_sys::Win32::Networking::WinSock::AF_INET;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, ResumeThread, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

fn wide(path: &Path) -> Result<Vec<u16>> {
    let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
    if value.contains(&0) {
        return Err("invalid_argument");
    }
    value.push(0);
    Ok(value)
}

// Aligned backing storage owns the opaque attribute list until CreateProcessW
// returns. Only listed stdio and typed PCM duplicates can be inherited.
struct Attributes {
    storage: Vec<usize>,
    initialized: bool,
}
impl Attributes {
    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
    fn new(handles: &[HANDLE]) -> Result<Self> {
        let mut bytes = 0usize;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
        }
        if bytes == 0 || bytes > 1024 * 1024 {
            return Err("engine_stdio_failed");
        }
        let mut value = Self {
            storage: vec![0usize; bytes.div_ceil(size_of::<usize>())],
            initialized: false,
        };
        if unsafe { InitializeProcThreadAttributeList(value.pointer(), 1, 0, &mut bytes) } == 0 {
            // Initialization failed, so there is no list to delete.
            return Err("engine_stdio_failed");
        }
        value.initialized = true;
        if unsafe {
            UpdateProcThreadAttribute(
                value.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err("engine_stdio_failed");
        }
        Ok(value)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.pointer());
            }
        }
    }
}

fn stdio_file(path: &Path, access: u32, creation: u32, sharing: u32) -> Result<Handle> {
    let path = wide(path)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let handle = Handle(unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            sharing,
            &attributes,
            creation,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    });
    if handle.0 == INVALID_HANDLE_VALUE || handle.0.is_null() {
        return Err("engine_stdio_failed");
    }
    Ok(handle)
}

// Only the write end can be inherited. The reader is an owned, bounded
// discard worker; no MPD output (including private media URLs) goes to a file.
struct PipeOutput { writer: Handle, drain: super::stdio::Drain }
fn stdio_pipe() -> Result<PipeOutput> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(), bInheritHandle: 1,
    };
    let mut reader = null_mut(); let mut writer = null_mut();
    // Failure leaves output parameters indeterminate; never wrap/use them then.
    if unsafe { CreatePipe(&mut reader, &mut writer, &attributes, 65536) } == 0 {
        return Err("engine_stdio_failed");
    }
    let reader = Handle(reader); let writer = Handle(writer);
    if unsafe { SetHandleInformation(reader.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err("engine_stdio_failed");
    }
    let raw = reader.0;
    std::mem::forget(reader); // Ownership transfers once to File and the worker.
    let file = unsafe { std::fs::File::from_raw_handle(raw) };
    let drain = super::stdio::Drain::start(file)?;
    Ok(PipeOutput { writer, drain })
}

// Read-only witness keeps the exact process object alive across observation.
// It never opens a process by PID or owns the job/termination lifecycle.
pub struct Identity { handle: OwnedHandle, pub pid: u32 }
impl Identity {
    pub fn alive(&self) -> bool { unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) == WAIT_TIMEOUT } }
    pub fn verify_peer(&self,stream:&TcpStream)->Result<()> {
        verify_peer(self.pid, || Ok(self.alive()), || "engine_exited", stream)
    }
}

pub struct Process {
    process: Handle,
    job: Handle,
    pid: u32,
    identity: Option<Arc<Identity>>,
    // Drop order: process, Job (kills remaining writers), then discard reader.
    output: super::stdio::Drain,
}
impl Process {
    pub fn start(executable: &Path, config: &Path, directory: &Path) -> Result<Self> {
        Self::start_owned(executable, &["--no-daemon", config.to_str().ok_or("unsupported_path")?], directory, None)
    }

    // Internal only: callers hold verified runtime locks and typed PCM duplicates.
    pub fn start_owned(executable: &Path, arguments: &[&str], directory: &Path, pcm: Option<&crate::windows_dsp::ring::ChildHandles>) -> Result<Self> {
        let executable_wide = wide(executable)?;
        let directory_wide = wide(directory)?;
        let args = std::iter::once(executable.to_str().ok_or("unsupported_path")?).chain(arguments.iter().copied());
        let command = args
            .into_iter()
            .map(protocol::windows_argument)
            .collect::<Result<Vec<_>>>()?
            .join(" ");
        let mut command_wide: Vec<u16> = command.encode_utf16().chain([0]).collect();
        let input = stdio_file(
            Path::new("NUL"),
            GENERIC_READ,
            OPEN_EXISTING,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )?;
        let output = stdio_pipe()?;
        let mut inherited = vec![input.0, output.writer.0];
        if let Some(pcm) = pcm { inherited.extend_from_slice(&pcm.raw()); }
        let mut attributes = Attributes::new(&inherited)?;
        // A GUI parent has no usable console streams. Supply valid stdin and
        // stdout/stderr, while excluding the job, sockets, media and runtime
        // handles from inheritance. All buffers remain live through creation.
        unsafe {
            let job = Handle(CreateJobObjectW(null(), null()));
            if job.0.is_null() {
                return Err("job_failed");
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err("job_failed");
            }
            let mut startup: STARTUPINFOEXW = zeroed();
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = input.0;
            startup.StartupInfo.hStdOutput = output.writer.0;
            startup.StartupInfo.hStdError = output.writer.0;
            startup.lpAttributeList = attributes.pointer();
            let mut info: PROCESS_INFORMATION = zeroed();
            if CreateProcessW(
                executable_wide.as_ptr(),
                command_wide.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NO_WINDOW | CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT,
                null(),
                directory_wide.as_ptr(),
                &startup.StartupInfo,
                &mut info,
            ) == 0
            {
                return Err("engine_start_failed");
            }
            let process = Handle(info.hProcess);
            let thread = Handle(info.hThread);
            // The child now owns its inherited writer; keeping the parent copy
            // would prevent EOF after the owned Job/process has ended.
            drop(output.writer);
            if AssignProcessToJobObject(job.0, process.0) == 0 {
                // Still suspended: no engine code, sockets or audio ran.
                let terminated = TerminateProcess(process.0, 1) != 0;
                let waited = WaitForSingleObject(process.0, 2000) == WAIT_OBJECT_0;
                if !terminated || !waited {
                    return Err("shutdown_failed");
                }
                return Err("job_failed");
            }
            // Optional metadata proof: failure never changes successful engine
            // creation. This duplicate is not inheritable and grants no IPC authority.
            let identity = BorrowedHandle::borrow_raw(process.0).try_clone_to_owned()
                .map(|handle| Arc::new(Identity { handle, pid: info.dwProcessId }))
                .map_err(|_| eprintln!("Windows output observation: identity_unavailable")).ok();
            let mut child = Self {
                process,
                job,
                pid: info.dwProcessId,
                identity,
                output: output.drain,
            };
            if ResumeThread(thread.0) == u32::MAX {
                child.terminate()?;
                return Err("engine_start_failed");
            }
            Ok(child)
        }
    }

    pub fn witness(&self) -> Result<crate::windows_dsp::ring::Witness> {
        crate::windows_dsp::ring::Witness::duplicate(self.process.0, self.job.0)
    }
    pub fn identity(&self) -> Option<Arc<Identity>> { self.identity.clone() }

    pub fn check_stdio(&self) -> Result<()> { self.output.check() }
    pub(crate) fn stdio_failure_signal(&self)->Arc<std::sync::atomic::AtomicBool>{self.output.failure_signal()}

    pub fn exit_reason(&self) -> &'static str {
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(self.process.0, &mut code) } == 0 {
            return "engine_state_failed";
        }
        match code {
            0xC0000135 => "engine_dependency_missing",
            0xC0000139 => "engine_system_incompatible",
            0xC000007B => "engine_image_invalid",
            0xC000001D => "engine_cpu_incompatible",
            0xC0000005 => "engine_crashed",
            _ => "engine_exited",
        }
    }

    pub fn alive(&self) -> Result<bool> {
        match unsafe { WaitForSingleObject(self.process.0, 0) } {
            WAIT_TIMEOUT => Ok(true),
            WAIT_OBJECT_0 => Ok(false),
            _ => Err("engine_state_failed"),
        }
    }

    pub fn verify_peer(&self, stream: &TcpStream) -> Result<()> {
        verify_peer(self.pid, || self.alive(), || self.exit_reason(), stream)
    }

    pub fn terminate(&mut self) -> Result<()> {
        if !self.alive()? && super::job_scope::owned(self.process.0,self.job.0,std::time::Duration::ZERO).is_ok() {
            return Ok(());
        }
        let signalled = unsafe { TerminateJobObject(self.job.0, 0) } != 0;
        let waited = super::job_scope::owned(self.process.0,self.job.0,std::time::Duration::from_secs(2)).is_ok();
        if signalled && waited {
            return Ok(());
        }
        // Exact owned handle, never a PID lookup. Closing the job is an
        // additional kernel-enforced fallback on return/drop/process exit.
        let forced = unsafe { TerminateProcess(self.process.0, 1) } != 0;
        let waited = super::job_scope::owned(self.process.0,self.job.0,std::time::Duration::from_secs(2)).is_ok();
        if forced && waited {
            Ok(())
        } else {
            Err("shutdown_failed")
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Err(code) = self.terminate() {
            eprintln!("Windows MPD cleanup: {code}");
        }
    }
}

fn verify_peer(pid:u32, alive:impl Fn()->Result<bool>, dead:impl Fn()->&'static str, stream:&TcpStream)->Result<()> {
        if !alive()? {
            return Err(dead());
        }
        let client = stream.local_addr().map_err(|_| "peer_unverified")?;
        let server = stream.peer_addr().map_err(|_| "peer_unverified")?;
        if !client.ip().is_loopback() || !server.ip().is_loopback() {
            return Err("peer_unverified");
        }
        let loopback = u32::from_ne_bytes([127, 0, 0, 1]);
        unsafe {
            let mut bytes = 0;
            let initial = GetExtendedTcpTable(
                null_mut(),
                &mut bytes,
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            );
            if initial != ERROR_INSUFFICIENT_BUFFER || !(4..=1024 * 1024).contains(&bytes) {
                return Err("peer_unverified");
            }
            let mut data = vec![0u8; bytes as usize];
            if GetExtendedTcpTable(
                data.as_mut_ptr() as _,
                &mut bytes,
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            ) != 0
            {
                return Err("peer_unverified");
            }
            // Byte buffers have no alignment guarantee; use unaligned reads and
            // bound every row against the actual returned buffer length.
            if bytes as usize > data.len() || bytes < 4 {
                return Err("peer_unverified");
            }
            let rows = std::ptr::read_unaligned(data.as_ptr() as *const u32) as usize;
            let start = offset_of!(MIB_TCPTABLE_OWNER_PID, table);
            if start
                + rows
                    .checked_mul(size_of::<MIB_TCPROW_OWNER_PID>())
                    .ok_or("peer_unverified")?
                > bytes as usize
            {
                return Err("peer_unverified");
            }
            for index in 0..rows {
                let row = std::ptr::read_unaligned(
                    data.as_ptr()
                        .add(start + index * size_of::<MIB_TCPROW_OWNER_PID>())
                        as *const MIB_TCPROW_OWNER_PID,
                );
                if row.dwState == 5
                    && row.dwOwningPid == pid
                    && row.dwLocalAddr == loopback
                    && row.dwRemoteAddr == loopback
                    && u16::from_be(row.dwLocalPort as u16) == server.port()
                    && u16::from_be(row.dwRemotePort as u16) == client.port()
                {
                    return if alive()? {
                        Ok(())
                    } else {
                        Err(dead())
                    };
                }
            }
        }
        Err("peer_unverified")
    }
