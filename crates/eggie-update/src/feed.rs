//! Update feed abstraction and its implementations.
//!
//! A feed lists the releases available upstream. The mock implementation reads
//! a local JSON document ([`FileFeed`]); once the GitHub repository exists an
//! HTTP implementation can be added behind the same [`UpdateFeed`] trait
//! without touching the controller or UI.
//!
//! The feed document is `{ "releases": [ ReleaseInfo, ... ] }`. Modeling the
//! feed as a *list* (rather than just "the latest") lets the controller
//! aggregate the notes of every release between the user's version and the
//! newest one — a user several versions behind sees the full changelog.
//!
//! Terminology, for the future GitHub backend: the release *list* and each
//! version's notes come from GitHub's `/releases` endpoint (the `body` field);
//! a per-release `manifest.json` *asset* carries only that one release's install
//! metadata (`protocol_version`, `sha256`, package name). `GitHubFeed` will
//! assemble those two sources into the `Vec<ReleaseInfo>` this trait returns —
//! so a "manifest.json" asset is per-release, while a "feed document" (this
//! struct / the mock file) is the assembled list.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use semver::Version;
use serde::Deserialize;
use url::Url;

/// Information about a single release, as served by a feed.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ReleaseInfo {
    /// The released app version, e.g. "0.0.2".
    pub version: Version,
    /// The daemon protocol version this release speaks. Used to decide
    /// whether an update can keep the running daemon.
    pub protocol_version: u32,
    /// Human-readable release notes (Markdown), shown in the update window.
    pub release_notes: String,
    /// RFC3339 publish timestamp, display only.
    pub published_at: String,
    /// Where to download the update package (a zip of the whole .app).
    /// `file://` URLs are supported for mock feeds.
    pub download_url: Url,
    /// Lowercase hex SHA-256 of the download, verified before install.
    pub sha256: String,
}

/// The top-level feed document: the assembled list of published releases.
#[derive(Clone, Debug, Deserialize)]
struct FeedDocument {
    releases: Vec<ReleaseInfo>,
}

impl ReleaseInfo {
    /// True if installing this release can keep the currently running
    /// daemon: same protocol version, so the new app can talk to it.
    pub fn daemon_compatible(&self) -> bool {
        self.protocol_version == eggie_protocol::PROTOCOL_VERSION
    }
}

/// A source of release information.
pub trait UpdateFeed: Send + Sync {
    /// Fetch all published releases. Order is not assumed; the controller
    /// sorts. Errors are surfaced to the UI (manual check) or swallowed
    /// (automatic background check).
    fn fetch_releases(&self) -> Result<Vec<ReleaseInfo>>;
}

/// Parse and validate a feed document.
fn parse_feed(bytes: &str, source: &str) -> Result<Vec<ReleaseInfo>> {
    let document: FeedDocument = serde_json::from_str(bytes)
        .with_context(|| format!("failed to parse update feed {source}"))?;
    for release in &document.releases {
        release.validate()?;
    }
    Ok(document.releases)
}

/// Mock feed backed by a local JSON document. The file layout matches
/// [`FeedDocument`]; see `packaging/update-feed.example.json`.
pub struct FileFeed {
    path: PathBuf,
}

impl FileFeed {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl UpdateFeed for FileFeed {
    fn fetch_releases(&self) -> Result<Vec<ReleaseInfo>> {
        let contents = std::fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read update feed {}", self.path.display()))?;
        parse_feed(&contents, &self.path.display().to_string())
    }
}

/// Feed that fetches a JSON document over HTTP(S). Reserved for the real
/// GitHub Releases backend; parsing is shared with [`FileFeed`].
pub struct HttpFeed {
    url: Url,
}

impl UpdateFeed for HttpFeed {
    fn fetch_releases(&self) -> Result<Vec<ReleaseInfo>> {
        let body = ureq::get(self.url.as_str())
            .call()
            .with_context(|| format!("failed to fetch update feed {}", self.url))?
            .into_string()
            .with_context(|| format!("failed to read update feed {}", self.url))?;
        parse_feed(&body, &self.url.to_string())
    }
}

impl ReleaseInfo {
    /// Cross-check fields that serde can't enforce on its own.
    fn validate(&self) -> Result<()> {
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("release {} has invalid sha256: {:?}", self.version, self.sha256);
        }
        match self.download_url.scheme() {
            "file" | "http" | "https" => Ok(()),
            other => bail!(
                "release {} has unsupported download URL scheme: {other}",
                self.version
            ),
        }
    }
}

