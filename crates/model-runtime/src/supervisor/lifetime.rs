use std::io;
use std::process::Command;
use std::thread;

const PARENT_FD: &str = "MODEL_RUNTIME_PARENT_FD";
const PARENT_PID: &str = "MODEL_RUNTIME_PARENT_PID";
const PARENT_CREATED: &str = "MODEL_RUNTIME_PARENT_CREATED";

#[cfg(unix)]
pub(super) struct ParentLifetime {
    _parent: Option<std::os::unix::net::UnixStream>,
    inherited: Option<std::os::unix::net::UnixStream>,
}

#[cfg(unix)]
impl ParentLifetime {
    pub(super) fn configure(command: &mut Command, enabled: bool) -> io::Result<Self> {
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixStream;
        use std::os::unix::process::CommandExt;
        command
            .env_remove(PARENT_FD)
            .env_remove(PARENT_PID)
            .env_remove(PARENT_CREATED);
        if !enabled {
            return Ok(Self {
                _parent: None,
                inherited: None,
            });
        }
        let (parent, inherited) = UnixStream::pair()?;
        let fd = inherited.as_raw_fd();
        command.env(PARENT_FD, fd.to_string());
        // Only the child's read end survives exec. Every parent's write end
        // remains CLOEXEC, including when other workers spawn concurrently.
        // No PDEATHSIG: Linux binds it to the spawning thread, not the process.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(Self {
            _parent: Some(parent),
            inherited: Some(inherited),
        })
    }

    pub(super) fn spawned(&mut self) {
        self.inherited.take();
    }
}

/// Start before loading kernels. An inherited lifetime channel is independent
/// of stdin (which may be blocked by inference) and cannot be fooled by PID
/// reuse. With no supervisor lifetime environment this is a no-op.
#[cfg(unix)]
pub fn start_parent_watchdog() -> io::Result<()> {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    use std::os::unix::net::UnixStream;
    let Some(value) = std::env::var_os(PARENT_FD) else {
        return Ok(());
    };
    let fd: libc::c_int = value
        .to_str()
        .and_then(|v| v.parse().ok())
        .filter(|fd| *fd > 2)
        .ok_or_else(|| io::Error::other("invalid worker parent lifetime descriptor"))?;
    // SAFETY: the supervisor passes ownership of this inherited descriptor to
    // the child. Mark it CLOEXEC again before the host could spawn descendants.
    let mut stream = unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
        UnixStream::from_raw_fd(fd)
    };
    std::env::remove_var(PARENT_FD);
    thread::Builder::new()
        .name("model-worker-parent".into())
        .spawn(move || {
            let mut byte = [0];
            loop {
                match stream.read(&mut byte) {
                    Ok(0) => exit_without_native_cleanup(3),
                    Ok(_) => continue,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => exit_without_native_cleanup(3),
                }
            }
        })?;
    Ok(())
}

/// Only for disposable worker processes after IPC flush; it skips vendor
/// destructors. Hosts must never use this in a process owning durable data.
#[cfg(unix)]
pub fn exit_without_native_cleanup(code: i32) -> ! {
    // SAFETY: the caller deliberately terminates this disposable process.
    unsafe { libc::_exit(code) }
}

#[cfg(windows)]
pub(super) struct ParentLifetime;

#[cfg(windows)]
impl ParentLifetime {
    pub(super) fn configure(command: &mut Command, enabled: bool) -> io::Result<Self> {
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        command
            .env_remove(PARENT_FD)
            .env_remove(PARENT_PID)
            .env_remove(PARENT_CREATED);
        if enabled {
            // SAFETY: the pseudo-handle identifies our live parent process.
            let created = creation_time(unsafe { GetCurrentProcess() })?;
            command
                .env(PARENT_PID, std::process::id().to_string())
                .env(PARENT_CREATED, created.to_string());
        }
        Ok(Self)
    }
    pub(super) fn spawned(&mut self) {}
}

#[cfg(windows)]
fn creation_time(handle: windows_sys::Win32::Foundation::HANDLE) -> io::Result<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: output pointers reference writable FILETIME structures.
    if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

#[cfg(windows)]
struct Handle(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: Windows process/job handles may be shared across threads; this type
// uniquely closes the handle only after all owning structures are dropped.
#[cfg(windows)]
unsafe impl Send for Handle {}
#[cfg(windows)]
unsafe impl Sync for Handle {}

#[cfg(windows)]
impl Drop for Handle {
    fn drop(&mut self) {
        if unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) } == 0 {
            tracing::warn!(error = %io::Error::last_os_error(), "cannot close worker process handle");
        }
    }
}

#[cfg(windows)]
pub fn start_parent_watchdog() -> io::Result<()> {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, INFINITE, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE,
    };
    let Some(pid) = std::env::var_os(PARENT_PID) else {
        return Ok(());
    };
    let pid = pid
        .to_str()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| io::Error::other("invalid parent PID"))?;
    let created = std::env::var(PARENT_CREATED)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| io::Error::other("missing parent process identity"))?;
    // Open once, then wait on the process object, never a reusable PID. The
    // creation timestamp also closes the race before OpenProcess succeeds.
    let raw = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let handle = Handle(raw);
    if creation_time(handle.0)? != created {
        return Err(io::Error::other("worker parent identity changed"));
    }
    thread::Builder::new()
        .name("model-worker-parent".into())
        .spawn(move || {
            wait_for_parent(handle);
        })?;
    fn wait_for_parent(handle: Handle) {
        unsafe {
            WaitForSingleObject(handle.0, INFINITE);
        }
        exit_without_native_cleanup(3);
    }
    Ok(())
}

#[cfg(windows)]
pub fn exit_without_native_cleanup(code: i32) -> ! {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    // ExitProcess runs DLL detach callbacks; those are precisely the unstable
    // native teardown paths this disposable process boundary must avoid.
    unsafe {
        TerminateProcess(GetCurrentProcess(), code as u32);
    }
    std::process::abort()
}

#[cfg(windows)]
pub(super) struct WindowsJob {
    _handle: Handle,
}

#[cfg(windows)]
impl WindowsJob {
    pub(super) fn assign(
        child: &std::process::Child,
        memory_limit: Option<u64>,
    ) -> io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        };
        let memory_limit = memory_limit
            .map(|limit| {
                usize::try_from(limit)
                    .ok()
                    .filter(|limit| *limit > 0)
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "native worker memory limit is not representable",
                        )
                    })
            })
            .transpose()?;
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = Handle(raw);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(limit) = memory_limit {
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            limits.ProcessMemoryLimit = limit;
        }
        if unsafe {
            SetInformationJobObject(
                handle.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if unsafe { AssignProcessToJobObject(handle.0, child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { _handle: handle })
    }
}

#[cfg(not(any(unix, windows)))]
pub(super) struct ParentLifetime;

#[cfg(not(any(unix, windows)))]
impl ParentLifetime {
    pub(super) fn configure(_: &mut Command, enabled: bool) -> io::Result<Self> {
        if enabled {
            return Err(io::Error::other(
                "parent lifetime monitoring is unsupported on this platform",
            ));
        }
        Ok(Self)
    }
    pub(super) fn spawned(&mut self) {}
}

#[cfg(not(any(unix, windows)))]
pub fn start_parent_watchdog() -> io::Result<()> {
    Err(io::Error::other(
        "parent lifetime monitoring is unsupported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
pub fn exit_without_native_cleanup(code: i32) -> ! {
    std::process::exit(code)
}
