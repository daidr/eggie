//! App-level update orchestrator: checks the feed, downloads releases, and
//! hands off to the `eggie-updater` binary for the actual swap.
//!
//! One entity per app, created in [`EggieApp::launch`]; every window holds a
//! handle so the sidebar indicator stays in sync across windows.

use std::process::{Command, Stdio};

use anyhow::{Result, bail};
use anyhow::Context as _;
use gpui::{Context, Task};
use eggie_update::{UpdateState, feed_from_url, select_update, updates_dir};

pub(crate) struct UpdateController {
    state: UpdateState,
    current_version: String,
    /// Set while a check or download is in flight so repeated triggers
    /// don't pile up.
    check_task: Option<Task<()>>,
    download_task: Option<Task<()>>,
}

impl UpdateController {
    pub(crate) fn new() -> Self {
        Self {
            state: UpdateState::Idle,
            current_version: env!("CARGO_PKG_VERSION").to_owned(),
            check_task: None,
            download_task: None,
        }
    }

    pub(crate) fn state(&self) -> &UpdateState {
        &self.state
    }

    /// Check the feed for a newer release on the given `channel`
    /// (`UpdateChannel::slug()`). Building the feed per-check means a runtime
    /// channel change is picked up on the next check with no extra plumbing.
    /// `silent` checks (automatic, on launch) stay quiet on failure; manual
    /// checks surface errors.
    pub(crate) fn check(&mut self, silent: bool, channel: &str, cx: &mut Context<Self>) {
        if self.check_task.is_some() || matches!(self.state, UpdateState::Downloading { .. }) {
            return;
        }
        let feed = match eggie_update::default_feed_url(channel) {
            Ok(url) => feed_from_url(&url),
            Err(error) => {
                let message = format!("invalid update feed URL: {error:#}");
                if silent {
                    eprintln!("{message}");
                    self.state = UpdateState::Idle;
                } else {
                    self.state = UpdateState::Error(message);
                }
                cx.notify();
                return;
            }
        };
        self.state = UpdateState::Checking;
        cx.notify();

        let executor = cx.background_executor().clone();
        let current_version = self.current_version.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = executor.spawn(async move { feed.fetch_releases() }).await;
            this.update(cx, |controller, cx| {
                controller.check_task = None;
                match result {
                    Ok(releases) => match select_update(&current_version, &releases) {
                        Some(release) => {
                            controller.state = UpdateState::Available(release);
                        }
                        None => {
                            controller.state = if silent {
                                UpdateState::Idle
                            } else {
                                UpdateState::UpToDate
                            };
                        }
                    },
                    Err(error) => {
                        if !silent {
                            controller.state = UpdateState::Error(format!("{error:#}"));
                        } else {
                            eprintln!("automatic update check failed: {error:#}");
                            controller.state = UpdateState::Idle;
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.check_task = Some(task);
    }

    /// Start downloading the currently available release. `channel`
    /// (`UpdateChannel::slug()`) is only used by the error-retry path, which
    /// re-checks the feed.
    pub(crate) fn start_download(&mut self, channel: &str, cx: &mut Context<Self>) {
        if self.download_task.is_some() {
            return;
        }
        let release = match &self.state {
            UpdateState::Available(release) => release.clone(),
            UpdateState::Error(_) | UpdateState::UpToDate => {
                // Retry from an error: re-check first.
                self.check(false, channel, cx);
                return;
            }
            _ => return,
        };

        let package = updates_dir().join(format!("Eggie-{}.zip", release.version));
        self.state = UpdateState::Downloading {
            release: release.clone(),
            progress: Default::default(),
        };
        cx.notify();

        let executor = cx.background_executor().clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<eggie_update::DownloadProgress>();
        let download_package = package.clone();
        let download_url = release.download_url.clone();
        let sha256 = release.sha256.clone();
        let download = executor.spawn(async move {
            let result = eggie_update::download::download(
                &download_url,
                &download_package,
                &sha256,
                |progress| {
                    let _ = tx.unbounded_send(progress);
                },
            );
            drop(tx);
            result
        });

        let task = cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some(progress) = rx.next().await {
                this.update(cx, |controller, cx| {
                    if let UpdateState::Downloading { progress: slot, .. } = &mut controller.state {
                        *slot = progress;
                        cx.notify();
                    }
                })
                .ok();
            }
            let result = download.await;
            this.update(cx, |controller, cx| {
                controller.download_task = None;
                match result {
                    Ok(()) => {
                        if let UpdateState::Downloading { release, .. } = &controller.state {
                            controller.state = UpdateState::Ready {
                                release: release.clone(),
                                package: package.clone(),
                            };
                        }
                    }
                    Err(error) => {
                        controller.state = UpdateState::Error(format!("{error:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.download_task = Some(task);
    }

    /// Spawn the updater binary and quit, letting it swap the app bundle.
    /// Only valid in the [`UpdateState::Ready`] state.
    pub(crate) fn install_and_restart(&mut self, cx: &mut Context<Self>) -> Result<()> {
        let (release, package) = match &self.state {
            UpdateState::Ready { release, package } => (release.clone(), package.clone()),
            _ => bail!("update is not ready to install"),
        };

        let current_exe = std::env::current_exe().context("cannot locate Eggie executable")?;
        let Some(app_path) = current_exe.ancestors().nth(3).map(|path| path.to_path_buf())
        else {
            bail!("cannot derive .app path from {}", current_exe.display());
        };
        if app_path.extension().and_then(|ext| ext.to_str()) != Some("app") {
            bail!(
                "Eggie is not running from a .app bundle ({}); in-app update requires an installed app",
                current_exe.display()
            );
        }
        let updater = current_exe
            .parent()
            .context("executable has no parent directory")?
            .join("eggie-updater");
        if !updater.exists() {
            bail!(
                "updater binary not found at {}; rebuild the app bundle with make-app.sh",
                updater.display()
            );
        }

        let mut command = Command::new(updater);
        command
            .arg("--app-path")
            .arg(&app_path)
            .arg("--package")
            .arg(&package)
            .arg("--app-pid")
            .arg(std::process::id().to_string());
        if !release.daemon_compatible() {
            command
                .arg("--daemon-socket")
                .arg(eggie_daemon::daemon_socket_path());
        }
        command.arg("--relaunch");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().context("failed to spawn updater")?;

        cx.quit();
        Ok(())
    }

    /// Surface an install error without leaving the Ready state (the user
    /// can retry from the update window).
    pub(crate) fn report_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.state = UpdateState::Error(message);
        cx.notify();
    }

    /// Dismiss a transient state (error / up-to-date), back to idle.
    pub(crate) fn dismiss(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.state,
            UpdateState::Error(_) | UpdateState::UpToDate
        ) {
            self.state = UpdateState::Idle;
            cx.notify();
        }
    }
}
