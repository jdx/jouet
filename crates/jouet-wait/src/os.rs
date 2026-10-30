//! The two questions the standard library cannot answer portably: is a
//! process that is not our child still running, and is the file at a path
//! still the same file (for following a log across rotation).

use std::fs::File;

/// Whether a process with this ID exists and has not exited.
#[cfg(unix)]
pub(crate) fn process_running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // `kill(0, ...)` would address our own process group.
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 sends nothing; it only checks that the process
    // exists and that we may signal it.
    let found = unsafe { libc::kill(pid, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    found && !zombie(pid)
}

/// A process that has exited but not been reaped by its parent still
/// answers `kill`. Linux says so in `/proc`; elsewhere it counts as running
/// until its parent reaps it.
#[cfg(target_os = "linux")]
fn zombie(pid: libc::pid_t) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            // The command name is in parentheses and may contain spaces; the
            // state is the field after the last `)`.
            let (_, rest) = stat.rsplit_once(')')?;
            rest.split_whitespace().next().map(|s| s == "Z")
        })
        .unwrap_or(false)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn zombie(_pid: libc::pid_t) -> bool {
    false
}

#[cfg(windows)]
pub(crate) fn process_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, GetLastError, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: plain Win32 calls; the handle is checked before use and
    // closed afterwards.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            // A process we may not open still exists.
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        // An exited process stays openable while anyone holds a handle to
        // it, but its handle is signalled.
        let running = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
        CloseHandle(handle);
        running
    }
}

/// The device and file number of an open file, which change when a log is
/// rotated even if the new file has the same name.
#[cfg(unix)]
pub(crate) fn file_id(file: &File) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata().ok()?;
    Some((meta.dev(), meta.ino()))
}

#[cfg(windows)]
pub(crate) fn file_id(file: &File) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle belongs to `file`, which outlives the call, and
    // `info` is a valid place for the result.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0;
    ok.then(|| {
        let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        (u64::from(info.dwVolumeSerialNumber), index)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_running() {
        assert!(process_running(std::process::id()));
    }

    #[test]
    fn a_reaped_child_is_not_running() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!process_running(pid));
    }

    #[test]
    fn a_renamed_file_is_a_different_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, "one").unwrap();
        let first = file_id(&File::open(&path).unwrap()).unwrap();
        assert_eq!(file_id(&File::open(&path).unwrap()).unwrap(), first);
        std::fs::rename(&path, dir.path().join("a.log.1")).unwrap();
        std::fs::write(&path, "two").unwrap();
        assert_ne!(file_id(&File::open(&path).unwrap()).unwrap(), first);
    }
}
