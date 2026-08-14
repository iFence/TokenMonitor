use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

pub fn app_data_dir() -> Result<PathBuf> {
    Err(anyhow!("app_data_dir not implemented on Linux yet"))
}

pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow!("no home directory"))
}

pub fn open_path_in_explorer(_path: &Path) -> Result<()> {
    Err(anyhow!(
        "open_path_in_explorer not implemented on Linux yet"
    ))
}
