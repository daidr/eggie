//! 跨窗口共享的项目列表及其磁盘持久化。
//!
//! 项目(名称、根目录、顺序)是所有窗口共享的:任一窗口增删改都要即时反映到其它窗口,并在
//! 完全退出应用后保留。会话(session)不在此处 —— 它按窗口独立、由 daemon 维护;布局(layout)
//! 也不在此处 —— 它是每窗口易变的 UI 态。所以这里只序列化 `Vec<Project>`。
//!
//! 实现刻意对齐 [`crate::settings::SettingsStore`]:同款 `Entity` + `cx.observe` 跨窗口同步、
//! 同款 `load` / 原子 `save`(写 `.tmp` 再 rename)、同款「mutating 方法末尾 save + notify」惯用法。

use std::fs;
use std::path::PathBuf;

use eggie_domain::{Project, ProjectId};
use gpui::Context;
use serde::{Deserialize, Serialize};

/// 持久化信封的当前版本。放在最外层,便于未来在不破坏旧文件的前提下演进磁盘格式。
const PERSIST_VERSION: u32 = 1;

/// 磁盘上 `projects.json` 的结构。独立于运行时的 [`ProjectStore`],只承载可序列化的部分。
#[derive(Serialize, Deserialize)]
struct PersistedProjects {
    version: u32,
    projects: Vec<Project>,
}

/// 跨窗口共享的项目列表。以 `Entity<ProjectStore>` 形式在所有窗口间共享;每个 mutating 方法都会
/// 立即落盘并 `cx.notify()`,观察它的窗口据此重渲染并对齐本地视图。
pub(crate) struct ProjectStore {
    projects: Vec<Project>,
    path: PathBuf,
}

impl ProjectStore {
    pub(crate) fn load() -> Self {
        Self::load_from(projects_path())
    }

    fn load_from(path: PathBuf) -> Self {
        let projects = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PersistedProjects>(&bytes).ok())
            .map(|persisted| persisted.projects)
            .unwrap_or_default();
        Self { projects, path }
    }

    pub(crate) fn projects(&self) -> &[Project] {
        &self.projects
    }

    /// 追加一个项目并落盘。
    pub(crate) fn add(&mut self, project: Project, cx: &mut Context<Self>) {
        self.projects.push(project);
        self.persist(cx);
    }

    /// 移除指定项目并落盘。移除不存在的 id 是无操作(不落盘、不 notify)。
    pub(crate) fn remove(&mut self, id: ProjectId, cx: &mut Context<Self>) {
        let before = self.projects.len();
        self.projects.retain(|project| project.id != id);
        if self.projects.len() != before {
            self.persist(cx);
        }
    }

    /// 改名并落盘。id 不存在则无操作。
    pub(crate) fn rename(&mut self, id: ProjectId, name: String, cx: &mut Context<Self>) {
        if let Some(project) = self.projects.iter_mut().find(|project| project.id == id) {
            project.name = name;
            self.persist(cx);
        }
    }

    /// 设置根目录并落盘。id 不存在则无操作。
    pub(crate) fn set_root(&mut self, id: ProjectId, root: Option<PathBuf>, cx: &mut Context<Self>) {
        if let Some(project) = self.projects.iter_mut().find(|project| project.id == id) {
            project.root = root;
            self.persist(cx);
        }
    }

    /// 按根目录查找或新建项目(D4:`eggie <dir>` 唤醒时用)。已存在同 `root` 的项目则复用其 id、
    /// 不落盘;否则以该目录新建一个项目并落盘。返回 `(项目 id, 是否新建)`。
    pub(crate) fn upsert_by_root(
        &mut self,
        root: PathBuf,
        cx: &mut Context<Self>,
    ) -> (ProjectId, bool) {
        if let Some(existing) = self
            .projects
            .iter()
            .find(|project| project.root.as_deref() == Some(root.as_path()))
        {
            return (existing.id, false);
        }
        let project = Project::from_root(root);
        let id = project.id;
        self.add(project, cx);
        (id, true)
    }

    fn persist(&self, cx: &mut Context<Self>) {
        if let Err(error) = self.save() {
            eprintln!("failed to persist Eggie projects: {error}");
        }
        cx.notify();
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let persisted = PersistedProjects {
            version: PERSIST_VERSION,
            projects: self.projects.clone(),
        };
        let encoded = serde_json::to_vec_pretty(&persisted)?;
        let temporary_path = self.path.with_extension("json.tmp");
        fs::write(&temporary_path, encoded)?;
        fs::rename(temporary_path, &self.path)
    }
}

/// 与 `settings_path()` 同目录:macOS `~/Library/Application Support/Eggie/projects.json`,
/// 其它平台 `~/.config/eggie/projects.json`。
fn projects_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    #[cfg(target_os = "macos")]
    return home
        .join("Library")
        .join("Application Support")
        .join("Eggie")
        .join("projects.json");
    #[cfg(not(target_os = "macos"))]
    return home.join(".config").join("eggie").join("projects.json");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn temp_path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eggie-project-store-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir.join("projects.json")
    }

    #[test]
    fn round_trips_projects_through_disk() {
        let path = temp_path();
        let _ = fs::remove_file(&path);
        let mut store = ProjectStore::load_from(path.clone());
        assert!(store.projects().is_empty());

        let project = Project::from_root(PathBuf::from("/tmp/example"));
        let id = project.id;
        // 手动模拟 add 的落盘(测试环境无 Context,直接调 save 验证序列化往返)。
        store.projects.push(project);
        store.save().expect("save must succeed");

        let reloaded = ProjectStore::load_from(path.clone());
        assert_eq!(reloaded.projects().len(), 1);
        assert_eq!(reloaded.projects()[0].id, id);
        assert_eq!(
            reloaded.projects()[0].root.as_deref(),
            Some(Path::new("/tmp/example"))
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn corrupt_file_falls_back_to_empty() {
        let path = temp_path();
        fs::write(&path, b"not json at all").expect("write garbage");
        let store = ProjectStore::load_from(path.clone());
        assert!(store.projects().is_empty());
        let _ = fs::remove_file(&path);
    }
}
