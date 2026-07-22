//! Process-group helpers used by PTY lifecycle management.

use std::io;

#[cfg(unix)]
fn signal_process_group_id(process_group_id: u32, signal: libc::c_int) -> io::Result<bool> {
    let process_group_id = libc::pid_t::try_from(process_group_id)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "process group ID overflow"))?;
    let result = unsafe { libc::killpg(process_group_id, signal) };
    if result == -1 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(true)
}

/// Send `SIGINT` to a Unix process group.
#[cfg(unix)]
pub fn interrupt_process_group(process_group_id: u32) -> io::Result<()> {
    signal_process_group_id(process_group_id, libc::SIGINT).map(|_| ())
}

/// Report unsupported group interrupts on non-Unix platforms.
#[cfg(not(unix))]
pub fn interrupt_process_group(_process_group_id: u32) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "numeric process groups are not supported on this platform",
    ))
}

/// Send `SIGTERM` to a Unix process group.
#[cfg(unix)]
pub fn terminate_process_group(process_group_id: u32) -> io::Result<bool> {
    signal_process_group_id(process_group_id, libc::SIGTERM)
}

/// Report no numeric process group on non-Unix platforms.
#[cfg(not(unix))]
pub fn terminate_process_group(_process_group_id: u32) -> io::Result<bool> {
    Ok(false)
}

/// Send `SIGKILL` to a Unix process group.
#[cfg(unix)]
pub fn kill_process_group(process_group_id: u32) -> io::Result<()> {
    signal_process_group_id(process_group_id, libc::SIGKILL).map(|_| ())
}

/// Report no numeric process group on non-Unix platforms.
#[cfg(not(unix))]
pub fn kill_process_group(_process_group_id: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
/// Arrange for a child to receive `SIGTERM` if its original parent dies.
///
/// This helper is intended for use from a `pre_exec` closure. The caller must
/// capture the expected parent PID before spawning.
pub fn set_parent_death_signal(expected_parent_pid: libc::pid_t) -> io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::getppid() } != expected_parent_pid {
        unsafe {
            libc::raise(libc::SIGTERM);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
/// No-op parent-death setup outside Linux.
pub fn set_parent_death_signal(_expected_parent_pid: i32) -> io::Result<()> {
    Ok(())
}
