//! 跨窗口共享的 profile 列表及其磁盘持久化。
//!
//! Profile 是全局设置与单个终端之间的第三层配置：它命名一套启动配方(shell/args/env/cwd/远程目标)
//! 和一组外观覆盖。和项目一样,profile 是所有窗口共享的,所以这里只序列化 `Vec<Profile>`,
//! 实现刻意对齐 [`crate::project_store::ProjectStore`](同款 `Entity` + `cx.observe` 跨窗口同步、
//! 同款原子 `save`、同款「mutating 方法末尾 save + notify」惯用法)。
//!
//! 首次加载时把全局设置里的 shell 字段(`shell_program` / `shell_args` / `shell_integration_path`)
//! 迁入内置 Default profile —— 迁移后这些字段的唯一真源就是 profile,设置窗口不再直接编辑它们。

use std::path::PathBuf;

use eggie_domain::{Profile, ProfileId, DEFAULT_PROFILE_ID};
use gpui::Context;
use serde::{Deserialize, Serialize};

/// 持久化信封的当前版本。放在最外层,便于未来在不破坏旧文件的前提下演进磁盘格式。
const PERSIST_VERSION: u32 = 1;

/// 磁盘上 `profiles.json` 的结构。独立于运行时的 [`ProfileStore`],只承载可序列化的部分。
#[derive(Serialize, Deserialize)]
struct PersistedProfiles {
    /// 带 default 是刻意的:缺 `version` 的文件(手工编辑、第三方脚本)仍应能读出 `profiles`,
    /// 否则整份文件解析失败会被当成损坏,把用户的 profile 挪进隔离区。
    #[serde(default = "default_persist_version")]
    version: u32,
    profiles: Vec<Profile>,
}

fn default_persist_version() -> u32 {
    PERSIST_VERSION
}

/// 从全局设置迁入 Default profile 的启动字段快照。
///
/// 迁移只在 `profiles.json` 尚不存在时发生一次。传值而非直接读 `SettingsStore`,让迁移逻辑
/// 保持纯函数、可单测,也避免 store 之间的加载顺序依赖。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LegacyShellSettings {
    pub(crate) shell_program: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) shell_integration_path: bool,
}

impl Default for LegacyShellSettings {
    /// 手写而非 derive:`shell_integration_path` 在全局设置里默认**开启**
    /// ([`crate::settings::AppSettings`] 用 `default_true`),derive 出来的 `false` 会让
    /// 「缺省的旧设置」意外关掉 path 特性。默认值必须跟它承载的那个设置一致。
    fn default() -> Self {
        Self {
            shell_program: String::new(),
            shell_args: Vec::new(),
            shell_integration_path: true,
        }
    }
}

/// 跨窗口共享的 profile 列表。以 `Entity<ProfileStore>` 形式在所有窗口间共享;每个 mutating 方法
/// 都会立即落盘并 `cx.notify()`,观察它的窗口据此重渲染。
///
/// 不变式:`profiles` 永远非空且第一个元素是内置 Default profile —— 它是所有失效 profile 引用的
/// 兜底目标,不可删除。
pub(crate) struct ProfileStore {
    profiles: Vec<Profile>,
    path: PathBuf,
    /// 每次变更自增。渲染路径据此判断「profile 定义有没有动过」,从而跳过昂贵的重解析 ——
    /// 比逐字段比对整张列表便宜得多。
    revision: u64,
}

impl ProfileStore {
    /// 从默认位置加载。`legacy` 是全局设置里的旧 shell 字段,仅在 `profiles.json` **确实不存在**时
    /// 迁入 Default profile。
    pub(crate) fn load(legacy: LegacyShellSettings) -> Self {
        Self::load_from(eggie_storage::data_file_path("profiles.json"), legacy)
    }

