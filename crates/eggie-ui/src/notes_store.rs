//! 跨窗口共享的全局便签(notes)及其磁盘持久化。
//!
//! 便签是一整块自由文本(iTerm2 式,可任意粘贴/多行编辑),不附着到某个会话或某一行。跨窗口
//! 共享:任一窗口编辑都要即时反映到其它窗口,并在完全退出应用后保留。实现刻意对齐
//! [`crate::project_store::ProjectStore`]:同款 `Entity` + `cx.observe` 跨窗口同步、同款
//! `load` / 原子 `save`(写 `.tmp` 再 rename)、同款「mutating 方法末尾 save + notify」惯用法。
//!
//! 保存时机由 UI 层控制(停止输入 ~500ms 的防抖),这里只提供 `set_text` 落盘原语。

use std::path::PathBuf;

use gpui::Context;
use serde::{Deserialize, Serialize};

/// 持久化信封的当前版本。放在最外层,便于未来在不破坏旧文件的前提下演进磁盘格式。
const PERSIST_VERSION: u32 = 1;

/// 磁盘上 `notes.json` 的结构。独立于运行时的 [`NotesStore`],只承载可序列化的部分。
#[derive(Serialize, Deserialize)]
struct PersistedNotes {
    version: u32,
    text: String,
}

/// 跨窗口共享的全局便签。以 `Entity<NotesStore>` 形式在所有窗口间共享;`set_text` 落盘并
/// `cx.notify()`,观察它的窗口据此对齐本地视图。
pub(crate) struct NotesStore {
    text: String,
    path: PathBuf,
}

impl NotesStore {
    pub(crate) fn load() -> Self {
        Self::load_from(crate::persist::data_file_path("notes.json"))
    }

    fn load_from(path: PathBuf) -> Self {
        let text = crate::persist::load_json::<PersistedNotes>(&path)
            .map(|persisted| persisted.text)
            .unwrap_or_default();
        Self { text, path }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// 设置便签正文并落盘。与当前内容相同则无操作(不落盘、不 notify),避免防抖回写引起的
    /// 无谓跨窗口重渲染。
    pub(crate) fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        if self.text == text {
            return;
        }
        self.text = text;
        self.persist(cx);
    }

    fn persist(&self, cx: &mut Context<Self>) {
        if let Err(error) = self.save() {
            eprintln!("failed to persist Eggie notes: {error}");
        }
        cx.notify();
    }

    fn save(&self) -> std::io::Result<()> {
        let persisted = PersistedNotes {
            version: PERSIST_VERSION,
            text: self.text.clone(),
        };
        crate::persist::save_json_atomic(&self.path, &persisted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_path() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("eggie-notes-store-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir.join("notes.json")
    }

    #[test]
    fn round_trips_notes_through_disk() {
        let path = temp_path();
        let _ = fs::remove_file(&path);
        let mut store = NotesStore::load_from(path.clone());
        assert!(store.text().is_empty());

        // 手动模拟 set_text 的落盘(测试环境无 Context,直接调 save 验证序列化往返)。
        store.text = "line one\nline two\n".to_owned();
        store.save().expect("save must succeed");

        let reloaded = NotesStore::load_from(path.clone());
        assert_eq!(reloaded.text(), "line one\nline two\n");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn corrupt_file_falls_back_to_empty() {
        let path = temp_path();
        fs::write(&path, b"not json at all").expect("write garbage");
        let store = NotesStore::load_from(path.clone());
        assert!(store.text().is_empty());
        let _ = fs::remove_file(&path);
    }
}
