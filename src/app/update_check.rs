//! Auto-update: check the GitHub releases feed, download the Windows installer,
//! and hand off to the installer (the user finishes the wizard, then the app
//! quits). Mirrors Lumia's update flow.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui::Context;
use http_client::{AsyncBody, HttpClient, Method, Request, Response};
use semver::Version;

use super::app::RTokenApp;

const RELEASES_LATEST_URL: &str = "https://api.github.com/repos/iFence/rToken/releases/latest";
const DEFAULT_BRANCH: &str = "main";
const CHANGELOG_FILENAME: &str = "Changelog.md";

fn changelog_url() -> String {
    format!("https://raw.githubusercontent.com/iFence/rToken/{DEFAULT_BRANCH}/{CHANGELOG_FILENAME}")
}

/// GitHub contents API for `Changelog.md`; with the raw media type it returns
/// the file bytes directly (no base64). Unlike `raw.githubusercontent.com`,
/// this hits `api.github.com` — the same host the update check itself already
/// talks to, so it is a reachable fallback where the raw CDN is not.
fn changelog_contents_url() -> String {
    format!("https://api.github.com/repos/iFence/rToken/contents/{CHANGELOG_FILENAME}?ref={DEFAULT_BRANCH}")
}

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
    /// Portable build only: the zip was saved next to the running exe and the
    /// containing folder was opened in Explorer for the user to extract over.
    Downloaded {
        latest_version: Version,
        file_name: String,
    },
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