    fn load_from(path: PathBuf, legacy: LegacyShellSettings) -> Self {
        use eggie_storage::LoadOutcome;

        match eggie_storage::load_json_outcome::<PersistedProfiles>(&path) {
            LoadOutcome::Loaded(persisted) => {
                if persisted.version > PERSIST_VERSION {
                    // 更新的格式:不认识的字段可能承载了这份文件的关键语义,盖写会丢数据。
                    // 隔离保留 + 从纯净 Default 起步(不迁移 —— 迁移只属于「从未有过 profiles.json」)。
                    eprintln!(
                        "Eggie profiles.json has a newer format (v{}, this build understands v{PERSIST_VERSION}); \
                         preserving it and starting from the built-in Default profile",
                        persisted.version
                    );
                    return Self::quarantined_fallback(path);
                }
                let mut store = Self {
                    profiles: persisted.profiles,
                    path,
                    revision: 0,
                };
                // 文件存在但内容不满足不变式(手工编辑、旧版本、部分损坏)时修复,不做迁移:
                // 迁移只属于「首次创建」这一刻。
                store.ensure_default_profile();
                // 把旧的单槽 `appearance.theme` 折进终端深/浅两槽。这与上面那句「不做迁移」不矛盾:
                // 那说的是「从全局 shell 设置导入」这类跨来源迁移,而这里只是把同一份文件里的旧字段
                // 读成新形状,不引入任何外部值。
                //
                // 故意不为此落盘:内存里已是新形状,而 `legacy_theme` 标了 skip_serializing,下一次
                // 真实保存自然会让旧 key 消失。仅为改写格式而在启动路径上写盘,是不必要的风险。
                for profile in &mut store.profiles {
                    profile.appearance.migrate_legacy();
                }
                store
            }
            LoadOutcome::Missing => {
                // 真正的首次运行:新建列表,并把旧的全局 shell 设置迁入 Default profile。
                let store = Self {
                    profiles: vec![migrated_default_profile(legacy)],
                    path,
                    revision: 0,
                };
                // 落盘让迁移只发生一次:下次启动读到文件就走上面的分支。写失败时不能假装迁移完成 ——
                // 否则下次启动会再迁一遍,把用户这一轮对 Default 的编辑覆盖掉。这里只能报错,
                // 由用户处理(数据目录不可写本身就是更大的问题,其它 store 也一样落不了盘)。
                if let Err(error) = store.save() {
                    eprintln!(
                        "failed to persist migrated Eggie profiles: {error}; \
                         the migration will be retried on the next launch"
                    );
                }
                store
            }
            LoadOutcome::Corrupt(reason) => {
                // 文件存在但读不出来 —— 磁盘上那份是用户 profile 的唯一副本。绝不能盖写它:
                // 先挪到隔离区保留证据,再从纯净的 Default 起步。也**不迁移**,因为无法确认
                // 用户是否曾编辑过 Default(把旧的全局 shell 设置强行塞回去等于悄悄改配置)。
                eprintln!("Eggie profiles.json could not be read ({reason}); preserving it aside");
                Self::quarantined_fallback(path)
            }
        }
    }

    /// 把 `path` 上无法使用的文件挪到隔离区,并返回一个只含纯净内置 Default 的 store。
    ///
    /// 不落盘:隔离后 `path` 已不存在,下次启动会走 `Missing` 分支重新迁移 —— 对「文件损坏」这个
    /// 场景来说,那正是想要的行为(用户的 profile 已丢,回到首次运行状态最诚实)。
    fn quarantined_fallback(path: PathBuf) -> Self {
        match eggie_storage::quarantine_corrupt_file(&path) {
            Some(quarantined) => {
                eprintln!("preserved the previous profiles file at {}", quarantined.display());
            }
            None => {
                eprintln!(
                    "could not preserve the previous profiles file at {}; \
                     leaving it untouched and running with the built-in Default profile",
                    path.display()
                );
            }
        }
        Self {
            profiles: vec![Profile::default_builtin()],
            path,
            revision: 0,
        }
    }

    /// 恢复「首个元素是内置 Default profile、且 id 无重复」的不变式。
    ///
    /// 手工编辑或未来的导入/复制功能都可能引入重复 id;重复项在 `profile()`(取首个匹配)下永远不可达,
    /// 又逃过 `remove_inner` 的 Default 守卫,只能靠改 JSON 删除 —— 所以在入口一次性去重。
    fn ensure_default_profile(&mut self) {
        // 先去重:保留每个 id 的首次出现。放在移动 Default 之前,避免「保留了错的那一个」。
        let mut seen = std::collections::HashSet::new();
        self.profiles.retain(|profile| seen.insert(profile.id));

        match self
            .profiles
            .iter()
            .position(|profile| profile.id == DEFAULT_PROFILE_ID)
        {
            Some(0) => {}
            Some(index) => {
                let default = self.profiles.remove(index);
                self.profiles.insert(0, default);
            }
            None => self.profiles.insert(0, Profile::default_builtin()),
        }
        // builtin 标记也可能在手工编辑中丢失;它决定 UI 是否允许删除,必须归位。
        self.profiles[0].builtin = true;
    }

