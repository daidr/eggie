//! Application paths and atomic persistence shared by all Eggie processes.
use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// 应用数据目录下某个 JSON 文件的完整路径:macOS `~/Library/Application Support/Eggie/<file_name>`,
/// 其它平台 `~/.config/eggie/<file_name>`。目录本身由 [`state_dir`] 提供,是全仓
/// 唯一的真源。
pub fn data_file_path(file_name: &str) -> PathBuf {
    state_dir().join(file_name)
}

/// 从 `path` 读取并反序列化一个 JSON 值。文件缺失、读取失败或内容损坏都返回 `None`,由调用方决定
/// 回退策略(通常 `unwrap_or_default`)。这与各 store「损坏文件回退空」的既有语义一致。
///
/// 需要区分「文件不存在」与「文件存在但读不懂」时用 [`load_json_outcome`] —— 前者可以安全地当作
/// 首次运行,后者意味着磁盘上有用户数据读不出来,盖写它会造成数据丢失。
pub fn load_json<T: DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    match load_json_outcome(path) {
        LoadOutcome::Loaded(value) => Some(value),
        LoadOutcome::Missing | LoadOutcome::Corrupt(_) => None,
    }
}

/// [`load_json`] 的三态版本:区分「加载成功」「文件不存在」「文件存在但无法解析」。
///
/// 只有 `Missing` 才是真正的首次运行。`Corrupt` 说明磁盘上有内容读不出来 —— 调用方应把原文件挪到
/// 一边保留证据(见 [`quarantine_corrupt_file`]),而不是当作空白重来。
pub enum LoadOutcome<T> {
    Loaded(T),
    Missing,
    /// 文件存在但读取或解析失败,附带原因(用于日志)。
    Corrupt(String),
}

pub fn load_json_outcome<T: DeserializeOwned>(path: &std::path::Path) -> LoadOutcome<T> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return LoadOutcome::Missing,
        Err(error) => return LoadOutcome::Corrupt(error.to_string()),
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(value) => LoadOutcome::Loaded(value),
        Err(error) => LoadOutcome::Corrupt(error.to_string()),
    }
}

/// 把无法解析的文件重命名到带时间戳的 `.corrupt-<epoch_secs>` 旁路,保留用户数据以便人工抢救。
/// 返回挪到的路径(失败则 `None`,此时调用方只能记日志继续 —— 保留证据是尽力而为,不能阻断启动)。
pub fn quarantine_corrupt_file(path: &std::path::Path) -> Option<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let mut file_name = path.file_name()?.to_os_string();
    file_name.push(format!(".corrupt-{stamp}"));
    let quarantined = path.with_file_name(file_name);
    fs::rename(path, &quarantined).ok().map(|()| quarantined)
}

/// 原子地把 `value` 序列化为美化 JSON 写入 `path`:先确保父目录存在,写入同目录的 `.json.tmp`,再
/// `rename` 覆盖目标,避免写入中途崩溃留下半截文件。
pub fn save_json_atomic<T: Serialize>(path: &std::path::Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let encoded = serde_json::to_vec_pretty(value)?;
    let temporary_path = path.with_extension("json.tmp");
    fs::write(&temporary_path, encoded)?;
    fs::rename(temporary_path, path)
}

pub fn config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("EGGIE_CONFIG") {
        return PathBuf::from(path);
    }
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    root.join("eggie/init.lua")
}

pub fn state_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    if let Some(home) = home {
        return home.join("Library/Application Support/Eggie");
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(home) = home {
        return home.join(".config/eggie");
    }
    PathBuf::from(".")
}
