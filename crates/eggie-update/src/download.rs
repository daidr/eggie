//! Streaming download with SHA-256 verification and speed sampling.
//!
//! All functions here are blocking; callers must run them on a background
//! executor (see `cx.background_executor()` in eggie-ui).

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use url::Url;

/// Progress snapshot for a download.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DownloadProgress {
    /// Bytes downloaded so far.
    pub downloaded: u64,
    /// Total size in bytes, if the server (or file metadata) provided it.
    pub total: Option<u64>,
    /// Smoothed download speed in bytes per second.
    pub bytes_per_sec: u64,
}

/// Sliding-window speed sampler: keeps samples from the last ~1.5s and
/// computes bytes/sec across the window.
#[derive(Debug)]
struct SpeedSampler {
    started: Instant,
    samples: Vec<(Instant, u64)>,
}

impl SpeedSampler {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            samples: Vec::with_capacity(16),
        }
    }

    fn sample(&mut self, downloaded: u64) -> u64 {
        let now = Instant::now();
        self.samples.push((now, downloaded));
        let window = Duration::from_millis(1500);
        while self.samples.len() > 2 && now.duration_since(self.samples[0].0) > window {
            self.samples.remove(0);
        }
        let (first_time, first_bytes) = self.samples[0];
        let elapsed = now.duration_since(first_time).as_secs_f64();
        if elapsed <= 0.05 {
            // Too early to tell; fall back to lifetime average.
            let total_elapsed = now.duration_since(self.started).as_secs_f64();
            if total_elapsed > 0.05 {
                return (downloaded as f64 / total_elapsed) as u64;
            }
            return 0;
        }
        ((downloaded - first_bytes) as f64 / elapsed) as u64
    }
}

/// Download `url` to `dest`, verifying the SHA-256 of the result.
///
/// `on_progress` is throttled to roughly 5 Hz. On hash mismatch (or any
/// error) the partial file is removed.
pub fn download(
    url: &Url,
    dest: &Path,
    expected_sha256: &str,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create download directory {}", parent.display()))?;
    }
    let part = part_path(dest);
    let result = download_inner(url, &part, &mut on_progress);
    match result {
        Ok(()) => {
            let actual = hash_file(&part)?;
            if !actual.eq_ignore_ascii_case(expected_sha256) {
                let _ = std::fs::remove_file(&part);
                bail!("sha256 mismatch for {url}: expected {expected_sha256}, got {actual}");
            }
            std::fs::rename(&part, dest).with_context(|| {
                format!("failed to move download into place: {}", dest.display())
            })?;
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(error)
        }
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    dest.with_file_name(name)
}

fn download_inner(
    url: &Url,
    part: &Path,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<()> {
    let mut reader: Box<dyn Read> = match url.scheme() {
        "file" => {
            let path = url
                .to_file_path()
                .map_err(|_| anyhow::anyhow!("invalid file URL: {url}"))?;
            Box::new(
                File::open(&path).with_context(|| format!("failed to open {}", path.display()))?,
            )
        }
        "http" | "https" => {
            let response = ureq::get(url.as_str())
                .call()
                .with_context(|| format!("failed to download {url}"))?;
            Box::new(response.into_reader())
        }
        other => bail!("unsupported download URL scheme: {other}"),
    };

    // Total size is only known up front for local files.
    let total = match url.scheme() {
        "file" => url
            .to_file_path()
            .ok()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len()),
        _ => None,
    };

    let mut output = File::create(part)
        .with_context(|| format!("failed to create {}", part.display()))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut downloaded = 0u64;
    let mut sampler = SpeedSampler::new();
    let mut last_callback = Instant::now() - Duration::from_secs(1);
    let callback_interval = Duration::from_millis(200);

    // Development aid: a local `file://` "download" completes almost instantly,
    // so the progress UI never gets a chance to render. Setting
    // EGGIE_UPDATE_THROTTLE_MS sleeps that many ms per 64 KiB chunk so the mock
    // feed exercises the downloading UI at a visible pace. No effect on https.
    let throttle = std::env::var("EGGIE_UPDATE_THROTTLE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|_| url.scheme() == "file")
        .map(Duration::from_millis);

    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        output.write_all(&buffer[..n])?;
        downloaded += n as u64;

        if let Some(delay) = throttle {
            std::thread::sleep(delay);
        }

        let now = Instant::now();
        if now.duration_since(last_callback) >= callback_interval {
            last_callback = now;
            on_progress(DownloadProgress {
                downloaded,
                total,
                bytes_per_sec: sampler.sample(downloaded),
            });
        }
    }
    output.flush()?;
    Ok(())
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to hash {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(path: &Path, contents: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn sha256_hex(contents: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(contents);
        format!("{:x}", hasher.finalize())
    }

    #[test]
    fn downloads_local_file_and_verifies_hash() {
        let dir = std::env::temp_dir().join(format!("eggie-dl-test-{}", uuid::Uuid::new_v4()));
        let source = dir.join("source.zip");
        let dest = dir.join("dest.zip");
        let contents = b"fake update package";
        write_file(&source, contents);

        let url = Url::from_file_path(&source).unwrap();
        let mut last = None;
        download(&url, &dest, &sha256_hex(contents), |p| last = Some(p)).unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), contents);
        assert_eq!(last.unwrap().downloaded, contents.len() as u64);
        assert!(!part_path(&dest).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hash_mismatch_removes_partial_file() {
        let dir = std::env::temp_dir().join(format!("eggie-dl-test-{}", uuid::Uuid::new_v4()));
        let source = dir.join("source.zip");
        let dest = dir.join("dest.zip");
        write_file(&source, b"contents");

        let url = Url::from_file_path(&source).unwrap();
        let result = download(&url, &dest, &"0".repeat(64), |_| {});
        assert!(result.is_err());
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_source_errors() {
        let url = Url::parse("file:///nonexistent/package.zip").unwrap();
        let result = download(&url, Path::new("/tmp/eggie-dest.zip"), &"0".repeat(64), |_| {});
        assert!(result.is_err());
        let _ = std::fs::remove_file("/tmp/eggie-dest.zip");
    }

    #[test]
    fn speed_sampler_reports_zero_when_stalled() {
        let mut sampler = SpeedSampler::new();
        assert_eq!(sampler.sample(0), 0);
        assert_eq!(sampler.sample(0), 0);
    }
}
