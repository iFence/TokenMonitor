//! Shared auto-update logic for both frontends: query the GitHub releases
//! feed, compare the semver tag against the running version, and assemble
//! per-version release notes from `Changelog.md`.
//!
//! No HTTP / framework dependency — each frontend fetches the raw JSON and
//! changelog markdown with its own client (GPUI: `http_client`; TUI: `ureq`)
//! and hands the text to the pure functions here.

use anyhow::Context as _;
use semver::Version;

pub const RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/iFence/TokenMonitor/releases/latest";
const DEFAULT_BRANCH: &str = "main";
const CHANGELOG_FILENAME: &str = "Changelog.md";

pub fn changelog_url() -> String {
    format!("https://raw.githubusercontent.com/iFence/TokenMonitor/{DEFAULT_BRANCH}/{CHANGELOG_FILENAME}")
}

/// GitHub contents API for `Changelog.md`; with the raw media type it returns
/// the file bytes directly (no base64). Unlike `raw.githubusercontent.com`,
/// this hits `api.github.com` — the same host the update check itself already
/// talks to, so it is a reachable fallback where the raw CDN is not.
pub fn changelog_contents_url() -> String {
    format!("https://api.github.com/repos/iFence/TokenMonitor/contents/{CHANGELOG_FILENAME}?ref={DEFAULT_BRANCH}")
}

/// UI state machine for the update flow, shared by both frontends.
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
    /// Portable build only: the zip was saved next to the running exe.
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

impl UpdateState {
    /// `true` while a check or download is in flight.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing
        )
    }

    /// `true` when an update is available and ready to download.
    pub fn has_update(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

#[derive(Debug, Clone, Default)]
pub struct UpdateAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub latest_version: Version,
    pub release_notes: String,
    pub asset: UpdateAsset,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
}

/// Whether an asset name matches this platform's update artifact.
///
/// - Windows: installed builds use the `.msi` installer, portable (免安装)
///   builds use the `.zip` bundle.
/// - Linux: any `-linux-x64` tarball. Official Linux packages are
///   statically-linked musl builds, so `-musl` assets are preferred (see
///   [`select_asset_for_os`]); `portable` is ignored — Linux ships only
///   portable tarballs.
/// - Other platforms: no matching assets.
fn asset_matches(name: &str, portable: bool, os: &str) -> bool {
    match os {
        "linux" => name.ends_with("-linux-x64-musl.tar.gz") || name.ends_with("-linux-x64.tar.gz"),
        "windows" => {
            if portable {
                name.ends_with(".zip")
            } else {
                name.ends_with(".msi")
            }
        }
        _ => false,
    }
}

/// Select the update asset for the running platform.
///
/// - Windows: the `.msi` installer for an installed build, or the portable
///   `.zip` for a portable (免安装) build.
/// - Linux: a `-linux-x64` tarball, preferring the statically-linked musl
///   package when both are uploaded.
fn select_asset(assets: &[GithubAsset], portable: bool) -> Option<UpdateAsset> {
    select_asset_for_os(assets, portable, std::env::consts::OS)
}

/// First asset whose name matches this platform (see [`asset_matches`]).
fn pick_matching<'a>(
    assets: &'a [GithubAsset],
    portable: bool,
    os: &str,
) -> Option<&'a GithubAsset> {
    assets
        .iter()
        .find(|asset| asset_matches(&asset.name, portable, os))
}

/// `select_asset` parametrized by OS so tests can exercise every platform.
fn select_asset_for_os(assets: &[GithubAsset], portable: bool, os: &str) -> Option<UpdateAsset> {
    // Official Linux packages are statically-linked musl builds; prefer them
    // over a glibc tarball uploaded alongside, whatever the asset order.
    let chosen = if os == "linux" {
        assets
            .iter()
            .find(|asset| asset.name.ends_with("-linux-x64-musl.tar.gz"))
            .or_else(|| pick_matching(assets, portable, os))
    } else {
        pick_matching(assets, portable, os)
    };
    chosen.map(|asset| UpdateAsset {
        name: asset.name.clone(),
        url: asset.browser_download_url.clone(),
        size: asset.size,
    })
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
    "更新说明请查看 [GitHub Releases](https://github.com/iFence/TokenMonitor/releases)。"
        .to_string()
}

