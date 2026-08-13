//! In-memory update state machine.
//!
//! The state is intentionally not persisted: a restart starts back at
//! [`UpdateState::Idle`] and the next check re-discovers the release.

use std::path::PathBuf;

use crate::download::DownloadProgress;
use crate::feed::ReleaseInfo;

/// Where the updater is in its lifecycle.
#[derive(Clone, Debug)]
pub enum UpdateState {
    /// Nothing pending.
    Idle,
    /// A check is in flight.
    Checking,
    /// A newer release was found; waiting for the user to start the download.
    Available(ReleaseInfo),
    /// Download in progress.
    Downloading {
        release: ReleaseInfo,
        progress: DownloadProgress,
    },
    /// Download finished and verified; waiting for the user to restart.
    Ready {
        release: ReleaseInfo,
        package: PathBuf,
    },
    /// A manual check found no newer release. Transient: dismiss back to idle.
    UpToDate,
    /// Something went wrong, with a human-readable message.
    Error(String),
}

impl UpdateState {
    /// True if this state represents an actionable update (available,
    /// downloading, or ready to install).
    pub fn is_update_pending(&self) -> bool {
        matches!(
            self,
            UpdateState::Available(_) | UpdateState::Downloading { .. } | UpdateState::Ready { .. }
        )
    }

    /// The release this state is about, if any.
    pub fn release(&self) -> Option<&ReleaseInfo> {
        match self {
            UpdateState::Available(release)
            | UpdateState::Downloading { release, .. }
            | UpdateState::Ready { release, .. } => Some(release),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::ReleaseInfo;
    use semver::Version;
    use url::Url;

    fn release() -> ReleaseInfo {
        ReleaseInfo {
            version: Version::new(0, 0, 2),
            protocol_version: 1,
            release_notes: String::new(),
            published_at: String::new(),
            download_url: Url::parse("file:///tmp/x.zip").unwrap(),
            sha256: "a".repeat(64),
        }
    }

    #[test]
    fn pending_states_track_release() {
        assert!(!UpdateState::Idle.is_update_pending());
        assert!(!UpdateState::Checking.is_update_pending());
        assert!(UpdateState::Available(release()).is_update_pending());
        assert!(UpdateState::Ready {
            release: release(),
            package: PathBuf::from("/tmp/x.zip"),
        }
        .is_update_pending());
        assert!(UpdateState::Error("boom".into()).release().is_none());
        assert_eq!(
            UpdateState::Available(release()).release().unwrap().version,
            Version::new(0, 0, 2)
        );
    }
}
