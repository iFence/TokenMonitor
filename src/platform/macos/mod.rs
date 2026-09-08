use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

pub fn app_data_dir() -> Result<PathBuf> {
    Err(anyhow!("app_data_dir not implemented on macOS yet"))
}

pub fn legacy_data_dir() -> Result<PathBuf> {
    Err(anyhow!("legacy_data_dir not implemented on macOS yet"))
}

pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow!("no home directory"))
}

pub fn is_portable() -> bool {
    false
}

pub fn open_path_in_explorer(_path: &Path) -> Result<()> {
    Err(anyhow!(
        "open_path_in_explorer not implemented on macOS yet"
    ))
}

pub fn launch_installer(_path: &Path) -> Result<()> {
    Err(anyhow!("launch_installer not implemented on macOS yet"))
}

pub fn apply_dark_titlebar() {}

/// No system tray yet on macOS; no-op to keep the surface uniform.
pub fn close_window() {}

/// Path of the LaunchAgent plist, if a home directory is known.
fn launch_agent_plist_path() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join("Library")
            .join("LaunchAgents")
            .join("com.ifence.tokenmonitor.plist"),
    )
}

pub fn autostart_enabled() -> bool {
    launch_agent_plist_path().is_some_and(|path| path.is_file())
}

pub fn set_autostart(enabled: bool) -> Result<()> {
    let Some(path) = launch_agent_plist_path() else {
        return Ok(());
    };
    if !enabled {
        if path.is_file() {
            std::fs::remove_file(&path).context("remove LaunchAgent plist")?;
        }
        return Ok(());
    }

    let exe = std::env::current_exe().context("resolve current exe")?;
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.ifence.tokenmonitor</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
        exe.display()
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create LaunchAgents directory")?;
    }
    std::fs::write(&path, plist).context("write LaunchAgent plist")?;
    Ok(())
}
