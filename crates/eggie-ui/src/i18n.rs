use crate::settings::Language;

impl Language {
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
            Self::English => "A process in this terminal wants to open:",
            Self::SimplifiedChinese => "此终端中的某个进程想要打开：",
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
            Self::English => "A link in this terminal points to:",
            Self::SimplifiedChinese => "此终端中的链接指向：",
        }
    }

    pub(crate) fn choose_receive_files_prompt(self) -> &'static str {
        match self {
            Self::English => "Choose where to receive terminal files",
            Self::SimplifiedChinese => "选择接收终端文件的位置",
        }
    }

    pub(crate) fn close_window_title(self) -> &'static str {
        match self {
            Self::English => "Close this Eggie window?",
            Self::SimplifiedChinese => "关闭此 Eggie 窗口？",
        }
    }

    pub(crate) fn close_window_message(self) -> &'static str {
        match self {
            Self::English => {
                "Choose whether its terminal sessions should terminate or stay attached to the daemon."
            }
            Self::SimplifiedChinese => "选择其终端会话应终止还是保持连接到守护进程。",
        }
    }

    pub(crate) fn terminate_all(self) -> &'static str {
        match self {
            Self::English => "Terminate All",
            Self::SimplifiedChinese => "全部终止",
        }
    }

    pub(crate) fn detach(self) -> &'static str {
        match self {
            Self::English => "Detach",
            Self::SimplifiedChinese => "分离",
        }
    }

    pub(crate) fn new_project_title(self) -> &'static str {
        match self {
            Self::English => "New Project",
            Self::SimplifiedChinese => "新建项目",
        }
    }

    pub(crate) fn new_project_message(self) -> &'static str {
        match self {
            Self::English => "Enter a name for the project.",
            Self::SimplifiedChinese => "输入项目名称。",
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

    pub(crate) fn rename_project_message(self) -> &'static str {
        match self {
            Self::English => "Enter a new name for the project.",
            Self::SimplifiedChinese => "输入项目的新名称。",
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
    pub(crate) fn detached_other_group(self) -> &'static str {
        match self {
            Self::English => "Other",
            Self::SimplifiedChinese => "其它",
        }
    }

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

    pub(crate) fn detached_sessions_tooltip(self) -> &'static str {
        match self {
            Self::English => "Detached sessions",
            Self::SimplifiedChinese => "游离会话",
        }
    }

    pub(crate) fn add_project_tooltip(self) -> &'static str {
        match self {
            Self::English => "Add project",
            Self::SimplifiedChinese => "添加项目",
        }
    }

    pub(crate) fn claim_session_tooltip(self) -> &'static str {
        match self {
            Self::English => "Claim to this window",
            Self::SimplifiedChinese => "接管到此窗口",
        }
    }

    pub(crate) fn destroy_session_tooltip(self) -> &'static str {
        match self {
            Self::English => "Destroy session",
            Self::SimplifiedChinese => "销毁会话",
        }
    }

    pub(crate) fn ports_label(self) -> &'static str {
        match self {
            Self::English => "PORTS",
            Self::SimplifiedChinese => "端口",
        }
    }

    pub(crate) fn current_directory_label(self) -> &'static str {
        match self {
            Self::English => "CURRENT DIRECTORY",
            Self::SimplifiedChinese => "当前目录",
        }
    }

    pub(crate) fn source_control_label(self) -> &'static str {
        match self {
            Self::English => "SOURCE CONTROL",
            Self::SimplifiedChinese => "源代码管理",
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

    pub(crate) fn files_tab(self) -> &'static str {
        match self {
            Self::English => "Files",
            Self::SimplifiedChinese => "文件",
        }
    }

    pub(crate) fn git_tab(self) -> &'static str {
        match self {
            Self::English => "Git",
            Self::SimplifiedChinese => "Git",
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

    pub(crate) fn loading_ports(self) -> &'static str {
        match self {
            Self::English => "Loading port information…",
            Self::SimplifiedChinese => "正在加载端口信息…",
        }
    }

    pub(crate) fn loading_process_info(self) -> &'static str {
        match self {
            Self::English => "Loading process information…",
            Self::SimplifiedChinese => "正在加载进程信息…",
        }
    }

    pub(crate) fn file_tree_scaffold_note(self) -> &'static str {
        match self {
            Self::English => {
                "File-tree service is scaffolded for current, initial, and locked roots."
            }
            Self::SimplifiedChinese => "文件树服务已为当前、初始和锁定根目录搭建。",
        }
    }

    pub(crate) fn git_scaffold_note(self) -> &'static str {
        match self {
            Self::English => {
                "Git status, diff, staging, commit, branch, pull, and push services are scaffolded."
            }
            Self::SimplifiedChinese => "Git 状态、差异、暂存、提交、分支、拉取和推送服务已搭建。",
        }
    }

    pub(crate) fn no_running_processes(self) -> &'static str {
        match self {
            Self::English => "No running processes.",
            Self::SimplifiedChinese => "没有正在运行的进程。",
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
            Self::English => "Indeterminate",
            Self::SimplifiedChinese => "不确定",
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
            Self::English => format!("Close {count} tab{}", if count == 1 { "" } else { "s" }),
            Self::SimplifiedChinese => format!("关闭 {} 个标签页", count),
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

    pub(crate) fn language_description(self) -> &'static str {
        match self {
            Self::English => "Choose the interface language.",
            Self::SimplifiedChinese => "选择界面语言。",
        }
    }

    pub(crate) fn theme_row(self) -> &'static str {
        match self {
            Self::English => "Theme",
            Self::SimplifiedChinese => "主题",
        }
    }

    pub(crate) fn theme_description(self) -> &'static str {
        match self {
            Self::English => "Choose dark, light, or follow the macOS appearance.",
            Self::SimplifiedChinese => "选择深色、浅色，或跟随 macOS 外观。",
        }
    }

    pub(crate) fn dark_theme_row(self) -> &'static str {
        match self {
            Self::English => "Dark theme",
            Self::SimplifiedChinese => "深色主题",
        }
    }

    pub(crate) fn dark_theme_description(self) -> &'static str {
        match self {
            Self::English => "Used in Dark mode and when the system appearance is dark.",
            Self::SimplifiedChinese => "在深色模式和系统外观为深色时使用。",
        }
    }

    pub(crate) fn light_theme_row(self) -> &'static str {
        match self {
            Self::English => "Light theme",
            Self::SimplifiedChinese => "浅色主题",
        }
    }

    pub(crate) fn light_theme_description(self) -> &'static str {
        match self {
            Self::English => "Used in Light mode and when the system appearance is light.",
            Self::SimplifiedChinese => "在浅色模式和系统外观为浅色时使用。",
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
            Self::English => {
                "Minimum WCAG contrast ratio between text and each cell background. Emoji and terminal graphics are unchanged."
            }
            Self::SimplifiedChinese => {
                "文本与每个单元格背景之间的最小 WCAG 对比度。Emoji 和终端图形不受影响。"
            }
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
            Self::English => "Only installed monospaced fonts are listed.",
            Self::SimplifiedChinese => "仅列出已安装的等宽字体。",
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

    pub(crate) fn font_bold_description(self) -> &'static str {
        match self {
            Self::English => "Font for bold text. Falls back to the regular font when unset.",
            Self::SimplifiedChinese => "粗体文本使用的字体。未设置时跟随常规字体。",
        }
    }

    pub(crate) fn font_italic_row(self) -> &'static str {
        match self {
            Self::English => "Italic font",
            Self::SimplifiedChinese => "斜体字体",
        }
    }

    pub(crate) fn font_italic_description(self) -> &'static str {
        match self {
            Self::English => "Font for italic text. Falls back to the regular font when unset.",
            Self::SimplifiedChinese => "斜体文本使用的字体。未设置时跟随常规字体。",
        }
    }

    pub(crate) fn font_bold_italic_row(self) -> &'static str {
        match self {
            Self::English => "Bold italic font",
            Self::SimplifiedChinese => "粗斜体字体",
        }
    }

    pub(crate) fn font_bold_italic_description(self) -> &'static str {
        match self {
            Self::English => {
                "Font for bold italic text. Falls back to the regular font when unset."
            }
            Self::SimplifiedChinese => "粗斜体文本使用的字体。未设置时跟随常规字体。",
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
            Self::English => {
                "Fake bold by thickening strokes when the font lacks a bold face and no bold family is set."
            }
            Self::SimplifiedChinese => "当字体没有粗体且未指定粗体字体时，通过加粗笔画模拟粗体。",
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
            Self::English => {
                "Fake italic by skewing glyphs when the font lacks an italic face and no italic family is set."
            }
            Self::SimplifiedChinese => "当字体没有斜体且未指定斜体字体时，通过倾斜字形模拟斜体。",
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
            Self::English => "Synthesize the bold italic style when the font and families lack it.",
            Self::SimplifiedChinese => "当字体与所选字体族都缺少粗斜体时，合成粗斜体样式。",
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
            Self::English => {
                "Combine sequences like -> and ==> into single glyphs (requires a font with programming ligatures)."
            }
            Self::SimplifiedChinese => "将 -> 、==> 等序列合并为连字（需字体本身包含编程连字）。",
        }
    }

    pub(crate) fn shaping_break_row(self) -> &'static str {
        match self {
            Self::English => "Break ligature at cursor",
            Self::SimplifiedChinese => "光标处拆分连字",
        }
    }

    pub(crate) fn shaping_break_description(self) -> &'static str {
        match self {
            Self::English => {
                "Render the character under the cursor un-ligated so editing sees individual characters."
            }
            Self::SimplifiedChinese => "光标所在字符不参与连字，便于编辑时看清单个字符。",
        }
    }

    pub(crate) fn font_thicken_row(self) -> &'static str {
        match self {
            Self::English => "Thicken",
            Self::SimplifiedChinese => "字体加粗",
        }
    }

    pub(crate) fn font_thicken_description(self) -> &'static str {
        match self {
            Self::English => "Thicken glyph strokes via macOS font smoothing.",
            Self::SimplifiedChinese => "通过 macOS 字体平滑加粗字形笔画。",
        }
    }

    pub(crate) fn font_thicken_strength_row(self) -> &'static str {
        match self {
            Self::English => "Thicken strength",
            Self::SimplifiedChinese => "加粗强度",
        }
    }

    pub(crate) fn font_thicken_strength_description(self) -> &'static str {
        match self {
            Self::English => "How much to thicken (0–255) when thickening is enabled.",
            Self::SimplifiedChinese => "开启加粗时的强度（0–255）。",
        }
    }

    pub(crate) fn font_size_row(self) -> &'static str {
        match self {
            Self::English => "Font size",
            Self::SimplifiedChinese => "字号",
        }
    }

    pub(crate) fn font_size_description(self) -> &'static str {
        match self {
            Self::English => "Applied to every terminal and used when calculating the PTY grid.",
            Self::SimplifiedChinese => "应用于所有终端，并用于计算 PTY 网格。",
        }
    }

    pub(crate) fn horizontal_padding_row(self) -> &'static str {
        match self {
            Self::English => "Horizontal padding",
            Self::SimplifiedChinese => "水平内边距",
        }
    }

    pub(crate) fn horizontal_padding_description(self) -> &'static str {
        match self {
            Self::English => "Left and right padding around the terminal grid.",
            Self::SimplifiedChinese => "终端网格左右两侧的内边距。",
        }
    }

    pub(crate) fn vertical_padding_row(self) -> &'static str {
        match self {
            Self::English => "Vertical padding",
            Self::SimplifiedChinese => "垂直内边距",
        }
    }

    pub(crate) fn vertical_padding_description(self) -> &'static str {
        match self {
            Self::English => "Top and bottom padding around the terminal grid.",
            Self::SimplifiedChinese => "终端网格上下两侧的内边距。",
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
            Self::English => "Pixel adjustment to each cell's width. Changes the grid columns.",
            Self::SimplifiedChinese => "对每个单元格宽度的像素微调，会改变网格列数。",
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
            Self::English => "Pixel adjustment to each cell's height. Changes the grid rows.",
            Self::SimplifiedChinese => "对每个单元格高度的像素微调，会改变网格行数。",
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
            Self::English => "Pixel adjustment to the text baseline within each cell.",
            Self::SimplifiedChinese => "对单元格内文本基线位置的像素微调。",
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
            Self::English => "Pixel adjustment to how far down the underline sits.",
            Self::SimplifiedChinese => "对下划线下沉距离的像素微调。",
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
            Self::English => "Pixel adjustment to the underline stroke thickness.",
            Self::SimplifiedChinese => "对下划线笔画粗细的像素微调。",
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
            Self::English => "Pixel adjustment to how far down the strikethrough sits.",
            Self::SimplifiedChinese => "对删除线下沉距离的像素微调。",
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
            Self::English => "Pixel adjustment to the strikethrough stroke thickness.",
            Self::SimplifiedChinese => "对删除线笔画粗细的像素微调。",
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
            Self::English => "Pixel adjustment to the bar/underline/hollow cursor stroke.",
            Self::SimplifiedChinese => "对竖线/下划线/空心光标笔画粗细的像素微调。",
        }
    }

    pub(crate) fn adjust_box_thickness_row(self) -> &'static str {
        match self {
            Self::English => "Box-drawing thickness",
            Self::SimplifiedChinese => "制表符线条粗细",
        }
    }

    pub(crate) fn adjust_box_thickness_description(self) -> &'static str {
        match self {
            Self::English => "Pixel adjustment to box-drawing line thickness.",
            Self::SimplifiedChinese => "对制表符（框线）线条粗细的像素微调。",
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
            Self::English => "Percentage or pixel adjustment to Nerd Font icon height.",
            Self::SimplifiedChinese => "对 Nerd Font 图标高度的百分比或像素微调。",
        }
    }

    pub(crate) fn completed_timeout_row(self) -> &'static str {
        match self {
            Self::English => "Completed timeout",
            Self::SimplifiedChinese => "完成超时",
        }
    }

    pub(crate) fn completed_timeout_description(self) -> &'static str {
        match self {
            Self::English => "Hide completed OSC 9;4 progress after this delay.",
            Self::SimplifiedChinese => "在此延迟后隐藏已完成的 OSC 9;4 进度。",
        }
    }

    pub(crate) fn inactive_timeout_row(self) -> &'static str {
        match self {
            Self::English => "Inactive timeout",
            Self::SimplifiedChinese => "不活动超时",
        }
    }

    pub(crate) fn inactive_timeout_description(self) -> &'static str {
        match self {
            Self::English => {
                "Hide running, error, paused, or indeterminate progress when no update arrives for this long."
            }
            Self::SimplifiedChinese => "当长时间没有更新时，隐藏运行中、错误、已暂停或不确定的进度。",
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
            Self::English => {
                "Allow terminal programs to read the system clipboard through OSC 52 or OSC 5522. Writes remain enabled."
            }
            Self::SimplifiedChinese => {
                "允许终端程序通过 OSC 52 或 OSC 5522 读取系统剪贴板。写入保持启用。"
            }
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
            Self::English => {
                "Highlight bare URLs in terminal output so they can be hovered and opened with Cmd/Ctrl+click."
            }
            Self::SimplifiedChinese => {
                "高亮终端输出中的裸露 URL，可悬停并用 Cmd/Ctrl+点击打开。"
            }
        }
    }

    pub(crate) fn copy_on_select_row(self) -> &'static str {
        match self {
            Self::English => "Copy on select",
            Self::SimplifiedChinese => "选中即复制",
        }
    }

    pub(crate) fn copy_on_select_description(self) -> &'static str {
        match self {
            Self::English => {
                "Automatically copy text to the clipboard as soon as it is selected with the mouse."
            }
            Self::SimplifiedChinese => {
                "鼠标选中文本后自动复制到剪贴板。"
            }
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
            Self::English => {
                "The default cursor shape. Programs can still override it at runtime (DECSCUSR)."
            }
            Self::SimplifiedChinese => {
                "默认光标形状。程序仍可在运行时覆盖（DECSCUSR）。"
            }
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
            Self::English => {
                "How many lines of terminal history to keep (0 disables scrollback). Applies to all terminals immediately."
            }
            Self::SimplifiedChinese => {
                "保留的终端历史行数（0 表示禁用回滚）。立即应用到所有终端。"
            }
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
            Self::English => {
                "Path to the shell to launch. Leave empty to use $SHELL. Applies to newly created terminals only."
            }
            Self::SimplifiedChinese => {
                "要启动的 shell 路径。留空则使用 $SHELL。仅对新建终端生效。"
            }
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
                "Space-separated arguments passed to the shell. Leave empty for the default. Applies to newly created terminals only."
            }
            Self::SimplifiedChinese => {
                "传给 shell 的参数，以空格分隔。留空则使用默认值。仅对新建终端生效。"
            }
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
            Self::English => {
                "Append Eggie's binary directory to the shell PATH so `eggie +version` and other CLI commands work in the terminal. Applies to newly created terminals only."
            }
            Self::SimplifiedChinese => {
                "将 Eggie 二进制所在目录追加到 shell PATH，使 `eggie +version` 等命令行命令可在终端内直接调用。仅对新建终端生效。"
            }
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

    pub(crate) fn cursor_blink_description(self) -> &'static str {
        match self {
            Self::English => {
                "Whether the cursor blinks. Follow lets programs decide (DECSCUSR / DEC Mode 12)."
            }
            Self::SimplifiedChinese => {
                "光标是否闪烁。「跟随程序」由程序决定（DECSCUSR / DEC Mode 12）。"
            }
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

    pub(crate) fn terminal_preview_label(self) -> &'static str {
        match self {
            Self::English => "TERMINAL PREVIEW",
            Self::SimplifiedChinese => "终端预览",
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
            Self::English => "Click a shortcut to record a new key combination.",
            Self::SimplifiedChinese => "点击快捷键即可录制新的组合键。",
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

    // --- Bell -------------------------------------------------------------------------------

    pub(crate) fn bell_row(self) -> &'static str {
        match self {
            Self::English => "Bell",
            Self::SimplifiedChinese => "响铃",
        }
    }

    pub(crate) fn bell_description(self) -> &'static str {
        match self {
            Self::English => "How to react when a program rings the terminal bell.",
            Self::SimplifiedChinese => "程序触发终端响铃时如何提示。",
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
