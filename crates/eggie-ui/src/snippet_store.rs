//! 跨窗口共享的 snippet 列表及其磁盘持久化。
//!
//! Snippet(片段)是用户手写、跨窗口共享的可注入命令块:任一窗口增删改都要即时反映到其它窗口,
//! 并在完全退出应用后保留。实现刻意对齐 [`crate::project_store::ProjectStore`]:同款 `Entity`
//! + `cx.observe` 跨窗口同步、同款 `load` / 原子 `save`(写 `.tmp` 再 rename)、同款「mutating
//! 方法末尾 save + notify」惯用法。
//!
//! 执行语义(见 handoff 决策):snippet 的 `content` 末尾带 `\n` → 注入后由内容本身触发执行,
//! 不带则只键入。**不额外存储「是否自动执行」标志位** —— 执行意图完全由 `content` 决定。

use std::fs;
use std::path::PathBuf;

use gpui::Context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 持久化信封的当前版本。放在最外层,便于未来在不破坏旧文件的前提下演进磁盘格式。
const PERSIST_VERSION: u32 = 1;

/// 单条 snippet。`id` 稳定标识;`name` 是列表里展示的短名;`content` 是注入终端的正文;
/// `auto_run` 为真时,注入后额外发一个回车执行(编辑对话框里的复选框显式控制,默认关闭)。
/// `#[serde(default)]` 让旧的 snippets.json(无该字段)按 false 读入,保持向后兼容。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snippet {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) content: String,
    #[serde(default)]
    pub(crate) auto_run: bool,
}

impl Snippet {
    pub(crate) fn new(name: String, content: String, auto_run: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            content,
            auto_run,
        }
    }
}

/// 磁盘上 `snippets.json` 的结构。独立于运行时的 [`SnippetStore`],只承载可序列化的部分。
#[derive(Serialize, Deserialize)]
struct PersistedSnippets {
    version: u32,
    snippets: Vec<Snippet>,
}

/// 跨窗口共享的 snippet 列表。以 `Entity<SnippetStore>` 形式在所有窗口间共享;每个 mutating 方法
/// 都会立即落盘并 `cx.notify()`,观察它的窗口据此重渲染。
pub(crate) struct SnippetStore {
    snippets: Vec<Snippet>,
    path: PathBuf,
}

impl SnippetStore {
    pub(crate) fn load() -> Self {
        Self::load_from(snippets_path())
    }

    fn load_from(path: PathBuf) -> Self {
        let snippets = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PersistedSnippets>(&bytes).ok())
            .map(|persisted| persisted.snippets)
            .unwrap_or_default();
        Self { snippets, path }
    }

    pub(crate) fn snippets(&self) -> &[Snippet] {
        &self.snippets
    }

    /// 追加一个 snippet 并落盘。
    pub(crate) fn add(&mut self, snippet: Snippet, cx: &mut Context<Self>) {
        self.snippets.push(snippet);
        self.persist(cx);
    }

    /// 移除指定 snippet 并落盘。移除不存在的 id 是无操作(不落盘、不 notify)。
    pub(crate) fn remove(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let before = self.snippets.len();
        self.snippets.retain(|snippet| snippet.id != id);
        if self.snippets.len() != before {
            self.persist(cx);
        }
    }

    /// 更新名称与正文并落盘。id 不存在则无操作。
    pub(crate) fn update(
        &mut self,
        id: Uuid,
        name: String,
        content: String,
        auto_run: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(snippet) = self.snippets.iter_mut().find(|snippet| snippet.id == id) {
            snippet.name = name;
            snippet.content = content;
            snippet.auto_run = auto_run;
            self.persist(cx);
        }
    }

    fn persist(&self, cx: &mut Context<Self>) {
        if let Err(error) = self.save() {
            eprintln!("failed to persist Eggie snippets: {error}");
        }
        cx.notify();
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let persisted = PersistedSnippets {
            version: PERSIST_VERSION,
            snippets: self.snippets.clone(),
        };
        let encoded = serde_json::to_vec_pretty(&persisted)?;
        let temporary_path = self.path.with_extension("json.tmp");
        fs::write(&temporary_path, encoded)?;
        fs::rename(temporary_path, &self.path)
    }
}

/// 与 `projects_path()` 同目录:macOS `~/Library/Application Support/Eggie/snippets.json`,
/// 其它平台 `~/.config/eggie/snippets.json`。
fn snippets_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    #[cfg(target_os = "macos")]
    return home
        .join("Library")
        .join("Application Support")
        .join("Eggie")
        .join("snippets.json");
    #[cfg(not(target_os = "macos"))]
    return home.join(".config").join("eggie").join("snippets.json");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("eggie-snippet-store-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir.join("snippets.json")
    }

    #[test]
    fn round_trips_snippets_through_disk() {
        let path = temp_path();
        let _ = fs::remove_file(&path);
        let mut store = SnippetStore::load_from(path.clone());
        assert!(store.snippets().is_empty());

        let snippet = Snippet::new("list".to_owned(), "ls -la".to_owned(), true);
        let id = snippet.id;
        // 手动模拟 add 的落盘(测试环境无 Context,直接调 save 验证序列化往返)。
        store.snippets.push(snippet);
        store.save().expect("save must succeed");

        let reloaded = SnippetStore::load_from(path.clone());
        assert_eq!(reloaded.snippets().len(), 1);
        assert_eq!(reloaded.snippets()[0].id, id);
        assert_eq!(reloaded.snippets()[0].name, "list");
        assert_eq!(reloaded.snippets()[0].content, "ls -la");
        assert!(reloaded.snippets()[0].auto_run);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn corrupt_file_falls_back_to_empty() {
        let path = temp_path();
        fs::write(&path, b"not json at all").expect("write garbage");
        let store = SnippetStore::load_from(path.clone());
        assert!(store.snippets().is_empty());
        let _ = fs::remove_file(&path);
    }
}
