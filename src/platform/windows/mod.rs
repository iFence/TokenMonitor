use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// rToken's own data directory (`%APPDATA%\rToken`).
pub fn app_data_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .map(|p| p.join("rToken"))
        .context("resolve OS data directory")?;
    Ok(dir)
}

pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().context("resolve home directory")
}

/// Whether the app is running from a portable (免安装) layout rather than an
/// MSI install. The portable zip ships a `.portable` marker and `README.md`
/// next to the exe; the MSI installs only the exe into `bin\`.
pub fn is_portable() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    dir.join(".portable").is_file() || dir.join("README.md").is_file()
}

/// Open a path in Windows Explorer.
pub fn open_path_in_explorer(path: &Path) -> Result<()> {
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .context("spawn explorer.exe")?;
    Ok(())
}

/// Launch a downloaded Windows installer (an `.msi`) through the Windows
/// Installer wizard. Spawns and returns immediately so the app can quit.
pub fn launch_installer(path: &Path) -> Result<()> {
    std::process::Command::new("msiexec.exe")
        .arg("/i")
        .arg(path)
        .spawn()
        .context("spawn msiexec")?;
    Ok(())
}