/// Select the Windows update asset: the `.msi` installer for an installed
/// build, or the portable `.zip` for a portable (免安装) build.
fn select_asset(assets: &[GithubAsset], portable: bool) -> Option<UpdateAsset> {
    let matches = |name: &str| {
        if portable {
            name.ends_with(".zip")
        } else {
            name.ends_with(".msi")
        }
    };
    assets
        .iter()
        .find(|asset| matches(&asset.name))
        .map(|asset| UpdateAsset {
            name: asset.name.clone(),
            url: asset.browser_download_url.clone(),
            size: asset.size,
        })
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangelogEntry {
    version: Version,
    body: String,
}

/// Parse `## vX.Y.Z` sections out of `Changelog.md`.
///
/// - Headers are `## ` followed by `vX.Y.Z` (leading `v`/`V` optional).
/// - A trailing date/token on the header line (e.g. `## v0.1.3 (2026-07-28)`)
///   is ignored: only the first whitespace-delimited token after `## ` is parsed.
/// - `---` separators and adjacent headers both cleanly terminate the previous entry.
/// - Lines before the first recognized header are dropped.
/// - A `## ` line whose token is not a version is treated as body content of the
///   current section (so sub-headers inside a version block are preserved).
fn parse_changelog(content: &str) -> Vec<ChangelogEntry> {
    let mut entries = Vec::new();
    let mut current: Option<(Version, String)> = None;

    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("## ") {
            let token = rest.split_whitespace().next().unwrap_or("");
            let ver_str = token
                .strip_prefix('v')
                .or_else(|| token.strip_prefix('V'))
                .unwrap_or(token);
            if let Ok(version) = Version::parse(ver_str) {
                if let Some((v, body)) = current.take() {
                    entries.push(ChangelogEntry {
                        version: v,
                        body: body.trim_end().to_string(),
                    });
                }
                current = Some((version, String::new()));
            } else if let Some((_, body)) = current.as_mut() {
                body.push_str(line);
                body.push('\n');
            }
        } else if trimmed == "---" {
            if let Some((v, body)) = current.take() {
                entries.push(ChangelogEntry {
                    version: v,
                    body: body.trim_end().to_string(),
                });
            }
        } else if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some((v, body)) = current.take() {
        entries.push(ChangelogEntry {
            version: v,
            body: body.trim_end().to_string(),
        });
    }
    entries
}

/// Return release notes for versions `current < v <= latest`, sorted descending,
/// joined with a `---` separator. Falls back to `fallback_body` (the GitHub
/// release body, minus GitHub's auto-generated compare link) when no matching
/// entries are found, and to a Releases-page pointer when that is empty too.
fn aggregate_release_notes(
    entries: &[ChangelogEntry],
    current: &Version,
    latest: &Version,
    fallback_body: Option<&str>,
) -> String {
    let mut matched: Vec<&ChangelogEntry> = entries
        .iter()
        .filter(|entry| entry.version > *current && entry.version <= *latest)
        .collect();
    matched.sort_by(|a, b| b.version.cmp(&a.version));

    if matched.is_empty() {
        return sanitize_release_body(fallback_body).unwrap_or_else(releases_fallback);
    }

    matched
        .into_iter()
        .map(|entry| format!("## v{}\n\n{}", entry.version, entry.body.trim()))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

/// Strip GitHub's auto-generated release boilerplate. Publishing a release
/// without notes makes GitHub fill the body with a `**Full Changelog**:
/// <compare-url>` line — never surface that bare compare link in the UI.
/// Returns `None` when nothing real remains.
fn sanitize_release_body(body: Option<&str>) -> Option<String> {
    let cleaned = body?
        .lines()
        .filter(|line| !line.to_ascii_lowercase().contains("full changelog"))
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Last-resort notes when neither the Changelog nor a usable release body is
/// available: send the user to the Releases page.
fn releases_fallback() -> String {
    "更新说明请查看 [GitHub Releases](https://github.com/iFence/rToken/releases)。".to_string()
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
    portable: bool,
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
        select_asset(&release.assets, portable).context("no matching update asset for platform")?;
    // Prefer the maintained Changelog (per-version release notes) over the
    // auto-generated GitHub release body, which is often just a compare link.
    let notes = match fetch_changelog(client).await {
        Ok(content) => aggregate_release_notes(
            &parse_changelog(&content),
            current,
            &latest,
            release.body.as_deref(),
        ),
        Err(_) => sanitize_release_body(release.body.as_deref()).unwrap_or_else(releases_fallback),
    };
    Ok(Some(UpdateInfo {
        latest_version: latest,
        release_notes: notes,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u64, minor: u64, patch: u64) -> Version {
        Version::new(major, minor, patch)
    }

    #[test]
    fn parse_changelog_extracts_version_sections() {
        let content =
            "# Changelog\n\npreamble\n\n## v0.1.3\n\n- new feature\n\n## v0.1.2\n\n- initial\n";
        let entries = parse_changelog(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].version, v(0, 1, 3));
        assert_eq!(entries[1].version, v(0, 1, 2));
        assert!(entries[0].body.contains("new feature"));
        assert!(entries[1].body.contains("initial"));
    }

    #[test]
    fn parse_changelog_handles_separator_and_adjacent_headers() {
        let content = "## v0.1.3\nbody1\n---\n## v0.1.2\nbody2\n";
        let entries = parse_changelog(content);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].body.contains("body1"));
        assert!(entries[1].body.contains("body2"));

        let adjacent = "## v0.1.3\n## v0.1.2\nbody2\n";
        let entries = parse_changelog(adjacent);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].version, v(0, 1, 3));
        assert_eq!(entries[1].version, v(0, 1, 2));
    }

    #[test]
    fn parse_changelog_drops_preamble_and_ignores_date() {
        let content = "# Changelog\n\n约定：\n- foo\n\n## v0.1.2 (2026-07-28)\nbody\n";
        let entries = parse_changelog(content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].version, v(0, 1, 2));
        assert!(entries[0].body.contains("body"));
    }

    #[test]
    fn aggregate_release_notes_filters_sorts_and_falls_back() {
        let entries = vec![
            ChangelogEntry {
                version: v(0, 1, 2),
                body: "current".into(),
            },
            ChangelogEntry {
                version: v(0, 1, 3),
                body: "minor".into(),
            },
            ChangelogEntry {
                version: v(0, 1, 4),
                body: "latest".into(),
            },
        ];
        let notes = aggregate_release_notes(&entries, &v(0, 1, 2), &v(0, 1, 4), None);
        assert!(notes.contains("## v0.1.4"));
        assert!(notes.contains("## v0.1.3"));
        assert!(!notes.contains("## v0.1.2"));
        let pos4 = notes.find("v0.1.4").unwrap();
        let pos3 = notes.find("v0.1.3").unwrap();
        assert!(pos4 < pos3);

        // No matching entries → fall back to the release body.
        let notes = aggregate_release_notes(&entries, &v(0, 1, 4), &v(0, 1, 5), Some("fallback"));
        assert_eq!(notes, "fallback");
    }

    #[test]
    fn sanitize_release_body_strips_auto_generated_compare_link() {
        // GitHub's auto-generated body is just the compare link → nothing left.
        let auto = Some("**Full Changelog**: https://github.com/iFence/rToken/compare/v0.2.1...v0.2.2");
        assert_eq!(sanitize_release_body(auto), None);

        // Real notes with an auto-generated line appended keep the real notes.
        let mixed = Some("## What's Changed\n- 新增功能\n\n**Full Changelog**: https://…");
        let cleaned = sanitize_release_body(mixed).unwrap();
        assert!(cleaned.contains("新增功能"));
        assert!(!cleaned.to_ascii_lowercase().contains("full changelog"));
    }

    #[test]
    fn aggregate_never_surfaces_auto_generated_compare_link() {
        let entries = vec![ChangelogEntry {
            version: v(0, 1, 2),
            body: "current".into(),
        }];
        // No changelog entry between current and latest, and the release body
        // is auto-generated → point at the Releases page, not the bare link.
        let notes = aggregate_release_notes(
            &entries,
            &v(0, 1, 2),
            &v(0, 1, 3),
            Some("**Full Changelog**: https://github.com/iFence/rToken/compare/v0.2.1...v0.2.2"),
        );
        assert!(notes.contains("GitHub Releases"));
        assert!(!notes.to_ascii_lowercase().contains("full changelog"));

        // A real release body is preserved as the fallback.
        let notes = aggregate_release_notes(&entries, &v(0, 1, 2), &v(0, 1, 3), Some("修复了崩溃问题"));
        assert_eq!(notes, "修复了崩溃问题");
    }

    #[test]
    fn select_asset_picks_zip_for_portable_and_msi_for_installed() {
        let assets = vec![
            GithubAsset {
                name: "rtoken-v0.2.2-windows-x64.zip".into(),
                browser_download_url: "https://example.com/portable.zip".into(),
                size: 100,
            },
            GithubAsset {
                name: "rtoken_0.2.2_x64_en-US.msi".into(),
                browser_download_url: "https://example.com/installer.msi".into(),
                size: 200,
            },
        ];
        let portable = select_asset(&assets, true).expect("portable asset");
        assert!(portable.name.ends_with(".zip"));

        let installed = select_asset(&assets, false).expect("installer asset");
        assert!(installed.name.ends_with(".msi"));
    }
}
