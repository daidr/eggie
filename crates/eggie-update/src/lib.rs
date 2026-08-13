//! Application self-update support: release feeds, downloading, and the
//! update state machine. The installer binary lives in the `eggie-updater`
//! crate; this crate holds everything the app needs to discover and fetch
//! updates.

pub mod download;
pub mod feed;
pub mod state;

pub use download::DownloadProgress;
pub use feed::{FileFeed, HttpFeed, ReleaseInfo, UpdateFeed, feed_from_url};
pub use state::UpdateState;

use std::path::PathBuf;

use anyhow::Result;
use semver::Version;
use url::Url;

/// True if `release` is newer than the currently running `current_version`.
pub fn is_update_available(current_version: &str, release: &ReleaseInfo) -> bool {
    match Version::parse(current_version) {
        Ok(current) => release.version > current,
        // Unparseable current version (shouldn't happen) never blocks an update.
        Err(_) => true,
    }
}

/// Per-user application data directory:
/// `~/Library/Application Support/Eggie` on macOS, `~/.config/eggie` elsewhere.
///
/// Shared with settings and project storage; keep in sync with the ad-hoc
/// implementations in `eggie-ui` (settings.rs / project_store.rs).
pub fn app_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home_dir() {
            return home.join("Library/Application Support/Eggie");
        }
    }
    if let Some(home) = home_dir() {
        return home.join(".config/eggie");
    }
    PathBuf::from(".")
}

/// Working directory for update downloads and updater logs.
pub fn updates_dir() -> PathBuf {
    app_data_dir().join("updates")
}

/// Where the mock feed JSON lives by default.
pub fn default_feed_path() -> PathBuf {
    app_data_dir().join("dev/update-feed.json")
}

/// The feed URL to use: the `EGGIE_UPDATE_FEED` environment variable if set,
/// otherwise the local mock feed. Once the GitHub repository exists this
/// becomes a real HTTPS URL.
pub fn default_feed_url() -> Result<Url> {
    if let Ok(custom) = std::env::var("EGGIE_UPDATE_FEED") {
        return Ok(Url::parse(&custom)?);
    }
    Ok(Url::from_file_path(default_feed_path()).unwrap())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> ReleaseInfo {
        ReleaseInfo {
            version: Version::parse(version).unwrap(),
            protocol_version: 1,
            release_notes: String::new(),
            published_at: String::new(),
            download_url: Url::parse("file:///tmp/x.zip").unwrap(),
            sha256: "a".repeat(64),
        }
    }

    #[test]
    fn compares_prerelease_versions() {
        assert!(is_update_available("0.0.1-alpha.1", &release("0.0.1-alpha.2")));
        assert!(is_update_available("0.0.1-alpha.2", &release("0.0.1")));
        assert!(is_update_available("0.0.1", &release("0.0.2")));
        assert!(is_update_available("0.0.1", &release("0.1.0")));
        assert!(!is_update_available("0.0.2", &release("0.0.2")));
        assert!(!is_update_available("0.0.2", &release("0.0.1")));
        assert!(!is_update_available("0.0.1-alpha.9", &release("0.0.1-alpha.1")));
    }

    #[test]
    fn unparseable_current_version_never_blocks() {
        assert!(is_update_available("not-a-version", &release("0.0.1")));
    }

    #[test]
    fn feed_url_defaults_to_mock_file_and_env_overrides() {
        // SAFETY: single test touching this env var, no parallel access.
        unsafe {
            std::env::remove_var("EGGIE_UPDATE_FEED");
            let url = default_feed_url().unwrap();
            assert_eq!(url.scheme(), "file");
            assert!(url.path().ends_with("Eggie/dev/update-feed.json"));

            std::env::set_var("EGGIE_UPDATE_FEED", "https://example.com/feed.json");
            let url = default_feed_url().unwrap();
            assert_eq!(url.as_str(), "https://example.com/feed.json");
            std::env::remove_var("EGGIE_UPDATE_FEED");
        }
    }
}