    pub(crate) fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    /// 变更计数器。渲染路径用它判断 profile 定义是否动过,而不必比对整张列表。
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// 内置 Default profile。所有失效引用的兜底目标,因此永远存在。
    ///
    /// 用 `expect` 而非 `[0]`:不变式由 [`Self::ensure_default_profile`] 在加载与每次变更后维护,
    /// 但那是写入侧的保证。万一将来某条新增的变更路径漏了它,这里的 panic 至少指名了违背的不变式,
    /// 而不是抛出一个来源不明的下标越界。
    pub(crate) fn default_profile(&self) -> &Profile {
        self.profiles
            .first()
            .expect("ProfileStore invariant: profiles[0] is the built-in Default profile")
    }

    /// 按 id 查找。
    pub(crate) fn profile(&self, id: ProfileId) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    /// 解析一个可能失效的 profile 引用:id 缺失、或指向已删除的 profile,都回落到 Default。
    /// 这是「新开终端该用哪个 profile」的唯一入口,让失效引用永远不会导致开不出终端。
    pub(crate) fn resolve(&self, id: Option<ProfileId>) -> &Profile {
        id.and_then(|id| self.profile(id))
            .unwrap_or_else(|| self.default_profile())
    }

    /// 某个终端应套用的外观覆盖;`None` 表示「跟随全局外观」。
    ///
    /// 刻意用 [`Self::profile`] 而非 [`Self::resolve`]:终端还在跑时 profile 被删掉,应当退回全局外观,
    /// 而不是继承内置 Default 的覆盖 —— 让终端突然变成另一个 profile 的配色是可见的意外。
    /// 全字段继承的覆盖归一成 `None`,这样调用方不必区分「没有 profile」和「有 profile 但没覆盖」。
    pub(crate) fn appearance_override(
        &self,
        profile_id: ProfileId,
    ) -> Option<eggie_domain::ProfileAppearance> {
        let appearance = self.profile(profile_id)?.appearance.clone();
        (!appearance.is_empty()).then_some(appearance)
    }

    /// 追加一个 profile 并落盘。
    pub(crate) fn add(&mut self, profile: Profile, cx: &mut Context<Self>) {
        self.profiles.push(profile);
        self.persist(cx);
    }

    /// 移除指定 profile 并落盘。内置 Default 不可删除;移除不存在的 id 是无操作。
    /// 返回是否真的删除了 —— 调用方据此把指向它的项目默认值清空。
    pub(crate) fn remove(&mut self, id: ProfileId, cx: &mut Context<Self>) -> bool {
        let removed = self.remove_inner(id);
        if removed {
            self.persist(cx);
        }
        removed
    }

    /// [`Self::remove`] 的纯粹变更部分(不落盘、不 notify),便于单测守卫逻辑。
    fn remove_inner(&mut self, id: ProfileId) -> bool {
        if id == DEFAULT_PROFILE_ID {
            return false;
        }
        let before = self.profiles.len();
        self.profiles.retain(|profile| profile.id != id);
        let removed = self.profiles.len() != before;
        if removed {
            // 当前的守卫已保证 Default 不会被删,但删除是唯一会缩短列表的路径 —— 让它自己收尾,
            // 这样将来新增「批量删除」之类的变更时,不变式不依赖调用方记得维护。
            self.ensure_default_profile();
        }
        removed
    }

