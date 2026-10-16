//! GUI 单实例唤醒的进程间通道。
//!
//! 需求:在终端里重复运行 `eggie` 时,不要再起第二个 GUI 进程,而是让**已在运行**的 GUI 进程开
//! 一个新窗口。macOS 原生的 reopen 事件在「裸二进制 + 新 CLI 进程」这个场景下不可用(操作系统不会
//! 把一个全新进程路由成原 app 的 reopen),所以这里自建一条 Unix domain socket:
//!
//! - 首个 GUI 进程 `bind` [`gui_socket_path`] 并在后台线程 `accept`([`serve`]);
//! - 后续 `eggie` 进程尝试 `connect` 并发 [`GuiWakeMessage::OpenWindow`]([`try_wake_existing`]),
//!   成功即退出,由原进程开窗;失败(无人监听/连接超时)则自己成为首个 GUI。
//!
//! 帧格式对齐 daemon 的线协议(4 字节小端长度前缀 + `rmp-serde`),但**刻意不复用**面向 daemon 的
//! `ClientRequest` 枚举 —— 唤醒语义单一,用一个独立的小枚举避免与 daemon 协议耦合。
//! socket 文件名带 `PROTOCOL_VERSION`,不同版本的 GUI 互不唤醒(与 daemon socket 同款版本隔离)。

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use eggie_protocol::{AutomationRequest, AutomationResponse, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};

/// 唤醒消息帧的长度上限。唤醒消息只携带一个路径,几百字节足矣;设一个宽松上限防御坏数据。
const MAX_WAKE_MESSAGE_SIZE: usize = 64 * 1024;

/// 连接已有 GUI 的超时。对面若在忙(卡住)则快速放弃、退回自起 GUI,而不是无限等待。
const CONNECT_TIMEOUT: Duration = Duration::from_millis(250);

/// GUI 进程间的唤醒消息。目前只有一种:请求原进程开一个新窗口,并带上发起进程的工作目录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum GuiWakeMessage {
    OpenWindow { cwd: PathBuf },
    Control { request: AutomationRequest },
}

pub(crate) struct GuiEvent {
    pub message: GuiWakeMessage,
    pub reply: Option<std::sync::mpsc::SyncSender<AutomationResponse>>,
    pub deadline: std::time::Instant,
}

/// Connect only to an existing GUI. Never start an application or replace a daemon for automation.
pub fn control_running_gui(request: AutomationRequest) -> Result<AutomationResponse> {
    let mut stream = UnixStream::connect(gui_socket_path())
        .context("Eggie GUI is not running in this build channel")?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    write_message(&mut stream, &GuiWakeMessage::Control { request })?;
    let mut header = [0; 4];
    stream
        .read_exact(&mut header)
        .context("No control response; inspect current state before retrying a mutation")?;
    let length = u32::from_le_bytes(header) as usize;
    anyhow::ensure!(length <= 2 * 1024 * 1024, "Control response is too large");
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    Ok(rmp_serde::from_slice(&body)?)
}

/// GUI 单实例 socket 路径:`$TMPDIR/eggie-<uid>/gui-v<PROTOCOL_VERSION>.sock`。与 daemon socket 同
/// 目录、同 uid 隔离,但用 `gui-` 前缀区分,清理时各认各的前缀(见 [`reap_stale_gui_sockets`])。
pub(crate) fn gui_socket_path() -> PathBuf {
    eggie_ipc::runtime_directory().join(format!("gui-v{PROTOCOL_VERSION}.sock"))
}

/// 尝试唤醒一个已在运行的 GUI 进程,请它以 `cwd` 开新窗口。
///
/// 返回 `true` 表示已成功把请求交给原进程(调用方应立即退出);`false` 表示当前没有可唤醒的 GUI
/// (无人监听或连接失败),调用方应自己成为首个 GUI。
pub fn try_wake_existing(cwd: PathBuf) -> bool {
    let path = gui_socket_path();
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(error) => {
            // 没人监听,或 socket 是陈旧文件。清掉陈旧文件,让调用方顺利 bind。
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) && path.exists()
            {
                let _ = std::fs::remove_file(&path);
            }
            return false;
        }
    };
    let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    match write_message(&mut stream, &GuiWakeMessage::OpenWindow { cwd }) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("failed to wake the running Eggie instance: {error:#}");
            false
        }
    }
}