/// Build a feed for a URL, dispatching on the scheme.
pub fn feed_from_url(url: &Url) -> Arc<dyn UpdateFeed> {
    match url.scheme() {
        "file" => {
            let path = url
                .to_file_path()
                .unwrap_or_else(|_| PathBuf::from(url.path()));
            Arc::new(FileFeed::new(path))
        }
        _ => Arc::new(HttpFeed { url: url.clone() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release_json(overrides: serde_json::Value) -> String {
        let mut value = json!({
            "version": "0.0.2",
            "protocol_version": 1,
            "release_notes": "- fix bugs",
            "published_at": "2026-08-13T10:00:00Z",
            "download_url": "file:///tmp/Eggie-0.0.2.zip",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        });
        if let (Some(object), Some(overrides)) = (value.as_object_mut(), overrides.as_object()) {
            for (key, value) in overrides {
                object.insert(key.clone(), value.clone());
            }
        }
        value.to_string()
    }

    #[test]
    fn parses_full_release_info() {
        let release: ReleaseInfo = serde_json::from_str(&release_json(json!({}))).unwrap();
        assert_eq!(release.version, Version::new(0, 0, 2));
        assert_eq!(release.protocol_version, 1);
        assert!(release.daemon_compatible());
        assert!(release.validate().is_ok());
    }

    #[test]
    fn rejects_bad_sha256() {
        let release: ReleaseInfo =
            serde_json::from_str(&release_json(json!({ "sha256": "nope" }))).unwrap();
        assert!(release.validate().is_err());
    }

    #[test]
    fn rejects_unsupported_scheme() {
        let release: ReleaseInfo = serde_json::from_str(&release_json(json!({
            "download_url": "ftp://example.com/x.zip"
        })))
        .unwrap();
        assert!(release.validate().is_err());
    }

    #[test]
    fn daemon_compatibility_tracks_protocol_version() {
        let mut release: ReleaseInfo = serde_json::from_str(&release_json(json!({}))).unwrap();
        assert!(release.daemon_compatible());
        release.protocol_version = eggie_protocol::PROTOCOL_VERSION + 1;
        assert!(!release.daemon_compatible());
    }

    #[test]
    fn file_feed_reads_local_json() {
        let dir = std::env::temp_dir().join(format!("eggie-feed-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("feed.json");
        let doc = format!("{{\"releases\":[{}]}}", release_json(json!({})));
        std::fs::write(&path, doc).unwrap();

        let feed = FileFeed::new(path.clone());
        let releases = feed.fetch_releases().unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].version, Version::new(0, 0, 2));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_feed_rejects_invalid_release_in_document() {
        let dir = std::env::temp_dir().join(format!("eggie-feed-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("feed.json");
        let doc = format!(
            "{{\"releases\":[{}]}}",
            release_json(json!({ "sha256": "nope" }))
        );
        std::fs::write(&path, doc).unwrap();
        assert!(FileFeed::new(path).fetch_releases().is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_feed_missing_file_errors() {
        let feed = FileFeed::new(PathBuf::from("/nonexistent/eggie-feed.json"));
        assert!(feed.fetch_releases().is_err());
    }

    #[test]
    fn feed_from_url_dispatches_on_scheme() {
        let file = Url::parse("file:///tmp/feed.json").unwrap();
        assert!(feed_from_url(&file).fetch_releases().is_err()); // missing file, but it is a FileFeed
        let http = Url::parse("https://example.com/feed.json").unwrap();
        let _ = feed_from_url(&http); // HttpFeed construction must not panic
    }
}
