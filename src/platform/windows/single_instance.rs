//! Single-instance guard for the desktop app.
//!
//! Launching TokenMonitor again — a double-clicked shortcut, the auto-start
//! entry, a taskbar pin — must not open a second window or add a second tray
//! icon; the running copy should be raised instead.
//!
//! The guard is a per-session named mutex: whoever creates it first is the
//! instance that keeps running. Later launches see `ERROR_ALREADY_EXISTS`,
//! signal a named event, and exit. The running copy waits on that event from a
//! small helper thread and turns each signal into a posted window message, which
//! the tray hook in `tray.rs` converts into "show / restore".
//!
//! The event is used instead of a `HWND_BROADCAST` message because a broadcast
//! reaches top-level windows only: it would be silently dropped in the (very
//! common) case where the first copy has not created its window yet.

use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Result};

/// What `CreateMutexW` reports when the named mutex already exists, i.e. when
/// another copy of TokenMonitor owns it.
const ERROR_ALREADY_EXISTS: u32 = 183;

/// Per-session objects (`Local\`): every Windows session has its own taskbar and
/// system tray, so one instance per session is the right granularity — and the
/// `Local\` namespace needs no `SeCreateGlobalPrivilege`.
const MUTEX_NAME: &str = "Local\\TokenMonitor.SingleInstance";
const ACTIVATE_EVENT_NAME: &str = "Local\\TokenMonitor.Activate";
/// Registered window message (never a fixed id, so it cannot collide with a
/// system message and is identical in both processes).
const ACTIVATE_MESSAGE_NAME: &str = "TokenMonitor.ActivateWindow";

const EVENT_MODIFY_STATE: u32 = 0x0002;
const SYNCHRONIZE: u32 = 0x0010_0000;
const INFINITE: u32 = u32::MAX;
/// How long a later launch keeps retrying to reach the running copy: the event
/// is created microseconds after the mutex, but the first launch may still be
/// starting GPUI up when a user double-clicks the icon a second time.
const ACTIVATE_RETRIES: u32 = 20;
const ACTIVATE_RETRY_DELAY: Duration = Duration::from_millis(50);

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(attrs: *const c_void, initial_owner: i32, name: *const u16) -> isize;
    fn CreateEventW(
        attrs: *const c_void,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> isize;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
    fn SetEvent(handle: isize) -> i32;
    fn WaitForSingleObject(handle: isize, millis: u32) -> u32;
    fn GetLastError() -> u32;
    fn CloseHandle(handle: isize) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
}

/// Keeps the mutex handle open for the process lifetime. The OS releases the
/// guard when the process exits, which is exactly when a later launch may become
/// the first instance again.
static MUTEX_HANDLE: OnceLock<isize> = OnceLock::new();
/// Event the running copy waits on; created by the first instance and signalled
/// by every later launch.
static ACTIVATE_EVENT: OnceLock<isize> = OnceLock::new();
/// Id of the "show / restore" message, registered lazily (0 when registration
/// failed, in which case activation messages are simply not handled).
static ACTIVATE_MESSAGE: OnceLock<u32> = OnceLock::new();
static LISTENING: AtomicBool = AtomicBool::new(false);

/// Take the single-instance guard.
///
/// `true` means this process is the only instance and should continue starting
/// up; `false` means another copy owns the guard and the caller should hand off
/// with [`activate_running_instance`] and exit.
pub fn acquire_single_instance() -> bool {
    match create_mutex(MUTEX_NAME) {
        Ok(Some(handle)) => {
            let _ = MUTEX_HANDLE.set(handle);
            create_activate_event();
            true
        }
        Ok(None) => false,
        // The guard itself could not be created (out of handles, denied): run
        // rather than refuse to start.
        Err(_) => true,
    }
}

/// Ask the already-running copy to show its window.
///
/// No-op if it cannot be reached — the caller exits either way, since the whole
/// point of the guard is that only one copy runs.
pub fn activate_running_instance() {
    signal_event(ACTIVATE_EVENT_NAME, ACTIVATE_RETRIES);
}

/// The window message that means "show the main window", registered once.
///
/// The tray's window-proc hook matches incoming messages against this id.
pub(super) fn activate_message() -> u32 {
    *ACTIVATE_MESSAGE
        .get_or_init(|| unsafe { RegisterWindowMessageW(wide(ACTIVATE_MESSAGE_NAME).as_ptr()) })
}