    /// 就地修改一个 profile 并落盘。id 不存在则无操作(不落盘、不 notify)。
    ///
    /// 内置 Default 可以改名、改 shell、改外观 —— 它只是「兜底的那一个」,不是不可配置的。但 `id` 和
    /// `builtin` 不由闭包决定:改掉它们会让兜底目标失效或让 Default 变得可删,所以调用后强制归位。
    pub(crate) fn update(
        &mut self,
        id: ProfileId,
        update: impl FnOnce(&mut Profile),
        cx: &mut Context<Self>,
    ) {
        // 用下标而非 `iter_mut().find()`:守卫必须作用在刚被修改的那个元素上。若用 find 拿到引用、
        // 再去写 `profiles[0]`,一旦不变式暂时不成立(例如文件里有过重复 id),就会把标记盖到
        // 另一个 profile 上。
        let Some(index) = self.profiles.iter().position(|profile| profile.id == id) else {
            return;
        };
        update(&mut self.profiles[index]);
        // 身份不可变更:闭包改了 id 就等于「把这个 profile 换成另一个」,会让所有指向它的引用
        // (项目默认值、已存在的 tab)静默失效。
        self.profiles[index].id = id;
        if id == DEFAULT_PROFILE_ID {
            self.profiles[index].builtin = true;
        }
        // 闭包无法破坏不变式了,但仍然便宜:顺手保证 Default 在首位(未来若新增批量修改路径也受益)。
        self.ensure_default_profile();
        self.persist(cx);
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
        // 每条变更路径都经由这里,所以这一处自增就覆盖了 add / remove / update。
        self.revision = self.revision.wrapping_add(1);
        if let Err(error) = self.save() {
            eprintln!("failed to persist Eggie profiles: {error}");
        }
        cx.notify();
    }

    fn save(&self) -> std::io::Result<()> {
        let persisted = PersistedProfiles {
            version: PERSIST_VERSION,
            profiles: self.profiles.clone(),
        };
        eggie_storage::save_json_atomic(&self.path, &persisted)
    }
}

