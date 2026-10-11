use crate::settings::{Language, SidebarLayout, UpdateChannel};

impl Language {
    pub(crate) fn terminal_suggest(self) -> &'static str {
        match self {
            Self::English => "Command completion",
            Self::SimplifiedChinese => "命令补全",
        }
    }
    pub(crate) fn sidebar_layout_row(self) -> &'static str {
        match self {
            Self::English => "Sidebar layout",
            Self::SimplifiedChinese => "侧边栏布局",
        }
    }

    pub(crate) fn sidebar_layout_label(self, layout: SidebarLayout) -> &'static str {
        match (self, layout) {
            (Self::English, SidebarLayout::AllProjects) => "All projects",
            (Self::English, SidebarLayout::SingleProject) => "Single project",
            (Self::SimplifiedChinese, SidebarLayout::AllProjects) => "全部项目",
            (Self::SimplifiedChinese, SidebarLayout::SingleProject) => "单个项目",
        }
    }

    pub(crate) fn select_project(self) -> &'static str {
        match self {
            Self::English => "Switch project",
            Self::SimplifiedChinese => "切换项目",
        }
    }

    pub(crate) fn terminal_switcher(self) -> &'static str {
        match self {
            Self::English => "Search All Terminals…",
            Self::SimplifiedChinese => "搜索全部终端…",
        }
    }
    pub(crate) fn window_switcher(self) -> &'static str {
        match self {
            Self::English => "Switch Window…",
            Self::SimplifiedChinese => "切换窗口…",
        }
    }
    pub(crate) fn undo_window_operation(self) -> &'static str {
        match self {
            Self::English => "Undo Window / Tab Move",
            Self::SimplifiedChinese => "撤销窗口关闭或标签页移动",
        }
    }
    pub(crate) fn quit_and_terminate(self) -> &'static str {
        match self {
            Self::English => "Terminate All Terminals and Quit…",
            Self::SimplifiedChinese => "结束全部终端并退出…",
        }
    }
    // --- Common dialog buttons ---------------------------------------------------------------

    pub(crate) fn ok(self) -> &'static str {
        match self {
            Self::English => "OK",
            Self::SimplifiedChinese => "确定",
        }
    }

    pub(crate) fn cancel(self) -> &'static str {
        match self {
            Self::English => "Cancel",
            Self::SimplifiedChinese => "取消",
        }
    }

    pub(crate) fn open(self) -> &'static str {
        match self {
            Self::English => "Open",
            Self::SimplifiedChinese => "打开",
        }
    }

    pub(crate) fn dismiss(self) -> &'static str {
        match self {
            Self::English => "Dismiss",
            Self::SimplifiedChinese => "关闭",
        }
    }

    // --- App dialogs -------------------------------------------------------------------------

    pub(crate) fn allow_url_open_title(self) -> &'static str {
        match self {
            Self::English => "Allow terminal to open this URL?",
            Self::SimplifiedChinese => "允许终端打开此 URL？",
        }
    }

    pub(crate) fn url_open_detail(self) -> &'static str {
        match self {
            Self::English => "URL:",
            Self::SimplifiedChinese => "地址：",
        }
    }

    pub(crate) fn open_hyperlink_title(self) -> &'static str {
        match self {
            Self::English => "Open terminal hyperlink?",
            Self::SimplifiedChinese => "打开终端超链接？",
        }
    }

    pub(crate) fn hyperlink_open_detail(self) -> &'static str {
        match self {
            Self::English => "URL:",
            Self::SimplifiedChinese => "地址：",
        }
    }

    pub(crate) fn choose_receive_files_prompt(self) -> &'static str {
        match self {
            Self::English => "Choose where to receive terminal files",
            Self::SimplifiedChinese => "选择接收终端文件的位置",
        }
    }

    pub(crate) fn terminate_all(self) -> &'static str {
        match self {
            Self::English => "Terminate All",
            Self::SimplifiedChinese => "结束全部",
        }
    }

    pub(crate) fn new_project_title(self) -> &'static str {
        match self {
            Self::English => "New Project",
            Self::SimplifiedChinese => "新建项目",
        }
    }

    pub(crate) fn project_name_placeholder(self) -> &'static str {
        match self {
            Self::English => "Project name",
            Self::SimplifiedChinese => "项目名称",
        }
    }

    pub(crate) fn set_working_directory_prompt(self) -> &'static str {
        match self {
            Self::English => "Set Project Working Directory",
            Self::SimplifiedChinese => "设置项目工作目录",
        }
    }

    pub(crate) fn rename_project_title(self) -> &'static str {
        match self {
            Self::English => "Rename Project",
            Self::SimplifiedChinese => "重命名项目",
        }
    }

    // --- Tab custom title --------------------------------------------------------------------

    /// Tab context-menu item that opens the custom-title prompt.
    pub(crate) fn set_custom_title(self) -> &'static str {
        match self {
            Self::English => "Rename Tab…",
            Self::SimplifiedChinese => "自定义标题…",
        }
    }

    /// Tab context-menu item that clears the custom title (disabled when none is set).
    pub(crate) fn remove_custom_title(self) -> &'static str {
        match self {
            Self::English => "Remove Custom Title",
            Self::SimplifiedChinese => "移除自定义标题",
        }
    }

    pub(crate) fn custom_title_prompt_title(self) -> &'static str {
        match self {
            Self::English => "Rename Tab",
            Self::SimplifiedChinese => "自定义标签标题",
        }
    }

    pub(crate) fn custom_title_prompt_message(self) -> &'static str {
        match self {
            Self::English => "Leave empty to use the process title",
            Self::SimplifiedChinese => "留空则使用进程标题",
        }
    }

    pub(crate) fn custom_title_placeholder(self) -> &'static str {
        match self {
            Self::English => "Custom tab title",
            Self::SimplifiedChinese => "自定义标签标题",
        }
    }

    // --- Section labels ----------------------------------------------------------------------

    pub(crate) fn projects_label(self) -> &'static str {
        match self {
            Self::English => "PROJECTS",
            Self::SimplifiedChinese => "项目",
        }
    }

    /// Group heading in the detached-session popover for sessions whose project no longer exists.
    pub(crate) fn collapse_sidebar_tooltip(self) -> &'static str {
        match self {
            Self::English => "Collapse sidebar",
            Self::SimplifiedChinese => "折叠侧边栏",
        }
    }

    pub(crate) fn expand_sidebar_tooltip(self) -> &'static str {
        match self {
            Self::English => "Expand sidebar",
            Self::SimplifiedChinese => "展开侧边栏",
        }
    }

    pub(crate) fn new_terminal_tooltip(self) -> &'static str {
        match self {
            Self::English => "New terminal",
            Self::SimplifiedChinese => "新建终端",
        }
    }

    pub(crate) fn toggle_right_sidebar_tooltip(self) -> &'static str {
        match self {
            Self::English => "Toggle info panel",
            Self::SimplifiedChinese => "切换信息面板",
        }
    }

    pub(crate) fn add_project_tooltip(self) -> &'static str {
        match self {
            Self::English => "Add project",
            Self::SimplifiedChinese => "添加项目",
        }
    }

    pub(crate) fn ports_label(self) -> &'static str {
        match self {
            Self::English => "PORTS",
            Self::SimplifiedChinese => "端口",
        }
    }

    pub(crate) fn forward_port(self) -> &'static str {
        match self {
            Self::English => "Forward Port…",
            Self::SimplifiedChinese => "端口映射…",
        }
    }

    pub(crate) fn forward_port_prompt(self) -> &'static str {
        match self {
            Self::English => {
                "Enter a port forward: L 8080 localhost:80, R 8080 localhost:80, or D 1080"
            }
            Self::SimplifiedChinese => {
                "输入端口映射：L 8080 localhost:80（本地）、R 8080 localhost:80（远程）或 D 1080（动态）"
            }
        }
    }

    pub(crate) fn processes_label(self) -> &'static str {
        match self {
            Self::English => "PROCESSES",
            Self::SimplifiedChinese => "进程",
        }
    }

    // --- Right sidebar tabs ------------------------------------------------------------------

    pub(crate) fn info_tab(self) -> &'static str {
        match self {
            Self::English => "Info",
            Self::SimplifiedChinese => "信息",
        }
    }

    pub(crate) fn commands_tab(self) -> &'static str {
        match self {
            Self::English => "Commands",
            Self::SimplifiedChinese => "命令",
        }
    }

    pub(crate) fn notes_tab(self) -> &'static str {
        match self {
            Self::English => "Notes",
            Self::SimplifiedChinese => "便签",
        }
    }

    // --- Right sidebar content ---------------------------------------------------------------

    pub(crate) fn current_directory(self) -> &'static str {
        match self {
            Self::English => "Current directory",
            Self::SimplifiedChinese => "当前目录",
        }
    }

    pub(crate) fn initial_directory(self) -> &'static str {
        match self {
            Self::English => "Initial directory",
            Self::SimplifiedChinese => "初始目录",
        }
    }

    pub(crate) fn copy_path(self) -> &'static str {
        match self {
            Self::English => "Copy path",
            Self::SimplifiedChinese => "复制路径",
        }
    }

    pub(crate) fn open_with_finder(self) -> &'static str {
        match self {
            Self::English => "Finder",
            Self::SimplifiedChinese => "访达",
        }
    }

    pub(crate) fn open_with_vscode(self) -> &'static str {
        match self {
            Self::English => "VS Code",
            Self::SimplifiedChinese => "VS Code",
        }
    }

    pub(crate) fn loading_ports(self) -> &'static str {
        match self {
            Self::English => "Loading ports…",
            Self::SimplifiedChinese => "正在加载端口…",
        }
    }

    pub(crate) fn loading_process_info(self) -> &'static str {
        match self {
            Self::English => "Loading processes…",
            Self::SimplifiedChinese => "正在加载进程…",
        }
    }

    // --- Commands tab: history + snippets ----------------------------------------------------

    pub(crate) fn command_history_label(self) -> &'static str {
        match self {
            Self::English => "COMMAND HISTORY",
            Self::SimplifiedChinese => "命令历史",
        }
    }

    pub(crate) fn snippets_label(self) -> &'static str {
        match self {
            Self::English => "COMMAND SNIPPETS",
            Self::SimplifiedChinese => "命令片段",
        }
    }

    pub(crate) fn command_history_empty(self) -> &'static str {
        match self {
            Self::English => "No commands yet",
            Self::SimplifiedChinese => "暂无命令",
        }
    }

    pub(crate) fn shell_integration_unavailable(self) -> &'static str {
        match self {
            Self::English => "Enable shell integration to view command history",
            Self::SimplifiedChinese => "启用 Shell 集成后可查看命令历史",
        }
    }

    pub(crate) fn command_running(self) -> &'static str {
        match self {
            Self::English => "running",
            Self::SimplifiedChinese => "运行中",
        }
    }

    pub(crate) fn command_no_text(self) -> &'static str {
        match self {
            Self::English => "(command output)",
            Self::SimplifiedChinese => "(命令输出)",
        }
    }

    pub(crate) fn snippets_empty(self) -> &'static str {
        match self {
            Self::English => "No command snippets yet",
            Self::SimplifiedChinese => "暂无命令片段",
        }
    }

    pub(crate) fn add_snippet(self) -> &'static str {
        match self {
            Self::English => "Add command snippet",
            Self::SimplifiedChinese => "新增命令片段",
        }
    }

    pub(crate) fn use_snippet(self) -> &'static str {
        match self {
            Self::English => "Use",
            Self::SimplifiedChinese => "使用",
        }
    }

    pub(crate) fn edit_snippet(self) -> &'static str {
        match self {
            Self::English => "Edit command snippet",
            Self::SimplifiedChinese => "编辑命令片段",
        }
    }

    pub(crate) fn delete_snippet(self) -> &'static str {
        match self {
            Self::English => "Delete command snippet",
            Self::SimplifiedChinese => "删除命令片段",
        }
    }

    pub(crate) fn snippet_name_placeholder(self) -> &'static str {
        match self {
            Self::English => "Snippet name",
            Self::SimplifiedChinese => "片段名称",
        }
    }

    pub(crate) fn snippet_description(self) -> &'static str {
        match self {
            Self::English => "Click a snippet to fill its command into the current terminal",
            Self::SimplifiedChinese => "点击片段可将命令填入当前终端",
        }
    }

    pub(crate) fn snippet_command_label(self) -> &'static str {
        match self {
            Self::English => "Command, e.g. npm run dev",
            Self::SimplifiedChinese => "命令，例如 npm run dev",
        }
    }

    pub(crate) fn auto_run_label(self) -> &'static str {
        match self {
            Self::English => "Run when used",
            Self::SimplifiedChinese => "使用时自动执行",
        }
    }

    pub(crate) fn save(self) -> &'static str {
        match self {
            Self::English => "Save",
            Self::SimplifiedChinese => "保存",
        }
    }

    pub(crate) fn delete_snippet_confirm_title(self) -> &'static str {
        match self {
            Self::English => "Delete command snippet?",
            Self::SimplifiedChinese => "删除命令片段？",
        }
    }

    pub(crate) fn delete_snippet_confirm_message(self) -> &'static str {
        match self {
            Self::English => "This cannot be undone.",
            Self::SimplifiedChinese => "删除后无法恢复。",
        }
    }

    pub(crate) fn delete(self) -> &'static str {
        match self {
            Self::English => "Delete",
            Self::SimplifiedChinese => "删除",
        }
    }

    // --- Notes tab ---------------------------------------------------------------------------

    pub(crate) fn notes_placeholder(self) -> &'static str {
        match self {
            Self::English => "Saved automatically; Shared across windows",
            Self::SimplifiedChinese => "自动保存，所有窗口共享",
        }
    }

    pub(crate) fn no_running_processes(self) -> &'static str {
        match self {
            Self::English => "No running processes",
            Self::SimplifiedChinese => "暂无运行中的进程",
        }
    }

    pub(crate) fn pid_label(self) -> &'static str {
        match self {
            Self::English => "PID",
            Self::SimplifiedChinese => "PID",
        }
    }

    // --- Progress states ---------------------------------------------------------------------

    pub(crate) fn progress_complete(self) -> &'static str {
        match self {
            Self::English => "Complete",
            Self::SimplifiedChinese => "已完成",
        }
    }

    pub(crate) fn progress_running(self) -> &'static str {
        match self {
            Self::English => "Running",
            Self::SimplifiedChinese => "运行中",
        }
    }

    pub(crate) fn progress_error(self) -> &'static str {
        match self {
            Self::English => "Error",
            Self::SimplifiedChinese => "错误",
        }
    }

    pub(crate) fn progress_indeterminate(self) -> &'static str {
        match self {
            Self::English => "In progress",
            Self::SimplifiedChinese => "进行中",
        }
    }

    pub(crate) fn progress_paused(self) -> &'static str {
        match self {
            Self::English => "Paused",
            Self::SimplifiedChinese => "已暂停",
        }
    }

    pub(crate) fn updated_just_now(self) -> &'static str {
        match self {
            Self::English => "updated just now",
            Self::SimplifiedChinese => "刚刚更新",
        }
    }

    pub(crate) fn updated_seconds_ago(self, seconds: u64) -> String {
        match self {
            Self::English => format!("updated {seconds}s ago"),
            Self::SimplifiedChinese => format!("{seconds} 秒前更新"),
        }
    }

    pub(crate) fn updated_minutes_ago(self, minutes: u64) -> String {
        match self {
            Self::English => format!("updated {minutes}m ago"),
            Self::SimplifiedChinese => format!("{minutes} 分钟前更新"),
        }
    }

    pub(crate) fn updated_hours_ago(self, hours: u64) -> String {
        match self {
            Self::English => format!("updated {hours}h ago"),
            Self::SimplifiedChinese => format!("{hours} 小时前更新"),
        }
    }

    // --- Native context menus ----------------------------------------------------------------

    pub(crate) fn split_up(self) -> &'static str {
        match self {
            Self::English => "Split Up",
            Self::SimplifiedChinese => "向上拆分",
        }
    }

    pub(crate) fn split_down(self) -> &'static str {
        match self {
            Self::English => "Split Down",
            Self::SimplifiedChinese => "向下拆分",
        }
    }

    pub(crate) fn split_left(self) -> &'static str {
        match self {
            Self::English => "Split Left",
            Self::SimplifiedChinese => "向左拆分",
        }
    }

    pub(crate) fn split_right(self) -> &'static str {
        match self {
            Self::English => "Split Right",
            Self::SimplifiedChinese => "向右拆分",
        }
    }

    pub(crate) fn move_up(self) -> &'static str {
        match self {
            Self::English => "Move Up",
            Self::SimplifiedChinese => "上移",
        }
    }

    pub(crate) fn move_down(self) -> &'static str {
        match self {
            Self::English => "Move Down",
            Self::SimplifiedChinese => "下移",
        }
    }

    pub(crate) fn move_left(self) -> &'static str {
        match self {
            Self::English => "Move Left",
            Self::SimplifiedChinese => "左移",
        }
    }

    pub(crate) fn move_right(self) -> &'static str {
        match self {
            Self::English => "Move Right",
            Self::SimplifiedChinese => "右移",
        }
    }

    pub(crate) fn split_and_move(self) -> &'static str {
        match self {
            Self::English => "Split and Move",
            Self::SimplifiedChinese => "拆分和移动",
        }
    }

    pub(crate) fn edit_name(self) -> &'static str {
        match self {
            Self::English => "Edit Name",
            Self::SimplifiedChinese => "编辑名称",
        }
    }

    pub(crate) fn set_root(self) -> &'static str {
        match self {
            Self::English => "Set Working Directory…",
            Self::SimplifiedChinese => "设置工作目录…",
        }
    }

    pub(crate) fn close_tabs(self, count: usize) -> String {
        match self {
            Self::English => format!(
                "Terminate {count} project terminal{}…",
                if count == 1 { "" } else { "s" }
            ),
            Self::SimplifiedChinese => format!("结束项目的 {} 个终端…", count),
        }
    }

    pub(crate) fn delete_project(self) -> &'static str {
        match self {
            Self::English => "Delete Project",
            Self::SimplifiedChinese => "删除项目",
        }
    }

    pub(crate) fn terminate(self) -> &'static str {
        match self {
            Self::English => "Terminate",
            Self::SimplifiedChinese => "终止",
        }
    }

    pub(crate) fn force_kill(self) -> &'static str {
        match self {
            Self::English => "Force Kill",
            Self::SimplifiedChinese => "强制结束",
        }
    }

    pub(crate) fn copy_pid(self) -> &'static str {
        match self {
            Self::English => "Copy PID",
            Self::SimplifiedChinese => "复制 PID",
        }
    }

    pub(crate) fn copy_executable_path(self) -> &'static str {
        match self {
            Self::English => "Copy Executable Path",
            Self::SimplifiedChinese => "复制可执行文件路径",
        }
    }

    // --- Settings window ---------------------------------------------------------------------

    pub(crate) fn settings_title(self) -> &'static str {
        match self {
            Self::English => "Settings",
            Self::SimplifiedChinese => "设置",
        }
    }

    pub(crate) fn settings_menu_item(self) -> &'static str {
        match self {
            Self::English => "Settings…",
            Self::SimplifiedChinese => "设置…",
        }
    }

    pub(crate) fn hide_eggie(self) -> &'static str {
        match self {
            Self::English => "Hide Eggie",
            Self::SimplifiedChinese => "隐藏 Eggie",
        }
    }

    pub(crate) fn hide_others(self) -> &'static str {
        match self {
            Self::English => "Hide Others",
            Self::SimplifiedChinese => "隐藏其他",
        }
    }

    pub(crate) fn show_all(self) -> &'static str {
        match self {
            Self::English => "Show All",
            Self::SimplifiedChinese => "全部显示",
        }
    }

    pub(crate) fn quit_eggie(self) -> &'static str {
        match self {
            Self::English => "Quit Eggie",
            Self::SimplifiedChinese => "退出 Eggie",
        }
    }

    pub(crate) fn edit_menu(self) -> &'static str {
        match self {
            Self::English => "Edit",
            Self::SimplifiedChinese => "编辑",
        }
    }

    pub(crate) fn file_menu(self) -> &'static str {
        match self {
            Self::English => "File",
            Self::SimplifiedChinese => "文件",
        }
    }

    pub(crate) fn new_window_menu_item(self) -> &'static str {
        match self {
            Self::English => "New Window",
            Self::SimplifiedChinese => "新建窗口",
        }
    }

    // --- Menu bar: menu titles and menu-only item labels -----------------------------------
    // Items that map to a configurable action reuse that action's `action_*` label instead
    // (e.g. New Tab -> `action_new_tab`), so only the menu-only strings live here.

    pub(crate) fn view_menu(self) -> &'static str {
        match self {
            Self::English => "View",
            Self::SimplifiedChinese => "显示",
        }
    }

    pub(crate) fn window_menu(self) -> &'static str {
        match self {
            Self::English => "Window",
            Self::SimplifiedChinese => "窗口",
        }
    }

    pub(crate) fn help_menu(self) -> &'static str {
        match self {
            Self::English => "Help",
            Self::SimplifiedChinese => "帮助",
        }
    }

    pub(crate) fn about_eggie(self) -> &'static str {
        match self {
            Self::English => "About Eggie",
            Self::SimplifiedChinese => "关于 Eggie",
        }
    }

    pub(crate) fn secure_keyboard_entry(self) -> &'static str {
        match self {
            Self::English => "Secure Keyboard Entry",
            Self::SimplifiedChinese => "安全键盘输入",
        }
    }

    pub(crate) fn close_all_windows_menu_item(self) -> &'static str {
        match self {
            Self::English => "Close All Windows",
            Self::SimplifiedChinese => "关闭全部窗口",
        }
    }

    /// The Edit-menu Find item uses the ellipsis form (it opens the search bar), distinct from
    /// the bare `action_terminal_find` label shown on the keybindings page.
    pub(crate) fn find_menu_item(self) -> &'static str {
        match self {
            Self::English => "Find…",
            Self::SimplifiedChinese => "查找…",
        }
    }

    pub(crate) fn zoom_menu_item(self) -> &'static str {
        match self {
            Self::English => "Zoom",
            Self::SimplifiedChinese => "缩放",
        }
    }

    pub(crate) fn help_docs_menu_item(self) -> &'static str {
        match self {
            Self::English => "Eggie Help",
            Self::SimplifiedChinese => "Eggie 帮助",
        }
    }

    pub(crate) fn check_for_updates_menu_item(self) -> &'static str {
        match self {
            Self::English => "Check for Updates…",
            Self::SimplifiedChinese => "检查更新…",
        }
    }

    pub(crate) fn update_window_title(self) -> &'static str {
        match self {
            Self::English => "Eggie Update",
            Self::SimplifiedChinese => "Eggie 更新",
        }
    }

    pub(crate) fn update_current_version(self) -> &'static str {
        match self {
            Self::English => "Current version",
            Self::SimplifiedChinese => "当前版本",
        }
    }

    pub(crate) fn update_new_version(self) -> &'static str {
        match self {
            Self::English => "New version",
            Self::SimplifiedChinese => "新版本",
        }
    }

    pub(crate) fn update_release_notes(self) -> &'static str {
        match self {
            Self::English => "What's new",
            Self::SimplifiedChinese => "更新内容",
        }
    }

    pub(crate) fn update_checking(self) -> &'static str {
        match self {
            Self::English => "Checking for updates…",
            Self::SimplifiedChinese => "正在检查更新…",
        }
    }

    pub(crate) fn update_up_to_date(self) -> &'static str {
        match self {
            Self::English => "Up to date",
            Self::SimplifiedChinese => "已是最新版本",
        }
    }

    pub(crate) fn update_no_pending(self) -> &'static str {
        match self {
            Self::English => "No pending update",
            Self::SimplifiedChinese => "暂无待处理更新",
        }
    }

    pub(crate) fn update_downloading(self) -> &'static str {
        match self {
            Self::English => "Downloading update…",
            Self::SimplifiedChinese => "正在下载更新…",
        }
    }

    pub(crate) fn update_download_and_install(self) -> &'static str {
        match self {
            Self::English => "Download and Install",
            Self::SimplifiedChinese => "下载并安装",
        }
    }

    pub(crate) fn update_later(self) -> &'static str {
        match self {
            Self::English => "Later",
            Self::SimplifiedChinese => "稍后",
        }
    }

    pub(crate) fn update_restart(self) -> &'static str {
        match self {
            Self::English => "Restart",
            Self::SimplifiedChinese => "重启",
        }
    }

    pub(crate) fn update_restart_with_daemon(self) -> &'static str {
        match self {
            Self::English => "Restart and Update",
            Self::SimplifiedChinese => "重启并更新",
        }
    }

    pub(crate) fn update_ready_restart(self) -> &'static str {
        match self {
            Self::English => "Update ready. Restart to apply.",
            Self::SimplifiedChinese => "更新已就绪，重启后生效。",
        }
    }

    pub(crate) fn update_ready_restart_with_daemon(self) -> &'static str {
        match self {
            Self::English => "Update ready. Restart to update the background service.",
            Self::SimplifiedChinese => "更新已就绪，重启将更新后台服务。",
        }
    }

    pub(crate) fn update_daemon_warning(self) -> &'static str {
        match self {
            Self::English => "Updating the background service will end all terminal sessions.",
            Self::SimplifiedChinese => "更新后台服务将结束全部终端会话。",
        }
    }

    pub(crate) fn update_error_title(self) -> &'static str {
        match self {
            Self::English => "Update failed",
            Self::SimplifiedChinese => "更新失败",
        }
    }

    pub(crate) fn update_retry(self) -> &'static str {
        match self {
            Self::English => "Retry",
            Self::SimplifiedChinese => "重试",
        }
    }

    pub(crate) fn update_available_tooltip(self) -> &'static str {
        match self {
            Self::English => "Update available",
            Self::SimplifiedChinese => "有可用更新",
        }
    }

    pub(crate) fn update_ready_tooltip(self) -> &'static str {
        match self {
            Self::English => "Restart to apply update",
            Self::SimplifiedChinese => "重启以完成更新",
        }
    }

    pub(crate) fn auto_check_updates_row(self) -> &'static str {
        match self {
            Self::English => "Automatically check for updates",
            Self::SimplifiedChinese => "自动检查更新",
        }
    }

    pub(crate) fn auto_check_updates_description(self) -> &'static str {
        match self {
            Self::English => "Check for updates on launch",
            Self::SimplifiedChinese => "启动时检查新版本",
        }
    }

    pub(crate) fn updates_section(self) -> &'static str {
        match self {
            Self::English => "Updates",
            Self::SimplifiedChinese => "更新",
        }
    }

    pub(crate) fn update_channel_row(self) -> &'static str {
        match self {
            Self::English => "Update channel",
            Self::SimplifiedChinese => "更新渠道",
        }
    }

    pub(crate) fn update_channel_description(self) -> &'static str {
        match self {
            Self::English => "Beta includes prereleases",
            Self::SimplifiedChinese => "测试版包含预发布版本",
        }
    }

    pub(crate) fn update_channel_label(self, channel: UpdateChannel) -> &'static str {
        match (self, channel) {
            (Self::English, UpdateChannel::Stable) => "Stable",
            (Self::English, UpdateChannel::Beta) => "Beta",
            (Self::SimplifiedChinese, UpdateChannel::Stable) => "稳定版",
            (Self::SimplifiedChinese, UpdateChannel::Beta) => "测试版",
        }
    }

    pub(crate) fn about_sidebar(self) -> &'static str {
        match self {
            Self::English => "About",
            Self::SimplifiedChinese => "关于",
        }
    }

    pub(crate) fn version_row(self) -> &'static str {
        match self {
            Self::English => "Version",
            Self::SimplifiedChinese => "版本",
        }
    }

    pub(crate) fn protocol_version_row(self) -> &'static str {
        match self {
            Self::English => "Protocol version",
            Self::SimplifiedChinese => "协议版本",
        }
    }

    pub(crate) fn build_date_row(self) -> &'static str {
        match self {
            Self::English => "Build date",
            Self::SimplifiedChinese => "构建日期",
        }
    }

    pub(crate) fn build_id_row(self) -> &'static str {
        match self {
            Self::English => "Build ID",
            Self::SimplifiedChinese => "构建标识",
        }
    }

    pub(crate) fn build_mode_row(self) -> &'static str {
        match self {
            Self::English => "Build mode",
            Self::SimplifiedChinese => "构建模式",
        }
    }

    pub(crate) fn build_mode_debug(self) -> &'static str {
        match self {
            Self::English => "Debug",
            Self::SimplifiedChinese => "调试",
        }
    }

    pub(crate) fn build_mode_release(self) -> &'static str {
        match self {
            Self::English => "Release",
            Self::SimplifiedChinese => "发布",
        }
    }

    pub(crate) fn check_for_updates_button(self) -> &'static str {
        match self {
            Self::English => "Check for updates",
            Self::SimplifiedChinese => "检查更新",
        }
    }

    pub(crate) fn build_info_section(self) -> &'static str {
        match self {
            Self::English => "Build info",
            Self::SimplifiedChinese => "构建信息",
        }
    }

    pub(crate) fn cut(self) -> &'static str {
        match self {
            Self::English => "Cut",
            Self::SimplifiedChinese => "剪切",
        }
    }

    pub(crate) fn copy(self) -> &'static str {
        match self {
            Self::English => "Copy",
            Self::SimplifiedChinese => "拷贝",
        }
    }

    pub(crate) fn paste(self) -> &'static str {
        match self {
            Self::English => "Paste",
            Self::SimplifiedChinese => "粘贴",
        }
    }

    pub(crate) fn select_all(self) -> &'static str {
        match self {
            Self::English => "Select All",
            Self::SimplifiedChinese => "全选",
        }
    }

    pub(crate) fn settings_window_title(self) -> String {
        match self {
            Self::English => "Eggie — Settings".to_owned(),
            Self::SimplifiedChinese => "Eggie — 设置".to_owned(),
        }
    }

    pub(crate) fn general_sidebar(self) -> &'static str {
        match self {
            Self::English => "General",
            Self::SimplifiedChinese => "通用",
        }
    }

    pub(crate) fn appearance_sidebar(self) -> &'static str {
        match self {
            Self::English => "Appearance",
            Self::SimplifiedChinese => "外观",
        }
    }

    pub(crate) fn advanced_sidebar(self) -> &'static str {
        match self {
            Self::English => "Advanced",
            Self::SimplifiedChinese => "高级",
        }
    }

    pub(crate) fn general_section(self) -> &'static str {
        match self {
            Self::English => "General",
            Self::SimplifiedChinese => "通用",
        }
    }

    pub(crate) fn appearance_section(self) -> &'static str {
        match self {
            Self::English => "Appearance",
            Self::SimplifiedChinese => "外观",
        }
    }

    pub(crate) fn theme_section(self) -> &'static str {
        match self {
            Self::English => "Theme",
            Self::SimplifiedChinese => "主题",
        }
    }

    pub(crate) fn terminal_text_section(self) -> &'static str {
        match self {
            Self::English => "Terminal text",
            Self::SimplifiedChinese => "终端文本",
        }
    }

    pub(crate) fn terminal_layout_section(self) -> &'static str {
        match self {
            Self::English => "Terminal layout",
            Self::SimplifiedChinese => "终端布局",
        }
    }

    pub(crate) fn progress_indicators_section(self) -> &'static str {
        match self {
            Self::English => "Progress indicators",
            Self::SimplifiedChinese => "进度指示器",
        }
    }

    pub(crate) fn terminal_security_section(self) -> &'static str {
        match self {
            Self::English => "Terminal security",
            Self::SimplifiedChinese => "终端安全",
        }
    }

    pub(crate) fn advanced_section(self) -> &'static str {
        match self {
            Self::English => "Advanced",
            Self::SimplifiedChinese => "高级",
        }
    }

    pub(crate) fn language_row(self) -> &'static str {
        match self {
            Self::English => "Language",
            Self::SimplifiedChinese => "语言",
        }
    }

    pub(crate) fn theme_row(self) -> &'static str {
        match self {
            Self::English => "Interface theme",
            Self::SimplifiedChinese => "界面主题",
        }
    }

    pub(crate) fn interface_dark_theme_row(self) -> &'static str {
        match self {
            Self::English => "Interface dark theme",
            Self::SimplifiedChinese => "界面深色主题",
        }
    }

    pub(crate) fn interface_light_theme_row(self) -> &'static str {
        match self {
            Self::English => "Interface light theme",
            Self::SimplifiedChinese => "界面浅色主题",
        }
    }

    pub(crate) fn terminal_theme_source_row(self) -> &'static str {
        match self {
            Self::English => "Terminal theme",
            Self::SimplifiedChinese => "终端主题",
        }
    }

    pub(crate) fn terminal_theme_follow_interface(self) -> &'static str {
        match self {
            Self::English => "Follow interface",
            Self::SimplifiedChinese => "跟随界面",
        }
    }

    pub(crate) fn terminal_theme_custom(self) -> &'static str {
        match self {
            Self::English => "Custom",
            Self::SimplifiedChinese => "自定义",
        }
    }

    pub(crate) fn terminal_dark_theme_row(self) -> &'static str {
        match self {
            Self::English => "Terminal dark theme",
            Self::SimplifiedChinese => "终端深色主题",
        }
    }

    pub(crate) fn terminal_light_theme_row(self) -> &'static str {
        match self {
            Self::English => "Terminal light theme",
            Self::SimplifiedChinese => "终端浅色主题",
        }
    }

    pub(crate) fn minimum_contrast_row(self) -> &'static str {
        match self {
            Self::English => "Minimum contrast",
            Self::SimplifiedChinese => "最小对比度",
        }
    }

    pub(crate) fn minimum_contrast_description(self) -> &'static str {
        match self {
            Self::English => "Text only; excludes emoji and graphics",
            Self::SimplifiedChinese => "仅调整文字，不影响 Emoji 和图形",
        }
    }

    pub(crate) fn font_row(self) -> &'static str {
        match self {
            Self::English => "Font",
            Self::SimplifiedChinese => "字体",
        }
    }

    pub(crate) fn font_description(self) -> &'static str {
        match self {
            Self::English => "Installed monospaced fonts",
            Self::SimplifiedChinese => "已安装的等宽字体",
        }
    }

    pub(crate) fn font_family_use_regular(self) -> &'static str {
        match self {
            Self::English => "Use regular font",
            Self::SimplifiedChinese => "跟随常规字体",
        }
    }

    pub(crate) fn font_bold_row(self) -> &'static str {
        match self {
            Self::English => "Bold font",
            Self::SimplifiedChinese => "粗体字体",
        }
    }

    pub(crate) fn font_italic_row(self) -> &'static str {
        match self {
            Self::English => "Italic font",
            Self::SimplifiedChinese => "斜体字体",
        }
    }

    pub(crate) fn font_bold_italic_row(self) -> &'static str {
        match self {
            Self::English => "Bold italic font",
            Self::SimplifiedChinese => "粗斜体字体",
        }
    }

    pub(crate) fn synthetic_bold_row(self) -> &'static str {
        match self {
            Self::English => "Synthesize bold",
            Self::SimplifiedChinese => "合成粗体",
        }
    }

    pub(crate) fn synthetic_bold_description(self) -> &'static str {
        match self {
            Self::English => "Simulate bold when no bold font is available",
            Self::SimplifiedChinese => "无可用粗体字体时模拟粗体",
        }
    }

    pub(crate) fn synthetic_italic_row(self) -> &'static str {
        match self {
            Self::English => "Synthesize italic",
            Self::SimplifiedChinese => "合成斜体",
        }
    }

    pub(crate) fn synthetic_italic_description(self) -> &'static str {
        match self {
            Self::English => "Simulate italic when no italic font is available",
            Self::SimplifiedChinese => "无可用斜体字体时模拟斜体",
        }
    }

    pub(crate) fn synthetic_bold_italic_row(self) -> &'static str {
        match self {
            Self::English => "Synthesize bold italic",
            Self::SimplifiedChinese => "合成粗斜体",
        }
    }

    pub(crate) fn synthetic_bold_italic_description(self) -> &'static str {
        match self {
            Self::English => "Simulate bold italic when no matching font is available",
            Self::SimplifiedChinese => "无可用粗斜体字体时模拟粗斜体",
        }
    }

    pub(crate) fn ligatures_row(self) -> &'static str {
        match self {
            Self::English => "Ligatures",
            Self::SimplifiedChinese => "连字",
        }
    }

    pub(crate) fn ligatures_description(self) -> &'static str {
        match self {
            Self::English => "Combine symbols such as ->; Requires a font with ligatures",
            Self::SimplifiedChinese => "合并 -> 等符号，需字体支持连字",
        }
    }

    pub(crate) fn shaping_break_row(self) -> &'static str {
        match self {
            Self::English => "Break ligature at cursor",
            Self::SimplifiedChinese => "光标处拆分连字",
        }
    }

    pub(crate) fn font_thicken_row(self) -> &'static str {
        match self {
            Self::English => "Thicken",
            Self::SimplifiedChinese => "字体加粗",
        }
    }

    pub(crate) fn font_thicken_strength_row(self) -> &'static str {
        match self {
            Self::English => "Thicken strength",
            Self::SimplifiedChinese => "加粗强度",
        }
    }

    pub(crate) fn font_size_row(self) -> &'static str {
        match self {
            Self::English => "Font size",
            Self::SimplifiedChinese => "字号",
        }
    }

    pub(crate) fn horizontal_padding_row(self) -> &'static str {
        match self {
            Self::English => "Horizontal padding",
            Self::SimplifiedChinese => "水平内边距",
        }
    }

    pub(crate) fn vertical_padding_row(self) -> &'static str {
        match self {
            Self::English => "Vertical padding",
            Self::SimplifiedChinese => "垂直内边距",
        }
    }

    pub(crate) fn font_metrics_section(self) -> &'static str {
        match self {
            Self::English => "Font metrics",
            Self::SimplifiedChinese => "字体度量",
        }
    }

    pub(crate) fn adjust_cell_width_row(self) -> &'static str {
        match self {
            Self::English => "Cell width",
            Self::SimplifiedChinese => "单元格宽度",
        }
    }

    pub(crate) fn adjust_cell_width_description(self) -> &'static str {
        match self {
            Self::English => "Adjust in pixels; changes the column count",
            Self::SimplifiedChinese => "按像素调整，会改变终端列数",
        }
    }

    pub(crate) fn adjust_cell_height_row(self) -> &'static str {
        match self {
            Self::English => "Cell height",
            Self::SimplifiedChinese => "单元格高度",
        }
    }

    pub(crate) fn adjust_cell_height_description(self) -> &'static str {
        match self {
            Self::English => "Adjust in pixels; changes the row count",
            Self::SimplifiedChinese => "按像素调整，会改变终端行数",
        }
    }

    pub(crate) fn adjust_font_baseline_row(self) -> &'static str {
        match self {
            Self::English => "Font baseline",
            Self::SimplifiedChinese => "字体基线",
        }
    }

    pub(crate) fn adjust_font_baseline_description(self) -> &'static str {
        match self {
            Self::English => "Move text vertically, in pixels",
            Self::SimplifiedChinese => "按像素调整文字的垂直位置",
        }
    }

    pub(crate) fn adjust_underline_position_row(self) -> &'static str {
        match self {
            Self::English => "Underline position",
            Self::SimplifiedChinese => "下划线位置",
        }
    }

    pub(crate) fn adjust_underline_position_description(self) -> &'static str {
        match self {
            Self::English => "Move the underline vertically, in pixels",
            Self::SimplifiedChinese => "按像素调整下划线的垂直位置",
        }
    }

    pub(crate) fn adjust_underline_thickness_row(self) -> &'static str {
        match self {
            Self::English => "Underline thickness",
            Self::SimplifiedChinese => "下划线粗细",
        }
    }

    pub(crate) fn adjust_underline_thickness_description(self) -> &'static str {
        match self {
            Self::English => "Adjust line thickness in pixels",
            Self::SimplifiedChinese => "按像素调整线条粗细",
        }
    }

    pub(crate) fn adjust_strikethrough_position_row(self) -> &'static str {
        match self {
            Self::English => "Strikethrough position",
            Self::SimplifiedChinese => "删除线位置",
        }
    }

    pub(crate) fn adjust_strikethrough_position_description(self) -> &'static str {
        match self {
            Self::English => "Move the strikethrough vertically, in pixels",
            Self::SimplifiedChinese => "按像素调整删除线的垂直位置",
        }
    }

    pub(crate) fn adjust_strikethrough_thickness_row(self) -> &'static str {
        match self {
            Self::English => "Strikethrough thickness",
            Self::SimplifiedChinese => "删除线粗细",
        }
    }

    pub(crate) fn adjust_strikethrough_thickness_description(self) -> &'static str {
        match self {
            Self::English => "Adjust line thickness in pixels",
            Self::SimplifiedChinese => "按像素调整线条粗细",
        }
    }

    pub(crate) fn adjust_cursor_thickness_row(self) -> &'static str {
        match self {
            Self::English => "Cursor thickness",
            Self::SimplifiedChinese => "光标粗细",
        }
    }

    pub(crate) fn adjust_cursor_thickness_description(self) -> &'static str {
        match self {
            Self::English => "Adjust bar, underline, or hollow cursor strokes in pixels",
            Self::SimplifiedChinese => "按像素调整竖线、下划线或空心光标的粗细",
        }
    }

    pub(crate) fn adjust_box_thickness_row(self) -> &'static str {
        match self {
            Self::English => "Box-drawing thickness",
            Self::SimplifiedChinese => "框线粗细",
        }
    }

    pub(crate) fn adjust_box_thickness_description(self) -> &'static str {
        match self {
            Self::English => "Adjust box-drawing strokes in pixels",
            Self::SimplifiedChinese => "按像素调整框线字符的线条粗细",
        }
    }

    pub(crate) fn adjust_icon_height_row(self) -> &'static str {
        match self {
            Self::English => "Nerd Font icon height",
            Self::SimplifiedChinese => "Nerd Font 图标高度",
        }
    }

    pub(crate) fn adjust_icon_height_description(self) -> &'static str {
        match self {
            Self::English => "Adjust in pixels or percent",
            Self::SimplifiedChinese => "按像素或百分比调整",
        }
    }

    pub(crate) fn completed_timeout_row(self) -> &'static str {
        match self {
            Self::English => "Hide completed progress after",
            Self::SimplifiedChinese => "完成后隐藏",
        }
    }

    pub(crate) fn inactive_timeout_row(self) -> &'static str {
        match self {
            Self::English => "Hide inactive progress after",
            Self::SimplifiedChinese => "无更新时隐藏",
        }
    }

    pub(crate) fn osc_clipboard_read_row(self) -> &'static str {
        match self {
            Self::English => "OSC clipboard read",
            Self::SimplifiedChinese => "OSC 剪贴板读取",
        }
    }

    pub(crate) fn osc_clipboard_read_description(self) -> &'static str {
        match self {
            Self::English => "Controls reading only; writing stays enabled",
            Self::SimplifiedChinese => "仅控制读取，写入始终可用",
        }
    }

    pub(crate) fn block(self) -> &'static str {
        match self {
            Self::English => "Block",
            Self::SimplifiedChinese => "阻止",
        }
    }

    pub(crate) fn allow(self) -> &'static str {
        match self {
            Self::English => "Allow",
            Self::SimplifiedChinese => "允许",
        }
    }

    pub(crate) fn enabled(self) -> &'static str {
        match self {
            Self::English => "Enabled",
            Self::SimplifiedChinese => "启用",
        }
    }

    pub(crate) fn disabled(self) -> &'static str {
        match self {
            Self::English => "Disabled",
            Self::SimplifiedChinese => "禁用",
        }
    }

    pub(crate) fn terminal_behavior_section(self) -> &'static str {
        match self {
            Self::English => "Terminal behavior",
            Self::SimplifiedChinese => "终端行为",
        }
    }

    pub(crate) fn detect_urls_row(self) -> &'static str {
        match self {
            Self::English => "Detect URLs",
            Self::SimplifiedChinese => "URL 检测",
        }
    }

    pub(crate) fn detect_urls_description(self) -> &'static str {
        match self {
            Self::English => "Cmd/Ctrl+click to open links",
            Self::SimplifiedChinese => "Cmd/Ctrl+点击打开链接",
        }
    }

    pub(crate) fn copy_on_select_row(self) -> &'static str {
        match self {
            Self::English => "Copy on select",
            Self::SimplifiedChinese => "选中即复制",
        }
    }

    pub(crate) fn cursor_section(self) -> &'static str {
        match self {
            Self::English => "Cursor",
            Self::SimplifiedChinese => "光标",
        }
    }

    pub(crate) fn cursor_shape_row(self) -> &'static str {
        match self {
            Self::English => "Cursor shape",
            Self::SimplifiedChinese => "光标形状",
        }
    }

    pub(crate) fn cursor_shape_description(self) -> &'static str {
        match self {
            Self::English => "Default shape; programs may override it",
            Self::SimplifiedChinese => "默认形状，程序可自行更改",
        }
    }

    pub(crate) fn scrollback_lines_row(self) -> &'static str {
        match self {
            Self::English => "Scrollback lines",
            Self::SimplifiedChinese => "回滚行数",
        }
    }

    pub(crate) fn scrollback_lines_description(self) -> &'static str {
        match self {
            Self::English => "0 disables history",
            Self::SimplifiedChinese => "0 表示不保留历史",
        }
    }

    pub(crate) fn shell_section(self) -> &'static str {
        match self {
            Self::English => "Shell",
            Self::SimplifiedChinese => "Shell",
        }
    }

    pub(crate) fn shell_program_row(self) -> &'static str {
        match self {
            Self::English => "Shell program",
            Self::SimplifiedChinese => "Shell 程序",
        }
    }

    pub(crate) fn shell_program_description(self) -> &'static str {
        match self {
            Self::English => "Leave empty to use $SHELL; Applies to new terminals",
            Self::SimplifiedChinese => "留空使用 $SHELL，仅对新建终端生效",
        }
    }

    pub(crate) fn shell_args_row(self) -> &'static str {
        match self {
            Self::English => "Shell arguments",
            Self::SimplifiedChinese => "Shell 参数",
        }
    }

    pub(crate) fn shell_args_description(self) -> &'static str {
        match self {
            Self::English => {
                "Separate arguments with spaces; Leave empty for defaults; Applies to new terminals"
            }
            Self::SimplifiedChinese => "参数以空格分隔，留空使用默认值；仅对新建终端生效",
        }
    }

    pub(crate) fn shell_integration_path_row(self) -> &'static str {
        match self {
            Self::English => "Add Eggie to PATH",
            Self::SimplifiedChinese => "将 Eggie 加入 PATH",
        }
    }

    pub(crate) fn shell_integration_path_description(self) -> &'static str {
        match self {
            Self::English => "Make the eggie command available in new terminals",
            Self::SimplifiedChinese => "在新建终端中可直接使用 eggie 命令",
        }
    }

    pub(crate) fn shell_program_placeholder(self) -> &'static str {
        match self {
            Self::English => "e.g. /opt/homebrew/bin/fish",
            Self::SimplifiedChinese => "例如 /opt/homebrew/bin/fish",
        }
    }

    pub(crate) fn shell_args_placeholder(self) -> &'static str {
        match self {
            Self::English => "e.g. -l",
            Self::SimplifiedChinese => "例如 -l",
        }
    }

    pub(crate) fn cursor_shape_block(self) -> &'static str {
        match self {
            Self::English => "Block",
            Self::SimplifiedChinese => "方块",
        }
    }

    pub(crate) fn cursor_shape_bar(self) -> &'static str {
        match self {
            Self::English => "Bar",
            Self::SimplifiedChinese => "竖线",
        }
    }

    pub(crate) fn cursor_shape_underline(self) -> &'static str {
        match self {
            Self::English => "Underline",
            Self::SimplifiedChinese => "下划线",
        }
    }

    pub(crate) fn cursor_shape_block_hollow(self) -> &'static str {
        match self {
            Self::English => "Hollow",
            Self::SimplifiedChinese => "空心",
        }
    }

    pub(crate) fn cursor_blink_row(self) -> &'static str {
        match self {
            Self::English => "Cursor blink",
            Self::SimplifiedChinese => "光标闪烁",
        }
    }

    pub(crate) fn cursor_blink_program(self) -> &'static str {
        match self {
            Self::English => "Follow",
            Self::SimplifiedChinese => "跟随程序",
        }
    }

    pub(crate) fn cursor_blink_on(self) -> &'static str {
        match self {
            Self::English => "On",
            Self::SimplifiedChinese => "开",
        }
    }

    pub(crate) fn cursor_blink_off(self) -> &'static str {
        match self {
            Self::English => "Off",
            Self::SimplifiedChinese => "关",
        }
    }

    pub(crate) fn search_dark_themes(self) -> &'static str {
        match self {
            Self::English => "Search dark themes",
            Self::SimplifiedChinese => "搜索深色主题",
        }
    }

    pub(crate) fn search_light_themes(self) -> &'static str {
        match self {
            Self::English => "Search light themes",
            Self::SimplifiedChinese => "搜索浅色主题",
        }
    }

    pub(crate) fn search_fonts(self) -> &'static str {
        match self {
            Self::English => "Search fonts",
            Self::SimplifiedChinese => "搜索字体",
        }
    }

    pub(crate) fn search_placeholder(self) -> &'static str {
        match self {
            Self::English => "Find",
            Self::SimplifiedChinese => "查找",
        }
    }

    pub(crate) fn no_matches(self) -> &'static str {
        match self {
            Self::English => "No matches",
            Self::SimplifiedChinese => "无匹配项",
        }
    }

    pub(crate) fn theme_mode_dark(self) -> &'static str {
        match self {
            Self::English => "Dark",
            Self::SimplifiedChinese => "深色",
        }
    }

    pub(crate) fn theme_mode_light(self) -> &'static str {
        match self {
            Self::English => "Light",
            Self::SimplifiedChinese => "浅色",
        }
    }

    pub(crate) fn theme_mode_system(self) -> &'static str {
        match self {
            Self::English => "System",
            Self::SimplifiedChinese => "跟随系统",
        }
    }

    // --- Keyboard shortcuts ------------------------------------------------------------------

    pub(crate) fn keybindings_sidebar(self) -> &'static str {
        match self {
            Self::English => "Keyboard",
            Self::SimplifiedChinese => "键盘",
        }
    }

    pub(crate) fn keybindings_section(self) -> &'static str {
        match self {
            Self::English => "Keyboard Shortcuts",
            Self::SimplifiedChinese => "键盘快捷键",
        }
    }

    pub(crate) fn recording_prompt(self) -> &'static str {
        match self {
            Self::English => "Press shortcut…",
            Self::SimplifiedChinese => "按下快捷键…",
        }
    }

    pub(crate) fn keybind_conflict(self, other: &str) -> String {
        match self {
            Self::English => format!("Conflicts with {other}"),
            Self::SimplifiedChinese => format!("与「{other}」冲突"),
        }
    }

    pub(crate) fn reset_to_default(self) -> &'static str {
        match self {
            Self::English => "Reset",
            Self::SimplifiedChinese => "恢复默认",
        }
    }

    pub(crate) fn reset_all(self) -> &'static str {
        match self {
            Self::English => "Reset All",
            Self::SimplifiedChinese => "全部恢复默认",
        }
    }

    pub(crate) fn keybindings_hint(self) -> &'static str {
        match self {
            Self::English => "Click a shortcut to change it",
            Self::SimplifiedChinese => "点击快捷键进行修改",
        }
    }

    // --- Action labels -----------------------------------------------------------------------

    pub(crate) fn action_open_settings(self) -> &'static str {
        match self {
            Self::English => "Open Settings",
            Self::SimplifiedChinese => "打开设置",
        }
    }

    pub(crate) fn action_quit(self) -> &'static str {
        match self {
            Self::English => "Quit Eggie",
            Self::SimplifiedChinese => "退出 Eggie",
        }
    }

    pub(crate) fn action_terminal_copy(self) -> &'static str {
        match self {
            Self::English => "Copy",
            Self::SimplifiedChinese => "复制",
        }
    }

    pub(crate) fn action_terminal_paste(self) -> &'static str {
        match self {
            Self::English => "Paste",
            Self::SimplifiedChinese => "粘贴",
        }
    }

    pub(crate) fn action_terminal_select_all(self) -> &'static str {
        match self {
            Self::English => "Select All",
            Self::SimplifiedChinese => "全选",
        }
    }

    pub(crate) fn action_terminal_find(self) -> &'static str {
        match self {
            Self::English => "Find",
            Self::SimplifiedChinese => "查找",
        }
    }

    pub(crate) fn action_new_tab(self) -> &'static str {
        match self {
            Self::English => "New Tab",
            Self::SimplifiedChinese => "新建标签页",
        }
    }

    pub(crate) fn action_new_window(self) -> &'static str {
        match self {
            Self::English => "New Window",
            Self::SimplifiedChinese => "新建窗口",
        }
    }

    pub(crate) fn action_close_tab(self) -> &'static str {
        match self {
            Self::English => "Close Tab",
            Self::SimplifiedChinese => "关闭标签页",
        }
    }

    pub(crate) fn action_next_tab(self) -> &'static str {
        match self {
            Self::English => "Next Tab",
            Self::SimplifiedChinese => "下一个标签页",
        }
    }

    pub(crate) fn action_prev_tab(self) -> &'static str {
        match self {
            Self::English => "Previous Tab",
            Self::SimplifiedChinese => "上一个标签页",
        }
    }

    pub(crate) fn action_split_right(self) -> &'static str {
        match self {
            Self::English => "Split Right",
            Self::SimplifiedChinese => "向右分屏",
        }
    }

    pub(crate) fn action_split_down(self) -> &'static str {
        match self {
            Self::English => "Split Down",
            Self::SimplifiedChinese => "向下分屏",
        }
    }

    pub(crate) fn action_clear_screen(self) -> &'static str {
        match self {
            Self::English => "Clear Screen",
            Self::SimplifiedChinese => "清屏",
        }
    }

    pub(crate) fn action_scroll_top(self) -> &'static str {
        match self {
            Self::English => "Scroll to Top",
            Self::SimplifiedChinese => "滚动到顶部",
        }
    }

    pub(crate) fn action_scroll_bottom(self) -> &'static str {
        match self {
            Self::English => "Scroll to Bottom",
            Self::SimplifiedChinese => "滚动到底部",
        }
    }

    pub(crate) fn action_page_up(self) -> &'static str {
        match self {
            Self::English => "Page Up",
            Self::SimplifiedChinese => "向上翻页",
        }
    }

    pub(crate) fn action_page_down(self) -> &'static str {
        match self {
            Self::English => "Page Down",
            Self::SimplifiedChinese => "向下翻页",
        }
    }

    pub(crate) fn action_font_increase(self) -> &'static str {
        match self {
            Self::English => "Increase Font Size",
            Self::SimplifiedChinese => "增大字号",
        }
    }

    pub(crate) fn action_font_decrease(self) -> &'static str {
        match self {
            Self::English => "Decrease Font Size",
            Self::SimplifiedChinese => "减小字号",
        }
    }

    pub(crate) fn action_font_reset(self) -> &'static str {
        match self {
            Self::English => "Reset Font Size",
            Self::SimplifiedChinese => "重置字号",
        }
    }

    pub(crate) fn action_split_left(self) -> &'static str {
        match self {
            Self::English => "Split Left",
            Self::SimplifiedChinese => "向左分屏",
        }
    }

    pub(crate) fn action_split_up(self) -> &'static str {
        match self {
            Self::English => "Split Up",
            Self::SimplifiedChinese => "向上分屏",
        }
    }

    pub(crate) fn action_find_next(self) -> &'static str {
        match self {
            Self::English => "Find Next",
            Self::SimplifiedChinese => "查找下一个",
        }
    }

    pub(crate) fn action_find_previous(self) -> &'static str {
        match self {
            Self::English => "Find Previous",
            Self::SimplifiedChinese => "查找上一个",
        }
    }

    pub(crate) fn action_close_window(self) -> &'static str {
        match self {
            Self::English => "Close Window",
            Self::SimplifiedChinese => "关闭窗口",
        }
    }

    pub(crate) fn action_minimize_window(self) -> &'static str {
        match self {
            Self::English => "Minimize",
            Self::SimplifiedChinese => "最小化",
        }
    }

    pub(crate) fn action_toggle_full_screen(self) -> &'static str {
        match self {
            Self::English => "Toggle Full Screen",
            Self::SimplifiedChinese => "切换全屏",
        }
    }

    pub(crate) fn action_command_palette(self) -> &'static str {
        match self {
            Self::English => "Command Palette",
            Self::SimplifiedChinese => "命令面板",
        }
    }

    pub(crate) fn action_toggle_left_sidebar(self) -> &'static str {
        match self {
            Self::English => "Toggle Left Sidebar",
            Self::SimplifiedChinese => "切换左侧边栏",
        }
    }

    pub(crate) fn action_toggle_right_sidebar(self) -> &'static str {
        match self {
            Self::English => "Toggle Right Sidebar",
            Self::SimplifiedChinese => "切换右侧边栏",
        }
    }

    // --- Command palette --------------------------------------------------------------------

    pub(crate) fn command_palette_menu_item(self) -> &'static str {
        match self {
            Self::English => "Command Palette…",
            Self::SimplifiedChinese => "命令面板…",
        }
    }

    pub(crate) fn command_palette_placeholder(self) -> &'static str {
        match self {
            Self::English => "Search commands…",
            Self::SimplifiedChinese => "搜索命令…",
        }
    }

    pub(crate) fn command_palette_empty(self) -> &'static str {
        match self {
            Self::English => "No matching commands",
            Self::SimplifiedChinese => "无匹配的命令",
        }
    }

    // --- Profiles ---------------------------------------------------------------------------

    /// Palette command that opens the profile picker.
    pub(crate) fn action_new_tab_with_profile(self) -> &'static str {
        match self {
            Self::English => "New Terminal with Profile…",
            Self::SimplifiedChinese => "使用配置新建终端…",
        }
    }

    pub(crate) fn profile_picker_placeholder(self) -> &'static str {
        match self {
            Self::English => "Choose a profile…",
            Self::SimplifiedChinese => "选择配置…",
        }
    }

    pub(crate) fn profile_picker_empty(self) -> &'static str {
        match self {
            Self::English => "No matching profiles",
            Self::SimplifiedChinese => "无匹配的配置",
        }
    }

    /// Suffix marking the project's default profile in the picker.
    pub(crate) fn profile_picker_project_default(self) -> &'static str {
        match self {
            Self::English => "project default",
            Self::SimplifiedChinese => "项目默认",
        }
    }

    // --- Profiles settings pane ---------------------------------------------------------------

    pub(crate) fn profiles_sidebar(self) -> &'static str {
        match self {
            Self::English => "Profiles",
            Self::SimplifiedChinese => "配置",
        }
    }

    pub(crate) fn profiles_section(self) -> &'static str {
        match self {
            Self::English => "Profiles",
            Self::SimplifiedChinese => "配置",
        }
    }

    /// Name given to a newly created profile (the user renames it right after).
    pub(crate) fn new_profile_name(self) -> &'static str {
        match self {
            Self::English => "New Profile",
            Self::SimplifiedChinese => "新配置",
        }
    }

    pub(crate) fn add_profile_tooltip(self) -> &'static str {
        match self {
            Self::English => "New profile",
            Self::SimplifiedChinese => "新建配置",
        }
    }

    pub(crate) fn delete_profile_tooltip(self) -> &'static str {
        match self {
            Self::English => "Delete profile",
            Self::SimplifiedChinese => "删除配置",
        }
    }

    /// Marks the built-in Default profile in the list, which cannot be deleted.
    pub(crate) fn builtin_profile_marker(self) -> &'static str {
        match self {
            Self::English => "built-in",
            Self::SimplifiedChinese => "内置",
        }
    }

    pub(crate) fn delete_profile_title(self) -> &'static str {
        match self {
            Self::English => "Delete this profile?",
            Self::SimplifiedChinese => "删除此配置？",
        }
    }

    /// Body of the delete confirmation. Names what survives deletion, because running terminals
    /// launched from this profile keep running — only their appearance overrides fall away.
    pub(crate) fn delete_profile_message(self, name: &str) -> String {
        match self {
            Self::English => format!(
                "“{name}” will be deleted. Existing terminals keep running with the global appearance. Projects using it as their default switch to Default."
            ),
            Self::SimplifiedChinese => format!(
                "删除「{name}」后，现有终端继续运行并使用全局外观，以此为默认配置的项目改用 Default。"
            ),
        }
    }

    pub(crate) fn profile_name_row(self) -> &'static str {
        match self {
            Self::English => "Name",
            Self::SimplifiedChinese => "名称",
        }
    }

    pub(crate) fn profile_launch_section(self) -> &'static str {
        match self {
            Self::English => "Launch",
            Self::SimplifiedChinese => "启动",
        }
    }

    pub(crate) fn profile_cwd_row(self) -> &'static str {
        match self {
            Self::English => "Working directory",
            Self::SimplifiedChinese => "工作目录",
        }
    }

    pub(crate) fn profile_cwd_description(self) -> &'static str {
        match self {
            Self::English => {
                "Leave empty to use the current directory, then the project root; Remote profiles use a remote path"
            }
            Self::SimplifiedChinese => {
                "留空使用当前终端目录，其次是项目根目录；远程配置请填写远端路径"
            }
        }
    }

    pub(crate) fn profile_cwd_placeholder(self) -> &'static str {
        match self {
            Self::English => "e.g. /Users/me/work",
            Self::SimplifiedChinese => "例如 /Users/me/work",
        }
    }

    pub(crate) fn profile_env_row(self) -> &'static str {
        match self {
            Self::English => "Environment variables",
            Self::SimplifiedChinese => "环境变量",
        }
    }

    pub(crate) fn profile_env_description(self) -> &'static str {
        match self {
            Self::English => {
                "One NAME=value per line; Names: [A-Za-z_][A-Za-z0-9_]*; Eggie integration variables take priority"
            }
            Self::SimplifiedChinese => {
                "每行 NAME=value；名称格式为 [A-Za-z_][A-Za-z0-9_]*；Eggie 集成变量优先"
            }
        }
    }

    pub(crate) fn profile_env_placeholder(self) -> &'static str {
        match self {
            Self::English => "AWS_PROFILE=prod",
            Self::SimplifiedChinese => "AWS_PROFILE=prod",
        }
    }

    /// Warning shown under the env editor when some lines were dropped. Names the count so a typo
    /// is visible rather than silently discarded.
    pub(crate) fn profile_env_rejected(self, count: usize) -> String {
        match self {
            Self::English => {
                format!("{count} line(s) ignored. Check NAME=value syntax and variable names.")
            }
            Self::SimplifiedChinese => {
                format!("已忽略 {count} 行，请检查 NAME=value 格式和变量名。")
            }
        }
    }

    pub(crate) fn profile_remote_section(self) -> &'static str {
        match self {
            Self::English => "Remote",
            Self::SimplifiedChinese => "远程",
        }
    }

    pub(crate) fn profile_remote_host_row(self) -> &'static str {
        match self {
            Self::English => "Host",
            Self::SimplifiedChinese => "主机",
        }
    }

    pub(crate) fn profile_remote_host_description(self) -> &'static str {
        match self {
            Self::English => "SSH host; Leave empty for a local terminal",
            Self::SimplifiedChinese => "SSH 主机，留空使用本地终端",
        }
    }

    pub(crate) fn profile_remote_host_placeholder(self) -> &'static str {
        match self {
            Self::English => "e.g. prod.example.com",
            Self::SimplifiedChinese => "例如 prod.example.com",
        }
    }

    pub(crate) fn profile_remote_user_row(self) -> &'static str {
        match self {
            Self::English => "User",
            Self::SimplifiedChinese => "用户",
        }
    }

    pub(crate) fn profile_remote_user_description(self) -> &'static str {
        match self {
            Self::English => "Leave empty to use SSH defaults",
            Self::SimplifiedChinese => "留空使用 SSH 默认用户",
        }
    }

    pub(crate) fn profile_remote_port_row(self) -> &'static str {
        match self {
            Self::English => "Port",
            Self::SimplifiedChinese => "端口",
        }
    }

    pub(crate) fn profile_remote_port_description(self) -> &'static str {
        match self {
            Self::English => "Leave empty to use SSH defaults (usually 22)",
            Self::SimplifiedChinese => "留空使用 SSH 默认端口（通常为 22）",
        }
    }

    /// Shown when host/user would be parsed by ssh as an option rather than a destination. This is a
    /// refusal, not a cosmetic hint: such a target is dropped at launch.
    pub(crate) fn profile_remote_invalid(self) -> &'static str {
        match self {
            Self::English => {
                "Ignored: a host or user cannot be empty, start with “-”, or contain spaces."
            }
            Self::SimplifiedChinese => "已忽略：主机或用户不能为空、不能以“-”开头，也不能含空格。",
        }
    }

    pub(crate) fn profile_appearance_section(self) -> &'static str {
        match self {
            Self::English => "Appearance",
            Self::SimplifiedChinese => "外观",
        }
    }

    pub(crate) fn profile_terminal_dark_theme_row(self) -> &'static str {
        match self {
            Self::English => "Terminal dark theme",
            Self::SimplifiedChinese => "终端深色主题",
        }
    }

    pub(crate) fn profile_terminal_light_theme_row(self) -> &'static str {
        match self {
            Self::English => "Terminal light theme",
            Self::SimplifiedChinese => "终端浅色主题",
        }
    }

    pub(crate) fn profile_font_row(self) -> &'static str {
        match self {
            Self::English => "Font",
            Self::SimplifiedChinese => "字体",
        }
    }

    pub(crate) fn profile_font_size_row(self) -> &'static str {
        match self {
            Self::English => "Font size",
            Self::SimplifiedChinese => "字号",
        }
    }

    pub(crate) fn profile_badge_row(self) -> &'static str {
        match self {
            Self::English => "Tab marker",
            Self::SimplifiedChinese => "标签标记",
        }
    }

    /// The "inherit the global setting" choice in a per-profile appearance selector.
    pub(crate) fn profile_appearance_inherit(self) -> &'static str {
        match self {
            Self::English => "Follow global setting",
            Self::SimplifiedChinese => "跟随全局设置",
        }
    }

    /// The "no badge" choice in the badge color picker.
    pub(crate) fn profile_badge_none(self) -> &'static str {
        match self {
            Self::English => "None",
            Self::SimplifiedChinese => "无",
        }
    }

    // --- Project default profile ---------------------------------------------------------------

    /// Project context-menu item opening the default-profile picker.
    pub(crate) fn set_default_profile(self) -> &'static str {
        match self {
            Self::English => "Default Profile…",
            Self::SimplifiedChinese => "默认配置…",
        }
    }

    pub(crate) fn project_default_profile_placeholder(self) -> &'static str {
        match self {
            Self::English => "Project default profile…",
            Self::SimplifiedChinese => "项目默认配置…",
        }
    }

    // --- Remote shell integration -------------------------------------------------------------

    pub(crate) fn remote_install_title(self) -> &'static str {
        match self {
            Self::English => "Install Eggie shell integration?",
            Self::SimplifiedChinese => "安装 Eggie shell 集成？",
        }
    }

    /// Body of the consent prompt. Names the destination and what will be written, because this is
    /// Eggie writing files into someone else's machine.
    pub(crate) fn remote_install_message(self, destination: &str) -> String {
        match self {
            Self::English => format!(
                "Install files in ~/.eggie/ on {destination} for directory tracking, command history, and prompt navigation.\n\nYou can connect without installing. This choice will be remembered."
            ),
            Self::SimplifiedChinese => format!(
                "将在 {destination} 的 ~/.eggie/ 安装集成文件，启用目录跟踪、命令历史和命令跳转。\n\n也可不安装直接连接，此选择将被记住。"
            ),
        }
    }

    pub(crate) fn remote_install_allow(self) -> &'static str {
        match self {
            Self::English => "Install",
            Self::SimplifiedChinese => "安装",
        }
    }

    pub(crate) fn remote_install_skip(self) -> &'static str {
        match self {
            Self::English => "Skip and Connect",
            Self::SimplifiedChinese => "跳过并连接",
        }
    }

    // --- Bell -------------------------------------------------------------------------------

    pub(crate) fn bell_row(self) -> &'static str {
        match self {
            Self::English => "Bell",
            Self::SimplifiedChinese => "响铃",
        }
    }

    pub(crate) fn bell_mode_silent(self) -> &'static str {
        match self {
            Self::English => "Silent",
            Self::SimplifiedChinese => "静默",
        }
    }

    pub(crate) fn bell_mode_flash(self) -> &'static str {
        match self {
            Self::English => "Flash",
            Self::SimplifiedChinese => "闪动",
        }
    }

    pub(crate) fn bell_mode_sound(self) -> &'static str {
        match self {
            Self::English => "Sound",
            Self::SimplifiedChinese => "声音",
        }
    }

    pub(crate) fn bell_mode_flash_and_sound(self) -> &'static str {
        match self {
            Self::English => "Both",
            Self::SimplifiedChinese => "闪动+声音",
        }
    }

    pub(crate) fn action_jump_prev_prompt(self) -> &'static str {
        match self {
            Self::English => "Jump to Previous Command",
            Self::SimplifiedChinese => "跳到上一条命令",
        }
    }

    pub(crate) fn action_jump_next_prompt(self) -> &'static str {
        match self {
            Self::English => "Jump to Next Command",
            Self::SimplifiedChinese => "跳到下一条命令",
        }
    }
}