/// Forward activation signals into `main_hwnd` as posted messages.
///
/// Call once, on the main thread, after the main window exists (the tray hook
/// does). The helper thread lives for the process lifetime: it blocks on the
/// event and posts, so the window is raised by the GPUI message loop rather than
/// by cross-process window calls.
pub(super) fn listen_for_activation(main_hwnd: isize) {
    let Some(&event) = ACTIVATE_EVENT.get() else {
        return; // guard not taken (or event creation failed)
    };
    if LISTENING.swap(true, Relaxed) {
        return;
    }
    let message = activate_message();
    if message == 0 {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("tokenmonitor-single-instance".into())
        .spawn(move || loop {
            let waited = unsafe { WaitForSingleObject(event, INFINITE) };
            if waited != 0 {
                return; // WAIT_FAILED: stop instead of spinning on a dead handle
            }
            unsafe {
                PostMessageW(main_hwnd, message, 0, 0);
            }
        });
}

/// Create the named mutex, or report that someone else already owns it.
fn create_mutex(name: &str) -> Result<Option<isize>> {
    let name = wide(name);
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle == 0 {
            return Err(anyhow!("CreateMutexW failed: {}", GetLastError()));
        }
        // `GetLastError` must be read immediately after the call.
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return Ok(None);
        }
        Ok(Some(handle))
    }
}

/// Create the auto-reset activation event. A signal that arrives before the
/// window exists stays pending until the listener thread consumes it, and a
/// signal is never queued twice — one raise per request is enough.
fn create_activate_event() {
    if let Some(handle) = create_event(ACTIVATE_EVENT_NAME) {
        let _ = ACTIVATE_EVENT.set(handle);
    }
}

/// Create (or open) the auto-reset event named `name`.
fn create_event(name: &str) -> Option<isize> {
    let name = wide(name);
    let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
    if handle != 0 {
        Some(handle)
    } else {
        None
    }
}

/// Signal the event named `name`, retrying while it does not exist yet (the
/// running copy creates it just after taking the mutex). `true` when the signal
/// reached a running copy.
fn signal_event(name: &str, retries: u32) -> bool {
    let name = wide(name);
    for attempt in 0..retries {
        if attempt > 0 {
            std::thread::sleep(ACTIVATE_RETRY_DELAY);
        }
        unsafe {
            let handle = OpenEventW(SYNCHRONIZE | EVENT_MODIFY_STATE, 0, name.as_ptr());
            if handle != 0 {
                SetEvent(handle);
                CloseHandle(handle);
                return true;
            }
        }
    }
    false
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A per-process name, so tests never collide with a TokenMonitor instance
    /// the developer happens to have running (or registered for auto-start).
    fn test_name(label: &str) -> String {
        format!("Local\\TokenMonitor.Test.{}.{label}", std::process::id())
    }

    /// Uses a per-process name so the test never collides with a TokenMonitor
    /// instance the developer happens to have running (or registered for
    /// auto-start).
    #[test]
    fn later_launch_does_not_become_the_first_instance() {
        let name = test_name("mutex");
        let first = create_mutex(&name).expect("create the guard");
        assert!(first.is_some(), "the first launch owns the guard");

        let second = create_mutex(&name).expect("re-open the guard");
        assert!(second.is_none(), "a later launch must not own the guard");

        for handle in first.into_iter().chain(second) {
            unsafe {
                CloseHandle(handle);
            }
        }
    }

    #[test]
    fn activation_signal_reaches_the_running_copy() {
        let name = test_name("event");
        let event = create_event(&name).expect("create the activation event");

        assert!(signal_event(&name, 1), "the signal should open the event");
        assert_eq!(
            unsafe { WaitForSingleObject(event, 0) },
            0,
            "the running copy must observe the signal"
        );
        unsafe {
            CloseHandle(event);
        }

        assert!(
            !signal_event(&test_name("missing"), 1),
            "an unreachable copy reports the hand-off as failed"
        );
    }

    #[test]
    fn activation_message_registers_once() {
        let first = activate_message();
        assert_ne!(first, 0, "the activation message should register");
        assert_eq!(first, activate_message(), "the id is cached");
    }
}