/// Turn a parsed `releases/latest` response into `UpdateInfo`, or `None` when
/// the running version is already current. `changelog_content` is the fetched
/// `Changelog.md` (preferred for notes); pass `None` to fall back to the
/// release body.
pub fn evaluate_update(
    release: &GithubRelease,
    current: &Version,
    portable: bool,
    changelog_content: Option<&str>,
) -> anyhow::Result<Option<UpdateInfo>> {
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
    let notes = match changelog_content {
        Some(content) => aggregate_release_notes(
            &parse_changelog(content),
            current,
            &latest,
            release.body.as_deref(),
        ),
        None => sanitize_release_body(release.body.as_deref()).unwrap_or_else(releases_fallback),
    };
    Ok(Some(UpdateInfo {
        latest_version: latest,
        release_notes: notes,
        asset,
    }))
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
        let auto = Some(
            "**Full Changelog**: https://github.com/iFence/TokenMonitor/compare/v0.2.1...v0.2.2",
        );
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
            Some("**Full Changelog**: https://github.com/iFence/TokenMonitor/compare/v0.2.1...v0.2.2"),
        );
        assert!(notes.contains("GitHub Releases"));
        assert!(!notes.to_ascii_lowercase().contains("full changelog"));

        // A real release body is preserved as the fallback.
        let notes =
            aggregate_release_notes(&entries, &v(0, 1, 2), &v(0, 1, 3), Some("修复了崩溃问题"));
        assert_eq!(notes, "修复了崩溃问题");
    }

    /// Asset named for the host platform so `evaluate_update` exercises the
    /// real `select_asset` branch regardless of which OS the tests run on.
    fn host_asset(version: &str) -> GithubAsset {
        let name = if std::env::consts::OS == "linux" {
            format!("TokenMonitor-v{version}-linux-x64-musl.tar.gz")
        } else {
            format!("TokenMonitor-v{version}-windows-x64.zip")
        };
        GithubAsset {
            name: name.clone(),
            browser_download_url: format!("https://example.com/{name}"),
            size: 100,
        }
    }

    #[test]
    fn select_asset_picks_zip_for_portable_and_msi_for_installed() {
        let assets = vec![
            GithubAsset {
                name: "TokenMonitor-v0.2.2-windows-x64.zip".into(),
                browser_download_url: "https://example.com/portable.zip".into(),
                size: 100,
            },
            GithubAsset {
                name: "TokenMonitor_0.2.2_x64_en-US.msi".into(),
                browser_download_url: "https://example.com/installer.msi".into(),
                size: 200,
            },
        ];
        let portable = select_asset_for_os(&assets, true, "windows").expect("portable asset");
        assert!(portable.name.ends_with(".zip"));

        let installed = select_asset_for_os(&assets, false, "windows").expect("installer asset");
        assert!(installed.name.ends_with(".msi"));
    }

    #[test]
    fn select_asset_picks_linux_tarball() {
        let assets = vec![
            GithubAsset {
                name: "TokenMonitor-v0.3.1-linux-x64.tar.gz".into(),
                browser_download_url: "https://example.com/linux.tar.gz".into(),
                size: 300,
            },
            GithubAsset {
                name: "TokenMonitor-v0.3.1-linux-x64-musl.tar.gz".into(),
                browser_download_url: "https://example.com/linux-musl.tar.gz".into(),
                size: 400,
            },
        ];

        // musl is preferred when both tarballs are uploaded.
        let asset = select_asset_for_os(&assets, true, "linux").expect("linux asset");
        assert!(asset.name.ends_with("-linux-x64-musl.tar.gz"));

        // `portable` is irrelevant on Linux.
        let asset = select_asset_for_os(&assets, false, "linux").expect("linux asset");
        assert!(asset.name.ends_with("-linux-x64-musl.tar.gz"));

        // Only the glibc tarball present → fall back to it.
        let gnu_only = vec![GithubAsset {
            name: "TokenMonitor-v0.3.1-linux-x64.tar.gz".into(),
            browser_download_url: "https://example.com/linux.tar.gz".into(),
            size: 300,
        }];
        let asset = select_asset_for_os(&gnu_only, true, "linux").expect("gnu linux asset");
        assert!(asset.name.ends_with("-linux-x64.tar.gz"));
        assert!(!asset.name.ends_with("-musl.tar.gz"));
    }

    #[test]
    fn select_asset_ignores_other_platform_assets() {
        let windows = vec![GithubAsset {
            name: "TokenMonitor-v0.2.2-windows-x64.zip".into(),
            browser_download_url: "https://example.com/portable.zip".into(),
            size: 100,
        }];
        assert!(select_asset_for_os(&windows, true, "linux").is_none());

        let linux = vec![GithubAsset {
            name: "TokenMonitor-v0.3.1-linux-x64-musl.tar.gz".into(),
            browser_download_url: "https://example.com/linux-musl.tar.gz".into(),
            size: 400,
        }];
        assert!(select_asset_for_os(&linux, true, "windows").is_none());
        assert!(select_asset_for_os(&linux, true, "macos").is_none());
    }

    #[test]
    fn evaluate_update_skips_drafts_and_current_versions() {
        let assets = vec![host_asset("0.2.2")];

        // Draft / prerelease releases are ignored.
        let draft = GithubRelease {
            tag_name: "v0.2.2".into(),
            body: None,
            assets: assets.clone(),
            draft: true,
            prerelease: false,
        };
        assert!(evaluate_update(&draft, &v(0, 2, 1), true, None)
            .unwrap()
            .is_none());

        // Same version as current → up to date.
        let release = GithubRelease {
            tag_name: "v0.2.2".into(),
            body: None,
            assets,
            draft: false,
            prerelease: false,
        };
        assert!(evaluate_update(&release, &v(0, 2, 2), true, None)
            .unwrap()
            .is_none());

        // Newer version → UpdateInfo with a release-body fallback.
        let info = evaluate_update(&release, &v(0, 2, 1), true, None)
            .unwrap()
            .expect("update available");
        assert_eq!(info.latest_version, v(0, 2, 2));
        assert!(info.release_notes.contains("GitHub Releases"));
    }
}
