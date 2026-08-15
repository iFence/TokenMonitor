//! Discovery of provider data directories, including WSL distros on Windows.

use super::source::ScanRoot;
#[cfg(windows)]
use std::path::PathBuf;

/// Every candidate data directory for a provider: the local home dir (no label)
/// plus, on Windows, each WSL distro's home dir (labelled `wsl/<distro>/<user>`
/// so their records never collide with the primary root or each other).
pub(crate) fn discover_roots(suffix: &[&str]) -> Vec<ScanRoot> {
    let mut roots = Vec::new();
    if let Ok(home) = crate::platform::home_dir() {
        let dir = suffix.iter().fold(home, |p, s| p.join(s));
        roots.push(ScanRoot { dir, label: None });
    }
    #[cfg(windows)]
    roots.extend(discover_wsl_roots(suffix));
    roots
}

/// Append one root per (distro, user) whose `~/.claude`/`~/.codex` lives inside
/// a WSL distribution. Best effort: any failure skips that distro and returns
/// whatever else was found.
#[cfg(windows)]
fn discover_wsl_roots(suffix: &[&str]) -> Vec<ScanRoot> {
    let mut roots = Vec::new();
    for distro in wsl_distros() {
        let home_root = PathBuf::from(format!(r"\\wsl.localhost\{distro}")).join("home");
        // Enumerating /home over the 9P UNC path may auto-start the distro
        // (slow on first access); this runs on the background scan thread, so
        // the UI is never blocked.
        let Ok(entries) = std::fs::read_dir(&home_root) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(user) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let dir = suffix.iter().fold(entry.path(), |p, s| p.join(s));
            roots.push(ScanRoot {
                dir,
                label: Some(format!("wsl/{distro}/{user}")),
            });
        }
    }
    roots
}

/// Names of installed WSL distributions via `wsl.exe --list --quiet`.
#[cfg(windows)]
fn wsl_distros() -> Vec<String> {
    let Ok(out) = std::process::Command::new("wsl.exe")
        .args(["--list", "--quiet"])
        .output()
    else {
        return Vec::new();
    };
    let text = decode_wsl_stdout(&out.stdout);
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `wsl.exe` writes UTF-16LE to a redirected stdout; fall back to UTF-8 for
/// safety. Detection is the usual "ASCII text has a NUL high byte" heuristic.
#[cfg(windows)]
fn decode_wsl_stdout(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[1] == 0 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}
