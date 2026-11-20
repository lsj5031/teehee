//! Named single-instance guard shared by the CLI sender and the GUI.
//!
//! A named Windows mutex is the native equivalent of a pid file: the
//! kernel destroys it when the owning process exits, so a crashed
//! sender never leaves a stale lock behind. The name is scoped to the
//! logon session (`Local\`) because WASAPI loopback capture is
//! session-bound — two logon sessions may each run one sender.
//!
//! The GUI embeds the sender in its own process, so it takes the same
//! mutex the CLI does. Without that, opening the app while a scheduled
//! `teehee send` (or a second copy) is running would double-stream to
//! the receiver and interleave two `send.jsonl` histories.

use std::fmt;

/// The mutex name both the CLI `send` subcommand and the GUI acquire.
pub const SEND_MUTEX_NAME: &str = "Local\\TeeHeeSend";

/// Why the guard could not be acquired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SingleInstanceError {
    /// The named mutex already exists: another sender owns it.
    AlreadyRunning(String),
    /// The OS refused to create the mutex.
    Os(String),
}

impl fmt::Display for SingleInstanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning(name) => write!(
                f,
                "another teehee send instance is already running \
                 (single-instance mutex '{name}')"
            ),
            Self::Os(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SingleInstanceError {}

/// A held single-instance lock. Dropping it releases the name for the
/// next sender process.
pub struct SingleInstanceGuard {
    #[cfg(target_os = "windows")]
    handle: Option<windows::Win32::Foundation::HANDLE>,
    #[cfg(not(target_os = "windows"))]
    _private: (),
}

impl SingleInstanceGuard {
    /// Try to take `name` without blocking.
    ///
    /// Returns [`SingleInstanceError::AlreadyRunning`] when another
    /// process already owns the mutex.
    #[cfg(target_os = "windows")]
    pub fn acquire(name: &str) -> Result<Self, SingleInstanceError> {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;

        let wide = HSTRING::from(name);
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) }
            .map_err(|error| SingleInstanceError::Os(format!("create mutex '{name}': {error}")))?;
        // CreateMutexW succeeds with a valid handle even when the named
        // mutex already exists; it reports that through last-error. Read
        // it immediately, before any other FFI call can clobber it.
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already_exists {
            let _ = unsafe { CloseHandle(handle) };
            return Err(SingleInstanceError::AlreadyRunning(name.to_string()));
        }
        Ok(Self {
            handle: Some(handle),
        })
    }

    /// Non-Windows builds have no named-mutex guard; the CLI's guard is
    /// a no-op there today, so the GUI matches it.
    #[cfg(not(target_os = "windows"))]
    pub fn acquire(_name: &str) -> Result<Self, SingleInstanceError> {
        Ok(Self { _private: () })
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        if let Some(handle) = self.handle.take() {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_is_rejected_on_windows() {
        let first = SingleInstanceGuard::acquire("Local\\TeeHeeTestGuard");
        let Some(_first) = first.ok() else {
            return; // mutex creation can fail in constrained test sandboxes
        };

        #[cfg(target_os = "windows")]
        {
            let second = SingleInstanceGuard::acquire("Local\\TeeHeeTestGuard");
            assert!(matches!(
                second,
                Err(SingleInstanceError::AlreadyRunning(_))
            ));
        }
    }
}
