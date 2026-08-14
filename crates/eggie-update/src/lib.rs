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

/// The result of evaluating a feed's releases against the running version:
/// the release to install (the newest available) with its notes rewritten to
/// the aggregated changelog of every version the user is behind.
///
/// Because we ship whole-bundle replacements (not per-version deltas), the
/// install target is always the single newest release; the intermediate
/// versions only contribute their notes. Daemon compatibility is therefore a
/// straight comparison between the target's protocol version and the running
/// daemon's — intermediate versions never run.
pub fn select_update(current_version: &str, releases: &[ReleaseInfo]) -> Option<ReleaseInfo> {
    let current = Version::parse(current_version).ok();

    // Newer-than-current releases, newest first.
    let mut newer: Vec<&ReleaseInfo> = releases
        .iter()
        .filter(|release| match &current {
            Some(current) => &release.version > current,
            None => true,
        })
        .collect();
    newer.sort_by(|a, b| b.version.cmp(&a.version));

    let target = newer.first().copied()?.clone();

    // Aggregate notes across every version the user is behind, newest first,
    // each under a version heading. A single pending version keeps its notes
    // verbatim (no redundant heading).
    let aggregated_notes = if newer.len() == 1 {
        target.release_notes.clone()
    } else {
        newer
            .iter()
            .map(|release| {
                let notes = release.release_notes.trim();
                format!("## {}\n\n{}", release.version, notes)
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    Some(ReleaseInfo {
        release_notes: aggregated_notes,
        ..target
    })
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

/// Base URL of the published feed, served from the `daidr/eggie` repository's
/// GitHub Pages site (built and deployed by the release workflow, bound to the
/// custom domain `eggie-pages.daidr.me`). The per-channel feed documents live
/// at `{base}/{channel}.json`.
pub const FEED_BASE_URL: &str = "https://eggie-pages.daidr.me";

/// The feed URL for a release channel (`"stable"` / `"beta"`, i.e.
/// `UpdateChannel::slug()`).
///
/// `EGGIE_UPDATE_FEED` overrides everything with an explicit URL — the channel
/// is ignored — which is how local mock feeds are tested in development
/// (`export EGGIE_UPDATE_FEED=file:///…/update-feed.json`). Without the
/// override the published per-channel feed is used.
pub fn default_feed_url(channel: &str) -> Result<Url> {
    if let Ok(custom) = std::env::var("EGGIE_UPDATE_FEED") {
        return Ok(Url::parse(&custom)?);
    }
    Ok(Url::parse(&format!("{FEED_BASE_URL}/{channel}.json"))?)
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

    fn release_with_notes(version: &str, notes: &str) -> ReleaseInfo {
        ReleaseInfo {
            release_notes: notes.to_string(),
            ..release(version)
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
    fn select_update_none_when_up_to_date() {
        let releases = vec![release("0.0.1"), release("0.0.2")];
        assert!(select_update("0.0.2", &releases).is_none());
        assert!(select_update("0.0.3", &releases).is_none());
    }

    #[test]
    fn select_update_targets_newest_regardless_of_order() {
        // Deliberately unsorted; target must be the highest version.
        let releases = vec![release("0.0.2"), release("0.1.0"), release("0.0.5")];
        let target = select_update("0.0.1", &releases).unwrap();
        assert_eq!(target.version, Version::new(0, 1, 0));
    }

    #[test]
    fn select_update_single_version_keeps_notes_verbatim() {
        let releases = vec![release_with_notes("0.0.2", "- one fix")];
        let target = select_update("0.0.1", &releases).unwrap();
        assert_eq!(target.release_notes, "- one fix");
    }

    #[test]
    fn select_update_aggregates_notes_newest_first() {
        let releases = vec![
            release_with_notes("0.0.2", "- v2 notes"),
            release_with_notes("0.0.4", "- v4 notes"),
            release_with_notes("0.0.3", "- v3 notes"),
        ];
        // User on 0.0.1 is behind by 0.0.2/0.0.3/0.0.4.
        let target = select_update("0.0.1", &releases).unwrap();
        assert_eq!(target.version, Version::new(0, 0, 4));
        let notes = target.release_notes;
        // Newest first, each under a version heading.
        assert!(notes.find("## 0.0.4").unwrap() < notes.find("## 0.0.3").unwrap());
        assert!(notes.find("## 0.0.3").unwrap() < notes.find("## 0.0.2").unwrap());
        assert!(notes.contains("- v4 notes") && notes.contains("- v2 notes"));
    }

    #[test]
    fn select_update_only_aggregates_versions_user_is_behind() {
        let releases = vec![
            release_with_notes("0.0.2", "- old, already installed"),
            release_with_notes("0.0.3", "- new"),
        ];
        // User already on 0.0.2 should only see 0.0.3's notes (single version → verbatim).
        let target = select_update("0.0.2", &releases).unwrap();
        assert_eq!(target.version, Version::new(0, 0, 3));
        assert_eq!(target.release_notes, "- new");
    }

    #[test]
    fn feed_url_defaults_to_channel_and_env_overrides() {
        // SAFETY: single test touching this env var, no parallel access.
        unsafe {
            std::env::remove_var("EGGIE_UPDATE_FEED");
            let stable = default_feed_url("stable").unwrap();
            assert_eq!(stable.scheme(), "https");
            assert!(stable.as_str().ends_with("/stable.json"));
            let beta = default_feed_url("beta").unwrap();
            assert!(beta.as_str().ends_with("/beta.json"));

            std::env::set_var("EGGIE_UPDATE_FEED", "file:///tmp/mock-feed.json");
            // The override wins regardless of channel.
            let url = default_feed_url("stable").unwrap();
            assert_eq!(url.as_str(), "file:///tmp/mock-feed.json");
            std::env::remove_var("EGGIE_UPDATE_FEED");
        }
    }
}
