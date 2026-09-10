//! Auto-start (run at login) registration on Windows.
//!
//! Writes a `Run` value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` using the built-in
//! `reg.exe`, so no extra registry crate is pulled in. The value points at the
//! current executable and is rewritten whenever the toggle changes, so moving a
//! portable build updates the launch target to the new location.

use std::os::windows::process::CommandExt as _;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

/// Full per-user run key. `reg.exe` rejects a key without its hive root
/// (`HKCU\`), which would make every query/set fail silently.
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "TokenMonitor";

/// `reg.exe` is a console-subsystem program; TokenMonitor is a GUI-subsystem
/// app, so spawning `reg.exe` without this flag makes Windows allocate a fresh
/// console window that flashes on screen for every query and write.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `reg.exe` invocation that never opens a console window and never reads
/// stdin. stdout is discarded; stderr is captured for error reporting.
fn reg<I, S>(args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new("reg.exe");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    command
}

/// Whether a `Run` value named `TokenMonitor` currently exists.
pub fn autostart_enabled() -> bool {
    let mut command = reg(["query", RUN_KEY, "/v", VALUE_NAME]);
    command
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Enable or disable auto-start by adding or removing the `Run` value.
pub fn set_autostart(enabled: bool) -> Result<()> {
    if enabled {
        let exe = std::env::current_exe().context("resolve current exe")?;
        let value = format!("\"{}\"", exe.display());
        let output = reg([
            "add",
            RUN_KEY,
            "/v",
            VALUE_NAME,
            "/t",
            "REG_SZ",
            "/d",
            value.as_str(),
            "/f",
        ])
        .output()
        .context("run reg.exe to add auto-start")?;
        if !output.status.success() {
            bail!(
                "reg.exe add failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    } else {
        // Already absent => disable is a no-op.
        if !autostart_enabled() {
            return Ok(());
        }
        let output = reg(["delete", RUN_KEY, "/v", VALUE_NAME, "/f"])
            .output()
            .context("run reg.exe to remove auto-start")?;
        if !output.status.success() {
            bail!(
                "reg.exe delete failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    Ok(())
}
