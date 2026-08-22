use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

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