/// 成为首个 GUI:绑定唤醒 socket。返回 [`UnixListener`] 交给 [`serve`] 在后台 accept。
///
/// 先 bind；只有 ConnectionRefused 证实为陈旧文件时才替换，保留活跃监听者。
/// `bind` 失败(极小概率的启动竞态:另一进程刚好抢先成为首窗)返回 `Err`,调用方可退回
/// [`try_wake_existing`] 把请求交给对方。
pub(crate) fn bind_listener() -> Result<UnixListener> {
    let path = gui_socket_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create Eggie runtime dir {}", parent.display()))?;
    }
    match UnixListener::bind(&path) {
        Ok(listener) => Ok(listener),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            match UnixStream::connect(&path) {
                Ok(_) => bail!("Eggie GUI is already listening at {}", path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                    std::fs::remove_file(&path)?;
                    UnixListener::bind(&path).context("failed to replace stale GUI socket")
                }
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

/// 在后台阻塞 accept:每收到一条 [`GuiWakeMessage`] 就调用 `on_message`。`UnixListener::incoming`
/// 是阻塞的,所以本函数意在独立 `std::thread` 上运行;`on_message` 负责把动作跳回 GPUI 主线程。
pub(crate) fn serve(listener: UnixListener, mut on_message: impl FnMut(GuiEvent)) {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("GUI wake socket accept failed: {error:#}");
                continue;
            }
        };
        let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
        match read_message(&mut stream) {
            Ok(message) => {
                let control = matches!(message, GuiWakeMessage::Control { .. });
                if control && active.load(Ordering::Acquire) >= 8 {
                    let body = rmp_serde::to_vec_named(&AutomationResponse::error(
                        "busy",
                        "Too many control requests",
                    ))
                    .unwrap();
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                    let _ = stream
                        .write_all(&(body.len() as u32).to_le_bytes())
                        .and_then(|_| stream.write_all(&body));
                    continue;
                }
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                on_message(GuiEvent {
                    message,
                    reply: control.then_some(tx),
                    deadline: std::time::Instant::now() + Duration::from_secs(25),
                });
                if control {
                    active.fetch_add(1, Ordering::AcqRel);
                    let active = active.clone();
                    std::thread::spawn(move || {
                        let response = rx.recv_timeout(Duration::from_secs(25)).unwrap_or_else(|_| AutomationResponse::error("timeout", "Control request timed out; inspect state before retrying a mutation"));
                        if let Ok(mut body) = rmp_serde::to_vec_named(&response) {
                            if body.len() > 2 * 1024 * 1024 {
                                body = rmp_serde::to_vec_named(&AutomationResponse::error(
                                    "response_too_large",
                                    "Narrow the project filter or terminal text range",
                                ))
                                .unwrap();
                            }
                            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                            let _ = stream
                                .write_all(&(body.len() as u32).to_le_bytes())
                                .and_then(|_| stream.write_all(&body));
                        }
                        active.fetch_sub(1, Ordering::AcqRel);
                    });
                }
            }
            Err(error) => eprintln!("failed to read GUI wake message: {error:#}"),
        }
    }
}

/// 清理已拒绝连接的 GUI socket 文件：扫当前构建通道下所有
/// `gui-v*.sock`,`connect` 探活,连不上的删掉。只认 `gui-` 前缀,绝不碰 `daemon-` 前缀,避免与
/// daemon 的清理逻辑互相误伤。
pub(crate) fn reap_stale_gui_sockets() {
    let current = gui_socket_path();
    let Some(dir) = current.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_gui_socket = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("gui-v") && name.ends_with(".sock"));
        if !is_gui_socket {
            continue;
        }
        // 能连上说明还有活着的监听者,保留;连不上就是陈旧文件,删掉。
        if UnixStream::connect(&path).is_err_and(|error| {
            matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            )
        }) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn write_message(stream: &mut UnixStream, message: &GuiWakeMessage) -> Result<()> {
    let body = rmp_serde::to_vec_named(message).context("failed to encode GUI wake message")?;
    if body.len() > MAX_WAKE_MESSAGE_SIZE {
        bail!(
            "GUI wake message exceeds {MAX_WAKE_MESSAGE_SIZE} bytes: {}",
            body.len()
        );
    }
    let header = (body.len() as u32).to_le_bytes();
    stream.write_all(&header)?;
    stream.write_all(&body)?;
    stream.flush()?;
    Ok(())
}

fn read_message(stream: &mut UnixStream) -> Result<GuiWakeMessage> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header) as usize;
    if length > MAX_WAKE_MESSAGE_SIZE {
        bail!("GUI wake message exceeds {MAX_WAKE_MESSAGE_SIZE} bytes: {length}");
    }
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body)?;
    rmp_serde::from_slice(&body).context("invalid GUI wake message")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_message_round_trips() {
        let message = GuiWakeMessage::OpenWindow {
            cwd: PathBuf::from("/tmp/project"),
        };
        let body = rmp_serde::to_vec_named(&message).expect("encode");
        let decoded: GuiWakeMessage = rmp_serde::from_slice(&body).expect("decode");
        match decoded {
            GuiWakeMessage::OpenWindow { cwd } => assert_eq!(cwd, PathBuf::from("/tmp/project")),
            _ => panic!("unexpected control request"),
        }
    }

    #[test]
    fn socket_path_is_versioned_and_under_runtime_dir() {
        let path = gui_socket_path();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("gui-v"));
        assert!(name.ends_with(".sock"));
        assert!(path.to_string_lossy().contains("eggie-"));
    }

    #[test]
    fn control_frame_round_trips_and_rejects_oversized_input() {
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        let id = eggie_domain::SessionId::new_v4();
        let worker = std::thread::spawn(move || {
            write_message(
                &mut writer,
                &GuiWakeMessage::Control {
                    request: AutomationRequest::TerminalSend {
                        session_id: id,
                        text: "hello 中文".into(),
                        submit: false,
                    },
                },
            )
            .unwrap();
            let oversized = GuiWakeMessage::Control {
                request: AutomationRequest::TerminalSend {
                    session_id: id,
                    text: "x".repeat(MAX_WAKE_MESSAGE_SIZE),
                    submit: false,
                },
            };
            assert!(write_message(&mut writer, &oversized).is_err());
        });
        match read_message(&mut reader).unwrap() {
            GuiWakeMessage::Control {
                request:
                    AutomationRequest::TerminalSend {
                        session_id,
                        text,
                        submit,
                    },
            } => {
                assert_eq!(session_id, id);
                assert_eq!(text, "hello 中文");
                assert!(!submit);
            }
            _ => panic!("wrong control frame"),
        }
        worker.join().unwrap();
    }
}