/// 内置 Default profile,启动字段来自旧的全局设置。
///
/// 迁移是一次性的、幂等的:字段语义完全对应(空 `shell_program` 表示回落 `$SHELL`,在两边含义相同),
/// 所以直接搬运即可,无需转换。
fn migrated_default_profile(legacy: LegacyShellSettings) -> Profile {
    Profile {
        shell_program: legacy.shell_program,
        shell_args: legacy.shell_args,
        shell_integration_path: legacy.shell_integration_path,
        ..Profile::default_builtin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 每个测试独占一个目录(label 进目录名)。整目录先清空再建:隔离文件带时间戳,上一轮跑剩的
    /// 残留会让「隔离区里应该只有一个文件」的断言假失败。
    fn temp_path(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eggie-profile-store-test-{}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        dir.join("profiles.json")
    }

    fn legacy() -> LegacyShellSettings {
        LegacyShellSettings {
            shell_program: "/opt/homebrew/bin/fish".to_owned(),
            shell_args: vec!["-l".to_owned()],
            shell_integration_path: false,
        }
    }

    #[test]
    fn first_load_migrates_legacy_shell_settings_into_default_profile() {
        let path = temp_path("migrate");
        let _ = fs::remove_file(&path);

        let store = ProfileStore::load_from(path.clone(), legacy());
        assert_eq!(store.profiles().len(), 1, "only the Default profile exists");
        let default = store.default_profile();
        assert_eq!(default.id, DEFAULT_PROFILE_ID);
        assert!(default.builtin);
        assert_eq!(default.shell_program, "/opt/homebrew/bin/fish");
        assert_eq!(default.shell_args, vec!["-l".to_owned()]);
        assert!(!default.shell_integration_path, "legacy toggle must carry over");

        // 迁移必须已落盘,这样第二次加载不会重跑迁移(否则用户对 Default 的编辑会被旧设置覆盖)。
        let reloaded = ProfileStore::load_from(path.clone(), LegacyShellSettings::default());
        assert_eq!(reloaded.default_profile().shell_program, "/opt/homebrew/bin/fish");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn existing_file_is_not_re_migrated() {
        let path = temp_path("no-remigrate");
        let _ = fs::remove_file(&path);

        // 首次迁移,然后模拟用户把 Default 的 shell 改回空(即「用 $SHELL」)。
        let mut store = ProfileStore::load_from(path.clone(), legacy());
        store.profiles[0].shell_program.clear();
        store.save().expect("save must succeed");

        // 再次加载时 legacy 仍是旧值,但不能覆盖用户的编辑。
        let reloaded = ProfileStore::load_from(path.clone(), legacy());
        assert!(
            reloaded.default_profile().shell_program.is_empty(),
            "user's edit must win over the legacy settings"
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn round_trips_custom_profiles_through_disk() {
        let path = temp_path("round-trip");
        let _ = fs::remove_file(&path);

        let mut store = ProfileStore::load_from(path.clone(), LegacyShellSettings::default());
        let mut profile = Profile::new("Prod".to_owned());
        profile.remote = Some(eggie_domain::RemoteTarget {
            host: "prod.example.com".to_owned(),
            user: Some("deploy".to_owned()),
            port: Some(2222),
        });
        profile.appearance.terminal_dark_theme = Some("Builtin Dark".to_owned());
        profile.appearance.font_size = Some(16.);
        profile.badge_color = Some(0xff0000);
        profile.env.insert("AWS_PROFILE".to_owned(), "prod".to_owned());
        // 关掉 path 特性:这是唯一「默认 true + skip_serializing_if = is_true」的字段,
        // 只有取 false 时才真正写进 JSON,所以只有这样才验证到它的往返。
        profile.shell_integration_path = false;
        let id = profile.id;
        store.profiles.push(profile);
        store.save().expect("save must succeed");

        let reloaded = ProfileStore::load_from(path.clone(), LegacyShellSettings::default());
        assert_eq!(reloaded.profiles().len(), 2);
        let loaded = reloaded.profile(id).expect("custom profile must survive");
        assert_eq!(loaded.name, "Prod");
        let remote = loaded.remote.as_ref().expect("remote target must survive");
        assert_eq!(remote.destination(), "deploy@prod.example.com");
        assert_eq!(remote.port, Some(2222));
        assert_eq!(loaded.appearance.font_size, Some(16.));
        assert_eq!(loaded.badge_color, Some(0xff0000));
        assert_eq!(loaded.env.get("AWS_PROFILE").map(String::as_str), Some("prod"));
        assert!(!loaded.shell_integration_path, "an explicit false must round-trip");
        // 反向:内置 Default 的 true 被 skip,靠 serde default 读回来。
        assert!(
            reloaded.default_profile().shell_integration_path,
            "the skipped default must reload as true"
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn corrupt_file_is_preserved_and_never_clobbered() {
        let path = temp_path("corrupt");
        let _ = fs::remove_file(&path);
        // 先造一份有用户数据的正常文件。
        let mut store = ProfileStore::load_from(path.clone(), LegacyShellSettings::default());
        let prod = Profile::new("Prod".to_owned());
        let prod_id = prod.id;
        store.profiles.push(prod);
        store.save().expect("seed must succeed");
        let original = fs::read(&path).expect("seeded file must be readable");

        // 截断成半截 JSON:文件在,但读不出来。
        fs::write(&path, &original[..original.len() / 2]).expect("truncate");

        let recovered = ProfileStore::load_from(path.clone(), legacy());
        // 1. 绝不能把旧的全局 shell 设置强行迁回去 —— 用户可能早就改过 Default。
        assert!(
            recovered.default_profile().shell_program.is_empty(),
            "a corrupt file must not re-run the legacy migration"
        );
        assert_eq!(recovered.profiles().len(), 1);
        assert!(recovered.profile(prod_id).is_none(), "the custom profile is gone from memory");

        // 2. 损坏的原文件必须被保留在隔离区,而不是被盖写 —— 那是用户数据的唯一副本。
        let dir = path.parent().expect("temp dir");
        let quarantined: Vec<_> = fs::read_dir(dir)
            .expect("read temp dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("profiles.json.corrupt-"))
            .collect();
        assert_eq!(quarantined.len(), 1, "the unreadable file must be preserved aside");
        let preserved = fs::read(dir.join(&quarantined[0])).expect("quarantined file readable");
        assert_eq!(
            preserved,
            &original[..original.len() / 2],
            "the preserved bytes must be exactly what was on disk"
        );
        // 原文件已挪走,不会留下一份被盖写的残骸。
        assert!(!path.exists(), "the corrupt file is moved, not overwritten in place");

        let _ = fs::remove_file(dir.join(&quarantined[0]));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_newer_format_version_is_preserved_rather_than_downgraded() {
        let path = temp_path("newer-version");
        let _ = fs::remove_file(&path);
        // 未来版本写的文件:能解析,但 version 更高 —— 可能有本版本不认识的字段。
        let persisted = PersistedProfiles {
            version: PERSIST_VERSION + 1,
            profiles: vec![Profile::default_builtin(), Profile::new("Future".to_owned())],
        };
        eggie_storage::save_json_atomic(&path, &persisted).expect("seed file");

        let store = ProfileStore::load_from(path.clone(), legacy());
        assert_eq!(store.profiles().len(), 1, "unknown-format contents are not adopted");
        assert!(
            store.default_profile().shell_program.is_empty(),
            "a newer file must not trigger the legacy migration either"
        );

        let dir = path.parent().expect("temp dir");
        let quarantined: Vec<_> = fs::read_dir(dir)
            .expect("read temp dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("profiles.json.corrupt-"))
            .collect();
        assert_eq!(quarantined.len(), 1, "the newer file must be preserved for a future build");

        for name in &quarantined {
            let _ = fs::remove_file(dir.join(name));
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_file_without_a_version_field_still_loads() {
        let path = temp_path("no-version");
        // 手工编辑或第三方脚本写出的、缺 version 的文件:内容仍应被采纳,而不是当成损坏。
        fs::write(
            &path,
            br#"{"profiles":[{"id":"eee1e000-0000-4000-8000-000000000001","name":"Default","builtin":true,"shell_program":"/bin/bash"}]}"#,
        )
        .expect("seed file");

        let store = ProfileStore::load_from(path.clone(), legacy());
        assert_eq!(
            store.default_profile().shell_program,
            "/bin/bash",
            "the file's contents must win, not the legacy migration"
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn duplicate_profile_ids_are_deduplicated_on_load() {
        let path = temp_path("duplicate-ids");
        let shared = Profile::new("Shared".to_owned());
        let mut alias = shared.clone();
        alias.name = "Alias".to_owned();
        let persisted = PersistedProfiles {
            version: PERSIST_VERSION,
            profiles: vec![
                Profile::default_builtin(),
                Profile::default_builtin(),
                shared.clone(),
                alias,
            ],
        };
        eggie_storage::save_json_atomic(&path, &persisted).expect("seed file");

        let store = ProfileStore::load_from(path.clone(), LegacyShellSettings::default());
        // 重复的 Default 与重复的自定义 id 都只保留首个:否则第二份永远不可达也删不掉。
        assert_eq!(store.profiles().len(), 2);
        assert_eq!(store.default_profile().id, DEFAULT_PROFILE_ID);
        assert_eq!(store.profiles()[1].name, "Shared", "the first occurrence wins");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn update_stamps_the_profile_it_mutated_and_keeps_identity() {
        let path = temp_path("update");
        let _ = fs::remove_file(&path);
        let mut store = ProfileStore::load_from(path.clone(), LegacyShellSettings::default());
        let custom = Profile::new("Custom".to_owned());
        let custom_id = custom.id;
        store.profiles.push(custom);

        // 复刻 update 的变更核心(它需要 Context,测试环境没有)。
        let apply = |store: &mut ProfileStore, id: ProfileId, edit: &dyn Fn(&mut Profile)| {
            let Some(index) = store.profiles.iter().position(|profile| profile.id == id) else {
                return;
            };
            edit(&mut store.profiles[index]);
            store.profiles[index].id = id;
            if id == DEFAULT_PROFILE_ID {
                store.profiles[index].builtin = true;
            }
            store.ensure_default_profile();
        };

        // 改内置 Default:允许改名/改 shell,但 builtin 必须保住。
        apply(&mut store, DEFAULT_PROFILE_ID, &|profile| {
            profile.name = "My Shell".to_owned();
            profile.shell_program = "/bin/zsh".to_owned();
            profile.builtin = false;
        });
        assert_eq!(store.default_profile().name, "My Shell", "renaming Default is allowed");
        assert_eq!(store.default_profile().shell_program, "/bin/zsh");
        assert!(store.default_profile().builtin, "builtin must be restored");

        // 闭包改 id 不生效:否则所有指向它的引用会静默失效。
        apply(&mut store, custom_id, &|profile| {
            profile.id = ProfileId::new_v4();
            profile.name = "Renamed".to_owned();
        });
        let custom = store.profile(custom_id).expect("identity must survive the closure");
        assert_eq!(custom.name, "Renamed", "other fields still change");
        assert!(!custom.builtin, "a custom profile is not promoted to builtin");
        // 改自定义 profile 不会动 Default 的位置。
        assert_eq!(store.profiles()[0].id, DEFAULT_PROFILE_ID);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn missing_default_profile_is_restored_without_migrating() {
        let path = temp_path("restore-default");
        // 手工写一个只有自定义 profile 的文件(旧版本或人工编辑的产物)。
        let custom = Profile::new("Only".to_owned());
        let persisted = PersistedProfiles {
            version: PERSIST_VERSION,
            profiles: vec![custom.clone()],
        };
        eggie_storage::save_json_atomic(&path, &persisted).expect("seed file");

        let store = ProfileStore::load_from(path.clone(), legacy());
        assert_eq!(store.profiles().len(), 2, "Default must be inserted");
        assert_eq!(store.default_profile().id, DEFAULT_PROFILE_ID);
        assert!(store.default_profile().builtin);
        assert!(
            store.default_profile().shell_program.is_empty(),
            "restoring the invariant must not re-run the legacy migration"
        );
        assert_eq!(store.profiles()[1].id, custom.id);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn default_profile_is_moved_to_the_front() {
        let path = temp_path("reorder-default");
        let custom = Profile::new("First".to_owned());
        let persisted = PersistedProfiles {
            version: PERSIST_VERSION,
            profiles: vec![custom.clone(), Profile::default_builtin()],
        };
        eggie_storage::save_json_atomic(&path, &persisted).expect("seed file");

        let store = ProfileStore::load_from(path.clone(), LegacyShellSettings::default());
        assert_eq!(store.profiles().len(), 2);
        assert_eq!(store.default_profile().id, DEFAULT_PROFILE_ID);
        assert_eq!(store.profiles()[1].id, custom.id);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn resolve_falls_back_to_default_for_stale_ids() {
        let path = temp_path("resolve");
        let _ = fs::remove_file(&path);
        let mut store = ProfileStore::load_from(path.clone(), LegacyShellSettings::default());
        let profile = Profile::new("Keep".to_owned());
        let id = profile.id;
        store.profiles.push(profile);

        assert_eq!(store.resolve(Some(id)).id, id, "a live id resolves to itself");
        assert_eq!(
            store.resolve(Some(ProfileId::new_v4())).id,
            DEFAULT_PROFILE_ID,
            "a deleted profile's id falls back to Default"
        );
        assert_eq!(
            store.resolve(None).id,
            DEFAULT_PROFILE_ID,
            "no id at all falls back to Default"
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_deleted_profile_yields_no_appearance_override() {
        let path = temp_path("appearance-override");
        let _ = fs::remove_file(&path);
        let mut store = ProfileStore::load_from(path.clone(), LegacyShellSettings::default());

        // A themed profile, and one with no overrides at all.
        let mut themed = Profile::new("Prod".to_owned());
        themed.appearance.terminal_dark_theme = Some("Builtin Dark".to_owned());
        let themed_id = themed.id;
        let plain = Profile::new("Plain".to_owned());
        let plain_id = plain.id;
        store.profiles.push(themed);
        store.profiles.push(plain);

        // Give the built-in Default an override too, so "falls back to Default" would be detectable.
        store.profiles[0].appearance.terminal_dark_theme = Some("Builtin Light".to_owned());

        assert_eq!(
            store.appearance_override(themed_id)
                .and_then(|appearance| appearance.terminal_dark_theme),
            Some("Builtin Dark".to_owned()),
        );
        assert!(
            store.appearance_override(plain_id).is_none(),
            "an all-inherit override normalizes to None"
        );
        assert!(
            store.appearance_override(ProfileId::new_v4()).is_none(),
            "a profile deleted while its terminals run must mean no overrides, not Default's"
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn builtin_default_cannot_be_removed() {
        let path = temp_path("no-delete-default");
        let _ = fs::remove_file(&path);
        let mut store = ProfileStore::load_from(path.clone(), LegacyShellSettings::default());
        let custom = Profile::new("Removable".to_owned());
        let custom_id = custom.id;
        store.profiles.push(custom);

        assert!(
            !store.remove_inner(DEFAULT_PROFILE_ID),
            "the built-in Default profile must be undeletable"
        );
        assert_eq!(store.profiles().len(), 2, "nothing was removed");

        assert!(!store.remove_inner(ProfileId::new_v4()), "unknown id is a no-op");
        assert_eq!(store.profiles().len(), 2);

        assert!(store.remove_inner(custom_id), "a custom profile is removable");
        assert_eq!(store.profiles().len(), 1);
        assert_eq!(store.default_profile().id, DEFAULT_PROFILE_ID);

        let _ = fs::remove_file(&path);
    }
}
