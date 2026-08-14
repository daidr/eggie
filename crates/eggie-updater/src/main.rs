//! Standalone updater for Eggie.
//!
//! Spawned by the app right before it quits, this binary waits for the app
//! process to exit, optionally terminates the old daemon, then atomically
//! replaces the app bundle with the downloaded package and relaunches.
//!
//! Usage:
//!   eggie-updater --app-path <Eggie.app> --package <zip> --app-pid <pid>
//!                 [--daemon-socket <path>] [--relaunch]
//!
//! The package is a zip of the whole `.app` (as produced by
//! `scripts/make-update-zip.sh`). Every step is logged to
//! `~/Library/Application Support/Eggie/updates/updater.log`.

use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, exit};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    WaitForAppExit,
    TerminateDaemon,
    Backup,
    Extract,
    Verify,
    Swap,
    Register,
    Relaunch,
    Cleanup,
}

/// What to do after a failed phase. Pure so the rollback policy is testable.
fn rollback_for(phase: Phase) -> Rollback {
    match phase {
        Phase::WaitForAppExit | Phase::TerminateDaemon => Rollback::NothingYet,
        Phase::Backup | Phase::Extract | Phase::Verify => Rollback::RemoveNew,
        Phase::Swap | Phase::Register | Phase::Relaunch | Phase::Cleanup => {
            Rollback::RestoreBackup
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Rollback {
    /// Nothing was touched yet.
    NothingYet,
    /// The old app is backed up but not replaced: drop the partial new app.
    RemoveNew,
    /// The new app is in place: swap the backup back.
    RestoreBackup,
}

struct Args {
    app_path: PathBuf,
    package: PathBuf,
    app_pid: i32,
    daemon_socket: Option<PathBuf>,
    relaunch: bool,
}

fn parse_args() -> Result<Args> {
    parse_args_from(std::env::args_os().skip(1))
}

fn parse_args_from(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Args> {
    let mut app_path = None;
    let mut package = None;
    let mut app_pid = None;
    let mut daemon_socket = None;
    let mut relaunch = false;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.to_string_lossy().as_ref() {
            "--app-path" => app_path = Some(PathBuf::from(iter.next().context("--app-path needs a value")?)),
            "--package" => package = Some(PathBuf::from(iter.next().context("--package needs a value")?)),
            "--app-pid" => {
                let value = iter.next().context("--app-pid needs a value")?;
                app_pid = Some(value.to_string_lossy().parse().context("invalid --app-pid")?);
            }
            "--daemon-socket" => {
                daemon_socket = Some(PathBuf::from(iter.next().context("--daemon-socket needs a value")?))
            }
            "--relaunch" => relaunch = true,
            other => bail!("unknown argument: {other}"),
        }
    }

    Ok(Args {
        app_path: app_path.context("--app-path is required")?,
        package: package.context("--package is required")?,
        app_pid: app_pid.context("--app-pid is required")?,
        daemon_socket,
        relaunch,
    })
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("eggie-updater: {error:#}");
            exit(2);
        }
    };

    if let Err(error) = run(&args) {
        log_line(&format!("FAILED: {error:#}"));
        exit(1);
    }
    log_line("done");
}

fn run(args: &Args) -> Result<()> {
    let mut phase = Phase::WaitForAppExit;
    let result = run_phases(args, &mut phase);
    if let Err(ref error) = result {
        log_line(&format!("phase {:?} failed: {error:#}", phase));
        match rollback_for(phase) {
            Rollback::NothingYet => {}
            Rollback::RemoveNew => {
                let new = new_app_path(&args.app_path);
                if new.exists() {
                    let _ = fs::remove_dir_all(&new);
                    log_line(&format!("removed partial {}", new.display()));
                }
            }
            Rollback::RestoreBackup => {
                let backup = backup_path(&args.app_path);
                let new = new_app_path(&args.app_path);
                if new.exists() {
                    let _ = fs::remove_dir_all(&new);
                }
                if backup.exists() {
                    let _ = fs::rename(&backup, &args.app_path);
                    log_line(&format!(
                        "restored previous app from {}",
                        backup.display()
                    ));
                }
            }
        }
    }
    result
}

fn run_phases(args: &Args, phase: &mut Phase) -> Result<()> {
    *phase = Phase::WaitForAppExit;
    wait_for_exit(args.app_pid)?;

    if let Some(socket) = &args.daemon_socket {
        *phase = Phase::TerminateDaemon;
        log_line(&format!("terminating daemon at {}", socket.display()));
        eggie_daemon::terminate_daemon_at(socket);
    }

    *phase = Phase::Backup;
    let backup = backup_path(&args.app_path);
    let new = new_app_path(&args.app_path);
    if backup.exists() {
        fs::remove_dir_all(&backup).context("removing stale backup")?;
    }
    if new.exists() {
        fs::remove_dir_all(&new).context("removing stale new app")?;
    }
    fs::rename(&args.app_path, &backup)
        .with_context(|| format!("backing up {} to {}", args.app_path.display(), backup.display()))?;
    log_line("backed up old app");

    *phase = Phase::Extract;
    let staging = staging_path(&args.app_path);
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    extract_zip(&args.package, &staging)?;
    let extracted = find_app_bundle(&staging)?;
    fs::rename(&extracted, &new).with_context(|| {
        format!(
            "moving extracted bundle to {}",
            new.display()
        )
    })?;
    let _ = fs::remove_dir_all(&staging);
    log_line("extracted new app");

    *phase = Phase::Verify;
    let executable = new.join("Contents/MacOS/eggie");
    if !executable.is_file() {
        bail!("new app is missing {}", executable.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&executable)?.permissions().mode() & 0o111 == 0 {
            bail!("new app executable is not executable");
        }
    }

    *phase = Phase::Swap;
    fs::rename(&new, &args.app_path)
        .with_context(|| format!("swapping in {}", args.app_path.display()))?;
    log_line("swapped in new app");

    // Deliberately no re-signing here. The downloaded package is a whole-bundle
    // replacement whose signature (and, for released builds, its stapled
    // notarization ticket) is embedded in the binary and
    // `_CodeSignature/CodeResources`. `extract_zip` restores every file
    // faithfully, so the seal survives intact. Re-signing ad-hoc (`--sign -`)
    // here would strip the Developer ID signature and staple, leaving a
    // notarized app "de-notarized" after its first self-update. Neither ureq's
    // download nor the zip extraction sets `com.apple.quarantine`, so Gatekeeper
    // does not block the relaunch.

    *phase = Phase::Register;
    let _ = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
        .arg("-f")
        .arg(&args.app_path)
        .status();

    if args.relaunch {
        *phase = Phase::Relaunch;
        let status = Command::new("open")
            .arg(&args.app_path)
            .status()
            .context("spawning open")?;
        if !status.success() {
            bail!("open failed with {status}");
        }
        log_line("relaunched app");
    }

    *phase = Phase::Cleanup;
    let _ = fs::remove_dir_all(&backup);
    let _ = fs::remove_file(&args.package);
    Ok(())
}

fn backup_path(app_path: &Path) -> PathBuf {
    app_path.with_extension("app.bak")
}

fn new_app_path(app_path: &Path) -> PathBuf {
    app_path.with_extension("app.new")
}

fn staging_path(app_path: &Path) -> PathBuf {
    app_path.with_extension("app.extract")
}

/// Poll `kill(pid, 0)` until the process exits or the timeout elapses.
fn wait_for_exit(pid: i32) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if unsafe { libc::kill(pid, 0) } != 0 {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for app (pid {pid}) to exit");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Extract a zip into `dest`, restoring unix permissions and symlinks.
fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file = File::open(zip_path)
        .with_context(|| format!("opening package {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("reading zip archive")?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("reading zip entry {index}"))?;
        let relative = entry
            .enclosed_name()
            .with_context(|| format!("unsafe path in zip entry {index}"))?
            .to_path_buf();

        // Skip macOS archive cruft: `__MACOSX/` sidecar trees and AppleDouble
        // `._*` files. Left in place they land in the bundle root and make
        // codesign reject the bundle ("unsealed contents present").
        if relative.components().any(|component| {
            let name = component.as_os_str().to_string_lossy();
            name == "__MACOSX" || name.starts_with("._")
        }) {
            continue;
        }

        let out_path = dest.join(&relative);

        let mode = entry.unix_mode();
        let is_symlink = mode
            .map(|mode| mode & 0o170_000 == 0o120_000)
            .unwrap_or(false);

        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }

        if is_symlink {
            let mut target = String::new();
            entry.read_to_string(&mut target)?;
            let _ = fs::remove_file(&out_path);
            symlink(Path::new(&target), &out_path).with_context(|| {
                format!("symlinking {} -> {}", out_path.display(), target)
            })?;
            continue;
        }

        let mut output = File::create(&out_path)
            .with_context(|| format!("creating {}", out_path.display()))?;
        io::copy(&mut entry, &mut output)?;
        if let Some(mode) = mode {
            fs::set_permissions(&out_path, fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

/// Find the single `.app` bundle at the top level of an extracted tree.
fn find_app_bundle(dir: &Path) -> Result<PathBuf> {
    let mut found = None;
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().map(|ext| ext == "app").unwrap_or(false) {
            if found.is_some() {
                bail!("package contains multiple .app bundles");
            }
            found = Some(path);
        }
    }
    found.context("package contains no .app bundle")
}

fn log_line(message: &str) {
    let mut path = eggie_update_path();
    let _ = fs::create_dir_all(&path);
    path.push("updater.log");
    let line = format!(
        "{} {message}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = io::Write::write_all(&mut file, line.as_bytes());
    }
    eprintln!("{message}");
}

/// Updates working directory, duplicated from `eggie_update::updates_dir`
/// to avoid a dependency on the app crate.
fn eggie_update_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home {
            return home.join("Library/Application Support/Eggie/updates");
        }
    }
    if let Some(home) = home {
        return home.join(".config/eggie/updates");
    }
    PathBuf::from("updates")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_policy_tracks_phase() {
        assert_eq!(rollback_for(Phase::WaitForAppExit), Rollback::NothingYet);
        assert_eq!(rollback_for(Phase::TerminateDaemon), Rollback::NothingYet);
        assert_eq!(rollback_for(Phase::Backup), Rollback::RemoveNew);
        assert_eq!(rollback_for(Phase::Extract), Rollback::RemoveNew);
        assert_eq!(rollback_for(Phase::Verify), Rollback::RemoveNew);
        assert_eq!(rollback_for(Phase::Swap), Rollback::RestoreBackup);
        assert_eq!(rollback_for(Phase::Register), Rollback::RestoreBackup);
        assert_eq!(rollback_for(Phase::Relaunch), Rollback::RestoreBackup);
    }

    #[test]
    fn parses_required_args() {
        let args = parse_args_from([
            "--app-path",
            "/Applications/Eggie.app",
            "--package",
            "/tmp/pkg.zip",
            "--app-pid",
            "42",
            "--daemon-socket",
            "/tmp/d.sock",
            "--relaunch",
        ]
        .into_iter()
        .map(std::ffi::OsString::from))
        .unwrap();
        assert_eq!(args.app_path, PathBuf::from("/Applications/Eggie.app"));
        assert_eq!(args.package, PathBuf::from("/tmp/pkg.zip"));
        assert_eq!(args.app_pid, 42);
        assert_eq!(args.daemon_socket, Some(PathBuf::from("/tmp/d.sock")));
        assert!(args.relaunch);
    }

    #[test]
    fn missing_required_arg_errors() {
        let result = parse_args_from(
            ["--app-path", "/x.app"]
                .into_iter()
                .map(std::ffi::OsString::from),
        );
        assert!(result.is_err());
    }

    #[test]
    fn finds_single_app_bundle() {
        let dir = std::env::temp_dir().join(format!("eggie-updater-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("Eggie.app/Contents/MacOS")).unwrap();
        fs::write(dir.join("Eggie.app/Contents/MacOS/eggie"), b"binary").unwrap();
        let found = find_app_bundle(&dir).unwrap();
        assert_eq!(found.file_name().unwrap(), "Eggie.app");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_multiple_app_bundles() {
        let dir = std::env::temp_dir().join(format!("eggie-updater-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("A.app")).unwrap();
        fs::create_dir_all(dir.join("B.app")).unwrap();
        assert!(find_app_bundle(&dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extracts_zip_with_permissions() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("eggie-updater-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("pkg.zip");

        // Build a zip with an executable file. Note: zip 0.6 masks out the
        // file-type bits when writing, so a symlink entry can't be crafted
        // here; symlink extraction relies on the unmasked read path and is
        // exercised by real `ditto`-built packages.
        {
            let file = File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::FileOptions::default().unix_permissions(0o755);
            writer
                .start_file("Eggie.app/Contents/MacOS/eggie", options)
                .unwrap();
            writer.write_all(b"#!/bin/sh\n").unwrap();
            writer.finish().unwrap();
        }

        let dest = dir.join("out");
        extract_zip(&zip_path, &dest).unwrap();
        let exe = dest.join("Eggie.app/Contents/MacOS/eggie");
        assert!(exe.is_file());
        assert_eq!(fs::metadata(&exe).unwrap().permissions().mode() & 0o111, 0o111);

        fs::remove_dir_all(&dir).unwrap();
    }
}
