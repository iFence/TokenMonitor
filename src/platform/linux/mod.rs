use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// TokenMonitor's own data directory
/// (`$XDG_DATA_HOME/tokenmonitor`, defaulting to `~/.local/share/tokenmonitor`).
pub fn app_data_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .map(|p| p.join("tokenmonitor"))
        .context("resolve OS data directory")?;
    Ok(dir)
}

/// Legacy `rToken` data directory (`~/.local/share/rtoken`) from before the
/// rename. Consulted only for the one-time database migration in
/// `storage::migrate_legacy_db`.
pub fn legacy_data_dir() -> Result<PathBuf> {
    dirs::data_dir()
        .map(|p| p.join("rtoken"))
        .context("resolve OS data directory")
}

pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().context("resolve home directory")
}

/// Whether the binary is running from a portable (免安装) layout. The packaged
/// tarball ships a `.portable` marker and `README.md` next to the binary, same
/// convention as the Windows portable zip.
pub fn is_portable() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    dir.join(".portable").is_file() || dir.join("README.md").is_file()
}

/// Open a path with the user's file manager via `xdg-open`.
pub fn open_path_in_explorer(path: &Path) -> Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .context("spawn xdg-open")?;
    Ok(())
}

/// Linux ships only portable tarballs (no installer); let the desktop's
/// download handler decide what to do with the downloaded artifact.
pub fn launch_installer(path: &Path) -> Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .context("spawn xdg-open")?;
    Ok(())
}

/// No native titlebar on the terminal frontend; no-op to keep the trait
/// surface uniform across platforms.
pub fn apply_dark_titlebar() {}
