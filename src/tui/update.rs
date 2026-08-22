//! Update check for the TUI: runs the GitHub releases query on a background
//! thread (blocking `ureq`), posting results back over an async channel so the
//! ratatui loop never blocks. Version compare / release-note assembly are in
//! `crate::core::update`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result};
use semver::Version;

use crate::core::update::{
    changelog_contents_url, changelog_url, evaluate_update, GithubRelease, UpdateInfo,
    RELEASES_LATEST_URL,
};

/// Result events delivered to the app from the check/download thread.
pub enum UpdateEvent {
    Checked {
        manual: bool,
        result: Result<Option<UpdateInfo>>,
    },
    Downloaded {
        version: Version,
        result: Result<std::path::PathBuf>,
    },
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(concat!("TokenMonitor-TUI/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
}

fn get_text(agent: &ureq::Agent, url: &str) -> Result<String> {
    let response = agent.get(url).call().context("http request")?;
    let mut text = String::new();
    response.into_reader().read_to_string(&mut text)?;
    Ok(text)
}

/// Fetch `Changelog.md`: the raw CDN first, then the GitHub contents API (raw
/// media type, same host as the release check) as a fallback.
fn fetch_changelog(agent: &ureq::Agent) -> Result<String> {
    match get_text(agent, &changelog_url()) {
        Ok(content) => Ok(content),
        Err(_) => {
            let response = agent
                .get(&changelog_contents_url())
                .set("Accept", "application/vnd.github.raw+json")
                .call()
                .context("fetch changelog via contents API")?;
            let mut text = String::new();
            response.into_reader().read_to_string(&mut text)?;
            Ok(text)
        }
    }
}

/// Run one update check (blocking). Returns `None` when already up-to-date.
pub fn check_update(current: &Version, portable: bool) -> Result<Option<UpdateInfo>> {
    let agent = agent();
    let json = get_text(&agent, RELEASES_LATEST_URL)?;
    let release: GithubRelease =
        serde_json::from_str(&json).context("parse GitHub releases/latest response")?;
    let changelog = fetch_changelog(&agent).ok();
    evaluate_update(&release, current, portable, changelog.as_deref())
}

/// Stream the update asset to `dest` (blocking), verifying the byte count when
/// GitHub reports one.
pub fn download(url: &str, dest: &Path, expected_size: u64) -> Result<()> {
    let agent = agent();
    let response = agent.get(url).call().context("download update asset")?;
    if !(200..300).contains(&response.status()) {
        anyhow::bail!("http status {}", response.status());
    }
    let mut file = std::fs::File::create(dest).context("create download file")?;
    let mut reader = response.into_reader();
    std::io::copy(&mut reader, &mut file).context("stream download")?;
    file.sync_all().context("flush download")?;
    if expected_size > 0 {
        let size = file.metadata()?.len();
        if size != expected_size {
            anyhow::bail!("download incomplete: {size}/{expected_size} bytes");
        }
    }
    Ok(())
}
