//! Bound SSH helper lifetimes without disturbing the terminal's authenticated master.
use super::*;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};

fn read_limited(mut reader: impl Read, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let retained = read.min(limit.saturating_sub(data.len()));
        data.extend_from_slice(&buffer[..retained]);
    }
    Ok(data)
}
pub(super) fn run(mut command: Command, timeout: Duration) -> Result<Output> {
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run ssh")?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out_done = Arc::new(AtomicBool::new(false));
    let err_done = Arc::new(AtomicBool::new(false));
    let done = out_done.clone();
    let out = thread::spawn(move || {
        let result = read_limited(stdout, 4 * 1024 * 1024);
        done.store(true, Ordering::Release);
        result
    });
    let done = err_done.clone();
    let err = thread::spawn(move || {
        let result = read_limited(stderr, 8192);
        done.store(true, Ordering::Release);
        result
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => {}
            result => {
                // ProxyCommand helpers inherit this private group; killing only ssh could leave
                // their output pipes open and prevent the timeout from returning.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.wait();
                break Err(match result {
                    Err(error) => anyhow!(error),
                    _ => anyhow!("SSH request timed out"),
                });
            }
        }
        thread::sleep(Duration::from_millis(10));
    };
    // A proxy may retain the pipes after ssh exits; apply the same deadline to draining them.
    let mut drain_timed_out = false;
    while !out_done.load(Ordering::Acquire) || !err_done.load(Ordering::Acquire) {
        if Instant::now() >= deadline {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            drain_timed_out = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let stdout = out
        .join()
        .map_err(|_| anyhow!("SSH output reader failed"))??;
    let stderr = err
        .join()
        .map_err(|_| anyhow!("SSH error reader failed"))??;
    anyhow::ensure!(!drain_timed_out, "SSH request timed out");
    Ok(Output {
        status: status?,
        stdout,
        stderr,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeout_also_closes_inherited_proxy_output_pipes() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & wait"]);
        let start = Instant::now();
        assert!(run(command, Duration::from_millis(100)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn output_is_drained_while_retained_memory_is_bounded() {
        assert_eq!(read_limited(&b"abcdef"[..], 3).unwrap(), b"abc");
    }
    #[test]
    fn exited_helper_cannot_leave_the_pipe_readers_blocked() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 &"]);
        let start = Instant::now();
        assert!(run(command, Duration::from_millis(100)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
