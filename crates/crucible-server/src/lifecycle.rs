//! Local process lifecycle helpers for replacing an already-running server.
//!
//! `cargo run -p crucible-server -- start` is intended to be a convenient
//! development command. Before binding the configured address it stops a
//! Crucible server from this checkout, if one is already running, so stale
//! `cargo run` sessions do not leave the next launch with an "address already
//! in use" error.

#[cfg(unix)]
use std::collections::BTreeSet;
use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process,
};
#[cfg(unix)]
use std::{
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);
#[cfg(unix)]
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Stop existing local Crucible server processes associated with this checkout.
///
/// On Unix, `/proc` lets us verify both the executable name and its working
/// directory before sending a signal. The PID file handles servers launched by
/// this version of the binary; the process scan also catches an older binary
/// that was started before PID files were introduced.
#[cfg(unix)]
pub(crate) fn replace_existing_server(addr: SocketAddr) {
    let pid_path = pid_file_path(addr);
    let mut candidates = BTreeSet::new();
    if let Some(pid) = read_pid_file(&pid_path) {
        candidates.insert(pid);
    }
    candidates.extend(local_server_pids(addr));

    let current_pid = process::id();
    for pid in candidates {
        if pid == current_pid || !is_local_server(pid) {
            continue;
        }
        tracing::info!(pid, "stopping existing local Crucible server");
        if !terminate(pid) {
            tracing::warn!(pid, "could not stop existing local Crucible server");
        }
    }

    // A stale file should not affect the server that is about to claim the
    // address. It is rewritten only after the new listener binds successfully.
    remove_pid_file(&pid_path);
}

/// Windows cannot enumerate `/proc`; the PID file is still sufficient for
/// servers started by this lifecycle-aware binary.
#[cfg(not(unix))]
pub(crate) fn replace_existing_server(addr: SocketAddr) {
    let pid_path = pid_file_path(addr);
    if let Some(pid) = read_pid_file(&pid_path) {
        if pid != process::id() {
            tracing::info!(pid, "stopping existing local Crucible server");
            if !terminate(pid) {
                tracing::warn!(pid, "could not stop existing local Crucible server");
            }
        }
    }
    remove_pid_file(&pid_path);
}

/// Record the running server's PID after its listener has bound successfully.
pub(crate) fn write_pid_file(addr: SocketAddr) -> Option<PathBuf> {
    let path = pid_file_path(addr);
    match fs::write(&path, format!("{}\n", process::id())) {
        Ok(()) => Some(path),
        Err(error) => {
            tracing::warn!(?error, path = %path.display(), "could not write server PID file");
            None
        }
    }
}

pub(crate) fn remove_pid_file(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::debug!(?error, path = %path.display(), "could not remove server PID file");
        }
    }
}

fn pid_file_path(addr: SocketAddr) -> PathBuf {
    // Include the address so a deliberate second server on another port does
    // not replace the first one.
    let host = addr.ip().to_string().replace([':', '%'], "_");
    std::env::temp_dir().join(format!("crucible-server-{host}-{}.pid", addr.port()))
}

fn read_pid_file(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(unix)]
fn local_server_pids(addr: SocketAddr) -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| is_local_server(pid) && process_port(pid) == Some(addr.port()))
        .collect()
}

#[cfg(unix)]
fn is_local_server(pid: u32) -> bool {
    let exe = fs::read_link(format!("/proc/{pid}/exe")).ok();
    let is_server_exe = exe
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "crucible-server" || name == "crucible-server.exe");
    if !is_server_exe {
        return false;
    }

    let Ok(root) = std::env::current_dir().and_then(fs::canonicalize) else {
        return false;
    };
    let Ok(cwd) = fs::read_link(format!("/proc/{pid}/cwd")) else {
        return false;
    };
    cwd == root || cwd.starts_with(root)
}

#[cfg(unix)]
fn process_port(pid: u32) -> Option<u16> {
    let environment = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let configured = environment
        .split(|&byte| byte == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok())
        .find_map(|entry| entry.strip_prefix("CRUCIBLE_ADDR="));
    match configured {
        Some(value) => value
            .parse::<SocketAddr>()
            .ok()
            .map(|address| address.port()),
        // An omitted CRUCIBLE_ADDR means the server's documented default.
        None => Some(8787),
    }
}

#[cfg(unix)]
fn terminate(pid: u32) -> bool {
    let raw_pid = pid as libc::pid_t;
    let result = unsafe { libc::kill(raw_pid, libc::SIGTERM) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        // The process already exited between discovery and signaling.
        if error.raw_os_error() == Some(libc::ESRCH) {
            return true;
        }
        tracing::debug!(pid, ?error, "SIGTERM failed");
        return false;
    }

    if wait_for_exit(raw_pid, SHUTDOWN_WAIT) {
        return true;
    }

    // A stale/debug process should not make every future `start` fail. Give it
    // a graceful window first, then force termination as a last resort.
    tracing::warn!(
        pid,
        "existing server did not stop gracefully; forcing termination"
    );
    let result = unsafe { libc::kill(raw_pid, libc::SIGKILL) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return true;
        }
        tracing::debug!(pid, ?error, "SIGKILL failed");
        return false;
    }
    wait_for_exit(raw_pid, SHUTDOWN_WAIT)
}

#[cfg(not(unix))]
fn terminate(pid: u32) -> bool {
    let Ok(status) = process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T"])
        .status()
    else {
        return false;
    };
    status.success()
}

#[cfg(unix)]
fn wait_for_exit(pid: libc::pid_t, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while process_alive(pid) && Instant::now() < deadline {
        thread::sleep(POLL_INTERVAL);
    }
    !process_alive(pid)
}

#[cfg(unix)]
fn process_alive(pid: libc::pid_t) -> bool {
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(test)]
mod tests {
    use super::pid_file_path;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn pid_file_is_unique_per_port() {
        let a = pid_file_path(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8787));
        let b = pid_file_path(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8788));
        assert_ne!(a, b);
        assert!(a.to_string_lossy().contains("8787"));
    }
}
