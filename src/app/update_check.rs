//! Auto-update: check the GitHub releases feed, download the Windows installer,
//! and hand off to the installer (the user finishes the wizard, then the app
//! quits). Mirrors Lumia's update flow.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui::Context;
use http_client::{AsyncBody, HttpClient};
use semver::Version;

use super::app::RTokenApp;

const RELEASES_LATEST_URL: &str = "https://api.github.com/repos/iFence/rToken/releases/latest";

#[derive(Debug, Clone)]
pub enum UpdateState {
    Idle,
    Checking,
    Available {
        latest_version: Version,
        release_notes: String,
        asset: UpdateAsset,
    },
    Downloading {
        latest_version: Version,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Installing,
    UpToDate,
    Error(String),
}

impl Default for UpdateState {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug, Clone, Default)]
pub struct UpdateCheckUiState {
    pub state: UpdateState,
}

impl UpdateCheckUiState {
    pub fn is_busy(&self) -> bool {
        matches!(
            self.state,
            UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Installing
        )
    }

    /// `true` when an update is available and ready to download.
    pub fn has_update(&self) -> bool {
        matches!(self.state, UpdateState::Available { .. })
    }
}

#[derive(Debug, Clone)]
pub struct UpdateAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Select the Windows installer asset (the `.msi` produced by the release
/// workflow).
fn select_asset(assets: &[GithubAsset]) -> Option<UpdateAsset> {
    assets
        .iter()
        .find(|asset| asset.name.ends_with(".msi"))
        .map(|asset| UpdateAsset {
            name: asset.name.clone(),
            url: asset.browser_download_url.clone(),
            size: asset.size,
        })
}

async fn fetch_text(client: &Arc<dyn HttpClient>, url: &str) -> anyhow::Result<String> {
    let response = client.get(url, AsyncBody::from(()), true).await?;
    if !response.status().is_success() {
        anyhow::bail!("http status {}", response.status());
    }
    let (_parts, mut body) = response.into_parts();
    let mut bytes = Vec::new();
    futures::io::AsyncReadExt::read_to_end(&mut body, &mut bytes).await?;
    Ok(String::from_utf8(bytes)?)
}

struct UpdateInfo {
    latest_version: Version,
    release_notes: String,
    asset: UpdateAsset,
}

/// Returns `Some(UpdateInfo)` when a newer release exists, `None` when up-to-date.
async fn check_update(
    client: &Arc<dyn HttpClient>,
    current: &Version,
) -> anyhow::Result<Option<UpdateInfo>> {
    let json = fetch_text(client, RELEASES_LATEST_URL).await?;
    let release: GithubRelease =
        serde_json::from_str(&json).context("parse GitHub releases/latest response")?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let tag = release.tag_name.trim();
    let ver_str = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag);
    let latest = Version::parse(ver_str).with_context(|| format!("parse release tag {tag:?}"))?;
    if latest <= *current {
        return Ok(None);
    }
    let asset =
        select_asset(&release.assets).context("no matching installer asset for platform")?;
    Ok(Some(UpdateInfo {
        latest_version: latest,
        release_notes: release.body.clone().unwrap_or_default(),
        asset,
    }))
}

impl RTokenApp {
    /// Manual check (surfaces errors) or startup check (silent on failure when
    /// `manual` is false).
    pub fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.update_check.is_busy() {
            return;
        }
        self.update_check.state = UpdateState::Checking;
        cx.notify();

        let client = cx.http_client();
        let current =
            Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is valid semver");

        cx.spawn(async move |this, cx| {
            let result = check_update(&client, &current).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Some(info)) => {
                        let skipped = this
                            .skipped_update_version
                            .as_deref()
                            .map(|v| v.trim_start_matches('v'))
                            .and_then(|v| Version::parse(v).ok());
                        if skipped.as_ref() == Some(&info.latest_version) {
                            this.update_check.state = if manual {
                                UpdateState::UpToDate
                            } else {
                                UpdateState::Idle
                            };
                        } else {
                            this.update_check.state = UpdateState::Available {
                                latest_version: info.latest_version,
                                release_notes: info.release_notes,
                                asset: info.asset,
                            };
                        }
                    }
                    Ok(None) => {
                        this.update_check.state = UpdateState::UpToDate;
                    }
                    Err(err) => {
                        this.update_check.state = if manual {
                            UpdateState::Error(format!("{err:#}"))
                        } else {
                            UpdateState::Idle
                        };
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Called once after the window is created.
    pub fn maybe_check_for_updates_on_startup(&mut self, cx: &mut Context<Self>) {
        if self.check_updates_on_startup {
            self.check_for_updates(false, cx);
        }
    }

    /// Download the installer for the currently-available update, launch it, and quit.
    pub fn download_and_install(&mut self, cx: &mut Context<Self>) {
        let (latest_version, asset) = match &self.update_check.state {
            UpdateState::Available {
                latest_version,
                asset,
                ..
            } => (latest_version.clone(), asset.clone()),
            _ => return,
        };

        let total_bytes = (asset.size > 0).then_some(asset.size);
        self.update_check.state = UpdateState::Downloading {
            latest_version: latest_version.clone(),
            downloaded_bytes: 0,
            total_bytes,
        };
        cx.notify();

        let client = cx.http_client();
        let dest = std::env::temp_dir().join(&asset.name);

        cx.spawn(async move |this, cx| {
            let result: anyhow::Result<()> = async {
                let response = client.get(&asset.url, AsyncBody::from(()), true).await?;
                if !response.status().is_success() {
                    anyhow::bail!("http status {}", response.status());
                }
                let (_parts, mut body) = response.into_parts();
                let mut file = std::fs::File::create(&dest)?;
                let mut downloaded: u64 = 0;
                let mut buf = [0u8; 8192];
                let mut last_notify = Instant::now();
                loop {
                    let n = futures::io::AsyncReadExt::read(&mut body, &mut buf).await?;
                    if n == 0 {
                        break;
                    }
                    std::io::Write::write_all(&mut file, &buf[..n])?;
                    downloaded += n as u64;
                    if last_notify.elapsed() >= Duration::from_millis(100) {
                        last_notify = Instant::now();
                        let _ = this.update(cx, |this, cx| {
                            if let UpdateState::Downloading {
                                downloaded_bytes, ..
                            } = &mut this.update_check.state
                            {
                                *downloaded_bytes = downloaded;
                            }
                            cx.notify();
                        });
                    }
                }
                file.sync_all()?;
                if let Some(expected) = total_bytes {
                    if downloaded != expected {
                        anyhow::bail!("download incomplete: {downloaded}/{expected} bytes");
                    }
                }
                Ok(())
            }
            .await;

            match result {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.update_check.state = UpdateState::Installing;
                        cx.notify();
                    });
                    let _ = crate::platform::launch_installer(&dest);
                    let _ = this.update(cx, |_, cx| cx.quit());
                }
                Err(err) => {
                    let _ = this.update(cx, |this, cx| {
                        this.update_check.state = UpdateState::Error(format!("{err:#}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Persist the currently-available version as skipped and dismiss the prompt.
    pub fn skip_update(&mut self, cx: &mut Context<Self>) {
        if let UpdateState::Available { latest_version, .. } = &self.update_check.state {
            let version = latest_version.to_string();
            self.skipped_update_version = Some(version.clone());
            let _ = self.collector.set_skipped_update_version(Some(&version));
        }
        self.update_check.state = UpdateState::Idle;
        cx.notify();
    }
}
