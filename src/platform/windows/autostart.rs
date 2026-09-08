//! Auto-start (run at login) registration on Windows.
//!
//! Writes a `Run` value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` using the built-in
//! `reg.exe`, so no extra registry crate is pulled in. The value points at the
//! current executable and is rewritten whenever the toggle changes, so moving a
//! portable build updates the launch target to the new location.

use anyhow::{bail, Context, Result};

/// Full per-user run key. `reg.exe` rejects a key without its hive root
/// (`HKCU\`), which would make every query/set fail silently.
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "TokenMonitor";

/// Whether a `Run` value named `TokenMonitor` currently exists.
pub fn autostart_enabled() -> bool {
    std::process::Command::new("reg.exe")
        .arg("query")
        .arg(RUN_KEY)
        .arg("/v")
        .arg(VALUE_NAME)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Enable or disable auto-start by adding or removing the `Run` value.
pub fn set_autostart(enabled: bool) -> Result<()> {
    if enabled {
        let exe = std::env::current_exe().context("resolve current exe")?;
        let value = format!("\"{}\"", exe.display());
        let output = std::process::Command::new("reg.exe")
            .arg("add")
            .arg(RUN_KEY)
            .arg("/v")
            .arg(VALUE_NAME)
            .arg("/t")
            .arg("REG_SZ")
            .arg("/d")
            .arg(&value)
            .arg("/f")
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
        let output = std::process::Command::new("reg.exe")
            .arg("delete")
            .arg(RUN_KEY)
            .arg("/v")
            .arg(VALUE_NAME)
            .arg("/f")
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
