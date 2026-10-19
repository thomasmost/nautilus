//! Process-level housekeeping for the browser process.

use std::io;

use url::Url;

/// Start another `nautilus` for `profile` with `urls` as its tabs (the
/// "Open in profile" tab menu). Another profile is another process: a
/// Chromium cache directory belongs to one browser process at a time. The
/// new browser is independent of this
/// one: on Windows it leaves the job object that ties content processes
/// to this browser (see [`bind_children_to_this_process`]).
pub fn open_in_profile(profile: &str, urls: &[Url]) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command.arg("--profile").arg(profile);
    for url in urls {
        command.arg(url.as_str());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_BREAKAWAY_FROM_JOB;
        command.creation_flags(CREATE_BREAKAWAY_FROM_JOB);
    }
    let mut child = command.spawn()?;
    // Reap it when it exits so it never lingers as a zombie on Unix.
    std::thread::Builder::new()
        .name("ProfileReaper".to_string())
        .spawn(move || {
            let _ = child.wait();
        })?;
    Ok(())
}

/// Make every process this one starts die with it.
///
/// Chromium's renderer, GPU, network, and utility processes are children
/// of the browser process (see `nautilus_engine::run_subprocess`). A clean
/// exit stops them through the engine, but a browser that is killed or
/// crashes natively would leave them running. On Windows this puts the
/// browser into a job object with `KILL_ON_JOB_CLOSE`; children join the
/// job automatically and the kernel ends them when the browser's handle to
/// the job goes away, which is when the browser process ends for any
/// reason. The job handle is never closed on purpose. On macOS Chromium's
/// helper processes watch their parent themselves.
///
/// Call it before the engine opens the first tab. Failure (for example a
/// parent job that forbids nesting) is logged and the browser runs on
/// without the guarantee.
#[cfg(windows)]
pub fn bind_children_to_this_process() {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: plain Win32 calls with valid arguments; `info` is a fully
    // initialised, correctly sized struct, and the job handle is leaked
    // deliberately so the job outlives this function.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            log::warn!(
                "cannot create the job object for content processes (error {})",
                GetLastError()
            );
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        // BREAKAWAY_OK lets a browser started for another profile
        // (`open_in_profile`) opt out; content processes stay in.
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast::<c_void>(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            log::warn!(
                "cannot configure the job object for content processes (error {})",
                GetLastError()
            );
            return;
        }
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            log::warn!(
                "cannot join the job object for content processes (error {}); \
                 content processes will outlive a killed browser",
                GetLastError()
            );
        }
    }
}

#[cfg(not(windows))]
pub fn bind_children_to_this_process() {}
