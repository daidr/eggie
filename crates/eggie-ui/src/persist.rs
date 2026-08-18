//! 跨窗口共享 store 的公共磁盘持久化原语。
//!
//! [`SettingsStore`](crate::settings::SettingsStore)、[`ProjectStore`](crate::project_store::ProjectStore)、
//! [`SnippetStore`](crate::snippet_store::SnippetStore)、[`NotesStore`](crate::notes_store::NotesStore)
//! 都遵循同一套「读 JSON 回退默认 / 原子写(`.tmp` 再 rename)」惯用法,且都落在同一个应用数据目录下。
//! 这里把路径推导与 I/O 收敛成三个自由函数,让各 store 只保留自己的信封结构与领域方法,消除逐字
//! 重复的样板,并保证「数据目录」在全仓只有一处定义(复用 [`eggie_update::app_data_dir`])。

use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// 应用数据目录下某个 JSON 文件的完整路径:macOS `~/Library/Application Support/Eggie/<file_name>`,
/// 其它平台 `~/.config/eggie/<file_name>`。目录本身由 [`eggie_update::app_data_dir`] 提供,是全仓
/// 唯一的真源。
pub(crate) fn data_file_path(file_name: &str) -> PathBuf {
    eggie_update::app_data_dir().join(file_name)
}

/// 从 `path` 读取并反序列化一个 JSON 值。文件缺失、读取失败或内容损坏都返回 `None`,由调用方决定
/// 回退策略(通常 `unwrap_or_default`)。这与各 store「损坏文件回退空」的既有语义一致。
pub(crate) fn load_json<T: DeserializeOwned>(path: &PathBuf) -> Option<T> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<T>(&bytes).ok())
}

/// 原子地把 `value` 序列化为美化 JSON 写入 `path`:先确保父目录存在,写入同目录的 `.json.tmp`,再
/// `rename` 覆盖目标,避免写入中途崩溃留下半截文件。
pub(crate) fn save_json_atomic<T: Serialize>(path: &PathBuf, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let encoded = serde_json::to_vec_pretty(value)?;
    let temporary_path = path.with_extension("json.tmp");
    fs::write(&temporary_path, encoded)?;
    fs::rename(temporary_path, path)
}
