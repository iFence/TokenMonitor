//! Auto-update for the desktop app: check the GitHub releases feed, download
//! the Windows installer, and hand off to the installer (the user finishes the
//! wizard, then the app quits). Mirrors Lumia's update flow.
//!
//! The parsing / version-compare logic lives in `crate::core::update`; this
//! module only wires it to GPUI's async HTTP client and render state.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui::Context;
use http_client::{AsyncBody, HttpClient, Method, Request, Response};
use semver::Version;

use crate::core::update::{
    changelog_contents_url, changelog_url, evaluate_update, GithubRelease, UpdateInfo, UpdateState,
    RELEASES_LATEST_URL,
};

use super::app::TokenMonitorApp;

#[derive(Debug, Clone, Default)]
pub struct UpdateCheckUiState {
    pub state: UpdateState,
}

impl UpdateCheckUiState {
    pub fn is_busy(&self) -> bool {
        self.state.is_busy()
    }

    /// `true` when an update is available and ready to download.
    pub fn has_update(&self) -> bool {
        self.state.has_update()
    }
}

/// Drain a response body into a string, erroring on non-2xx status.
async fn response_text(response: Response<AsyncBody>) -> anyhow::Result<String> {
    if !response.status().is_success() {
        anyhow::bail!("http status {}", response.status());
    }
    let (_parts, mut body) = response.into_parts();
    let mut bytes = Vec::new();
    futures::io::AsyncReadExt::read_to_end(&mut body, &mut bytes).await?;
    Ok(String::from_utf8(bytes)?)
}

async fn fetch_text(client: &Arc<dyn HttpClient>, url: &str) -> anyhow::Result<String> {
    let response = client.get(url, AsyncBody::from(()), true).await?;
    response_text(response).await
}

/// Fetch the current `Changelog.md`. `raw.githubusercontent.com` is the primary
/// source (plain text, no rate limit); if it is unreachable or serves stale
/// content, fall back to the GitHub contents API, which hits `api.github.com` —
/// the same host the update check already uses for `releases/latest`.
async fn fetch_changelog(client: &Arc<dyn HttpClient>) -> anyhow::Result<String> {
    match fetch_text(client, &changelog_url()).await {
        Ok(content) => Ok(content),
        Err(_) => {
            let request = Request::builder()
                .uri(changelog_contents_url())
                .method(Method::GET)
                .header("Accept", "application/vnd.github.raw+json")
                .body(AsyncBody::from(()))
                .context("build GitHub contents API request")?;
            let response = client.send(request).await?;
            response_text(response).await
        }
    }
}

/// Returns `Some(UpdateInfo)` when a newer release exists, `None` when up-to-date.
async fn check_update(
    client: &Arc<dyn HttpClient>,
    current: &Version,
    portable: bool,
) -> anyhow::Result<Option<UpdateInfo>> {
    let json = fetch_text(client, RELEASES_LATEST_URL).await?;
    let release: GithubRelease =
        serde_json::from_str(&json).context("parse GitHub releases/latest response")?;
    let changelog = fetch_changelog(client).await.ok();
    evaluate_update(&release, current, portable, changelog.as_deref())
}

impl TokenMonitorApp {
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
        let portable = crate::platform::is_portable();

        cx.spawn(async move |this, cx| {
            let result = check_update(&client, &current, portable).await;
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

    /// Download the update asset for the currently-available version. Installed
    /// builds get the `.msi` (handed off to the installer, then the app quits);
    /// portable builds get the `.zip` saved next to the running exe, with the
    /// folder opened in Explorer for the user to extract over manually.
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
        let portable = crate::platform::is_portable();
        // Portable: save next to the running exe so the user can extract the
        // zip over it; installed: save to temp and hand off to msiexec.
        let dest = if portable {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
                .unwrap_or_else(std::env::temp_dir)
                .join(&asset.name)
        } else {
            std::env::temp_dir().join(&asset.name)
        };

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
                Ok(()) if portable => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(dir) = dest.parent() {
                            let _ = crate::platform::open_path_in_explorer(dir);
                        }
                        this.update_check.state = UpdateState::Downloaded {
                            latest_version: latest_version.clone(),
                            file_name: asset.name.clone(),
                        };
                        cx.notify();
                    });
                }
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
