//! 跨窗口共享的项目列表及其磁盘持久化。
//!
//! 项目(名称、根目录、顺序)是所有窗口共享的:任一窗口增删改都要即时反映到其它窗口,并在
//! 完全退出应用后保留。会话(session)不在此处 —— 它按窗口独立、由 daemon 维护;布局(layout)
//! 也不在此处 —— 它是每窗口易变的 UI 态。所以这里只序列化 `Vec<Project>`。
//!
//! 实现刻意对齐 [`crate::settings::SettingsStore`]:同款 `Entity` + `cx.observe` 跨窗口同步、
//! 同款 `load` / 原子 `save`(写 `.tmp` 再 rename)、同款「mutating 方法末尾 save + notify」惯用法。

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
        Self::load_from(crate::persist::data_file_path("projects.json"))
    }

    fn load_from(path: PathBuf) -> Self {
        let projects = crate::persist::load_json::<PersistedProjects>(&path)
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

    /// 设置该项目新终端使用的默认 profile 并落盘。`None` 表示跟随全局 Default。id 不存在则无操作。
    pub(crate) fn set_default_profile(
        &mut self,
        id: ProjectId,
        profile_id: Option<eggie_domain::ProfileId>,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.projects.iter_mut().find(|project| project.id == id) {
            project.default_profile_id = profile_id;
            self.persist(cx);
        }
    }

    /// 把所有指向 `profile_id` 的项目默认值清空,让它们回落到全局 Default。
    ///
    /// 在 profile 被删除后调用。功能上并非必需 —— [`crate::profile_store::ProfileStore::resolve`]
    /// 已经把失效 id 解析为 Default,所以不清理也开得出终端。清理的意义在于:失效 id 留在磁盘上,
    /// 若将来某个 profile 恰好复用了这个 uuid(导入、从备份恢复),这些项目会静默指向一个它们从未
    /// 选择过的 profile。返回是否真的改了什么(仅在有改动时落盘)。
    pub(crate) fn forget_profile(
        &mut self,
        profile_id: eggie_domain::ProfileId,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut changed = false;
        for project in &mut self.projects {
            if project.default_profile_id == Some(profile_id) {
                project.default_profile_id = None;
                changed = true;
            }
        }
        if changed {
            self.persist(cx);
        }
        changed
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
        let persisted = PersistedProjects {
            version: PERSIST_VERSION,
            projects: self.projects.clone(),
        };
        crate::persist::save_json_atomic(&self.path, &persisted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// 每个测试独占一个目录(label 进目录名)。cargo 默认并发跑同一模块的测试,而这些测试都读写
    /// `projects.json` —— 只按进程 id 分目录会让它们互相覆盖(实测:新增第三个测试后,单独跑通过、
    /// 全量跑失败)。与 `profile_store.rs` 的同名 helper 保持一致。
    fn temp_path(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eggie-project-store-test-{}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        dir.join("projects.json")
    }

    #[test]
    fn round_trips_projects_through_disk() {
        let path = temp_path("round-trip");
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
        let path = temp_path("corrupt");
        fs::write(&path, b"not json at all").expect("write garbage");
        let store = ProjectStore::load_from(path.clone());
        assert!(store.projects().is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn the_default_profile_id_round_trips_and_forgetting_clears_only_matching_projects() {
        let path = temp_path("default-profile");
        let mut store = ProjectStore::load_from(path.clone());

        let doomed = eggie_domain::ProfileId::new_v4();
        let kept = eggie_domain::ProfileId::new_v4();
        for (name, profile) in [
            ("uses-doomed", Some(doomed)),
            ("uses-kept", Some(kept)),
            ("uses-none", None),
            // A second project on the same profile, so `forget_profile` is exercised across more than
            // one match rather than stopping at the first.
            ("also-doomed", Some(doomed)),
        ] {
            let mut project = Project::new(name.to_owned());
            project.default_profile_id = profile;
            store.projects.push(project);
        }
        store.save().expect("save must succeed");

        // `default_profile_id` must survive the disk round trip, or a project's chosen profile would
        // silently revert to the built-in Default on every restart.
        let reloaded = ProjectStore::load_from(path.clone());
        let by_name = |store: &ProjectStore, name: &str| {
            store
                .projects()
                .iter()
                .find(|project| project.name == name)
                .expect("project must exist")
                .default_profile_id
        };
        assert_eq!(by_name(&reloaded, "uses-doomed"), Some(doomed));
        assert_eq!(by_name(&reloaded, "uses-none"), None);

        // Replicate `forget_profile`'s mutation (it needs a `Context`, which tests have no access to).
        let mut store = reloaded;
        let forget = |store: &mut ProjectStore, profile_id: eggie_domain::ProfileId| {
            let mut changed = false;
            for project in &mut store.projects {
                if project.default_profile_id == Some(profile_id) {
                    project.default_profile_id = None;
                    changed = true;
                }
            }
            changed
        };

        assert!(forget(&mut store, doomed), "a match reports a change so the caller can persist");
        assert_eq!(by_name(&store, "uses-doomed"), None, "every match is cleared");
        assert_eq!(by_name(&store, "also-doomed"), None);
        assert_eq!(by_name(&store, "uses-kept"), Some(kept), "other profiles are untouched");

        // Idempotent, and a no-op reports false — that is what keeps it from rewriting the file (and
        // notifying every window) when a profile no project referenced is deleted.
        assert!(!forget(&mut store, doomed), "a second pass changes nothing");
        assert!(!forget(&mut store, eggie_domain::ProfileId::new_v4()), "an unknown id is a no-op");

        let _ = fs::remove_file(&path);
    }
}
