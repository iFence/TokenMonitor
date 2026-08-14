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

/// Open a path in Windows Explorer.
pub fn open_path_in_explorer(path: &Path) -> Result<()> {
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .context("spawn explorer.exe")?;
    Ok(())
}
