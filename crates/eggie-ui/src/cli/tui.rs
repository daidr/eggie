//! `eggie +list-themes` 的交互式 TUI 预览(基于 ratatui)。
//!
//! 左栏是可滚动的主题名列表(选中 `❯ …❮`、鼠标 hover 高亮、点击选中),右栏像素级
//! 复刻 ghostty `+list-themes` 的预览:主题名标题、16 色调色板网格、一段 `bat fibonacci.ts`
//! 的语法高亮(用主题自身的调色板着色,切主题即跟随)、以及 lorem ipsum 排版样例。
//!
//! 用 ratatui 的双缓冲差分渲染,天然无闪烁(不再每帧全屏清屏)。终端状态由
//! [`TerminalGuard`] 以 RAII + panic hook 兜底恢复,保证不会把用户终端留在
//! raw / alternate-screen / mouse-capture 状态。

use std::io::{self, Stdout};
use std::panic;

use crossterm::{
    cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    Terminal,
};

use crate::settings::{system_uses_dark_appearance, theme_catalog, TerminalTheme};

/// 列表栏固定宽度(列),对齐 ghostty。
const LIST_WIDTH: u16 = 32;
/// 每翻页移动的行数,对齐 ghostty。
const PAGE: usize = 20;

/// 进入 TUI 预览。`names` 应已排序好。返回进程退出码。
///
/// 终端初始化失败(无法进 raw mode 等)时回退为纯文本列表,绝不把用户困在半初始化状态。
pub(super) fn run(names: &[String]) -> i32 {
    if names.is_empty() {
        println!("(没有可显示的主题)");
        return 0;
    }

    match TerminalGuard::enter() {
        Ok(mut guard) => {
            let result = event_loop(guard.terminal(), names);
            drop(guard);
            match result {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("预览出错:{error}");
                    1
                }
            }
        }
        Err(_) => {
            // 无法进入 TUI(例如非真正的终端),退化为纯文本。
            let mut out = String::new();
            for name in names {
                out.push_str(name);
                out.push('\n');
            }
            print!("{out}");
            0
        }
    }
}

/// 终端状态的 RAII 守卫:构造进入 TUI 模式并安装 panic hook,析构还原。
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            cursor::Hide
        ) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        // panic 兜底:先还原终端再执行原 hook,否则回溯信息会打在备用屏幕上且终端卡在 raw。
        let original = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            restore_terminal();
            original(info);
        }));
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend)?;
        Ok(TerminalGuard { terminal })
    }

    fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
        // 还原 panic hook(避免影响后续,虽然本进程通常随即退出)。
        let _ = panic::take_hook();
    }
}

/// 把终端从 TUI 模式还原到正常状态。幂等、忽略错误(析构/ panic 中无法再传播)。
fn restore_terminal() {
    let mut stdout = io::stdout();
    let _ = execute!(
        stdout,
        cursor::Show,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

/// dark/light 过滤范围,`f` 键循环。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    All,
    Dark,
    Light,
}

impl Filter {
    fn next(self) -> Filter {
        match self {
            Filter::All => Filter::Dark,
            Filter::Dark => Filter::Light,
            Filter::Light => Filter::All,
        }
    }
}

/// 一次键盘/鼠标操作解析出的导航动作。抽出便于单测。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Quit,
    Up(usize),
    Down(usize),
    Home,
    End,
    Hex(bool),
    CycleFilter,
    None,
}

/// UI chrome(列表栏)的配色,跟随系统明暗,复刻 ghostty 的 `ui_*`。预览区不用这些,用主题自身色。
#[derive(Clone, Copy)]
struct UiPalette {
    fg: Color,
    bg: Color,
    hover_bg: Color,
    sel_fg: Color,
    sel_bg: Color,
}

impl UiPalette {
    fn new(dark: bool) -> Self {
        if dark {
            UiPalette {
                fg: Color::Rgb(0xff, 0xff, 0xff),
                bg: Color::Rgb(0x00, 0x00, 0x00),
                hover_bg: Color::Rgb(0x22, 0x22, 0x22),
                sel_fg: Color::Rgb(0x00, 0xaa, 0x00),
                sel_bg: Color::Rgb(0x33, 0x33, 0x33),
            }
        } else {
            UiPalette {
                fg: Color::Rgb(0x00, 0x00, 0x00),
                bg: Color::Rgb(0xff, 0xff, 0xff),
                hover_bg: Color::Rgb(0xbb, 0xbb, 0xbb),
                sel_fg: Color::Rgb(0x00, 0xaa, 0x00),
                sel_bg: Color::Rgb(0xaa, 0xaa, 0xaa),
            }
        }
    }
}

/// TUI 应用状态。
struct App<'a> {
    names: &'a [String],
    /// 过滤后指向 `names` 的下标集合。
    filtered: Vec<usize>,
    /// 当前选中项在 `filtered` 中的下标。
    current: usize,
    /// 列表可视窗口顶部(在 `filtered` 中的下标)。
    window: usize,
    /// 鼠标悬停的 `filtered` 下标(仅命中帧有效)。
    hover: Option<usize>,
    /// 调色板编号是否用十六进制。
    hex: bool,
    filter: Filter,
    ui: UiPalette,
    /// 上一帧列表栏的区域,供鼠标命中测试。
    list_area: Rect,
    should_quit: bool,
}

impl<'a> App<'a> {
    fn new(names: &'a [String]) -> Self {
        let mut app = App {
            names,
            filtered: Vec::new(),
            current: 0,
            window: 0,
            hover: None,
            hex: false,
            filter: Filter::All,
            ui: UiPalette::new(system_uses_dark_appearance()),
            list_area: Rect::new(0, 0, 0, 0),
            should_quit: false,
        };
        app.recompute_filtered();
        app
    }

    /// 按当前 filter 重算 `filtered`,并把 `current` 夹到合法范围。
    fn recompute_filtered(&mut self) {
        let catalog = theme_catalog();
        self.filtered = self
            .names
            .iter()
            .enumerate()
            .filter(|(_, name)| match self.filter {
                Filter::All => true,
                Filter::Dark => catalog.theme_by_name(name).is_some_and(TerminalTheme::is_dark),
                Filter::Light => catalog
                    .theme_by_name(name)
                    .is_some_and(|theme| !theme.is_dark()),
            })
            .map(|(index, _)| index)
            .collect();
        if self.filtered.is_empty() {
            self.current = 0;
        } else if self.current >= self.filtered.len() {
            self.current = self.filtered.len() - 1;
        }
    }

    /// 当前选中主题(若有)。
    fn selected_theme(&self) -> Option<&'static TerminalTheme> {
        let name = self.names.get(*self.filtered.get(self.current)?)?;
        theme_catalog().theme_by_name(name)
    }

    fn move_up(&mut self, count: usize) {
        self.current = self.current.saturating_sub(count);
    }

    fn move_down(&mut self, count: usize) {
        if !self.filtered.is_empty() {
            self.current = (self.current + count).min(self.filtered.len() - 1);
        }
    }

    /// 应用一个导航动作。
    fn apply(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::Up(n) => self.move_up(n),
            Action::Down(n) => self.move_down(n),
            Action::Home => self.current = 0,
            Action::End => {
                if !self.filtered.is_empty() {
                    self.current = self.filtered.len() - 1;
                }
            }
            Action::Hex(value) => self.hex = value,
            Action::CycleFilter => {
                self.filter = self.filter.next();
                self.recompute_filtered();
            }
            Action::None => {}
        }
    }
}

/// 主事件循环:渲染 + 读事件,直到退出。
fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, names: &[String]) -> io::Result<()> {
    let mut app = App::new(names);
    while !app.should_quit {
        terminal.draw(|frame| {
            let area = frame.area();
            draw(frame.buffer_mut(), area, &mut app)
        })?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                // 键盘操作意味着焦点不在鼠标上,清掉 hover 高亮,避免残留。
                app.hover = None;
                let action = key_to_action(key);
                app.apply(action);
            }
            Event::Mouse(mouse) => handle_mouse(&mut app, mouse),
            Event::Resize(_, _) => app.hover = None,
            _ => {}
        }
    }
    Ok(())
}

/// 键 → 导航动作(纯函数,便于单测)。
fn key_to_action(key: KeyEvent) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Action::Quit;
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => Action::Quit,
        KeyCode::Up | KeyCode::Char('k') => Action::Up(1),
        KeyCode::Down | KeyCode::Char('j') => Action::Down(1),
        KeyCode::PageUp => Action::Up(PAGE),
        KeyCode::PageDown => Action::Down(PAGE),
        KeyCode::Home | KeyCode::Char('g') => Action::Home,
        KeyCode::End | KeyCode::Char('G') => Action::End,
        KeyCode::Char('h') | KeyCode::Char('x') => Action::Hex(true),
        KeyCode::Char('d') => Action::Hex(false),
        KeyCode::Char('f') => Action::CycleFilter,
        _ => Action::None,
    }
}

/// 处理鼠标事件:滚轮全局移动,列表区内 hover 高亮 / 左键释放选中。
fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            app.hover = None;
            app.move_up(1);
        }
        MouseEventKind::ScrollDown => {
            app.hover = None;
            app.move_down(1);
        }
        MouseEventKind::Moved | MouseEventKind::Drag(_) => {
            app.hover = hit_test(app.list_area, app.window, app.filtered.len(), mouse.column, mouse.row);
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if let Some(index) =
                hit_test(app.list_area, app.window, app.filtered.len(), mouse.column, mouse.row)
            {
                app.current = index;
            }
        }
        _ => {}
    }
}

/// 鼠标坐标 → `filtered` 下标(命中列表区且落在有效行时)。纯函数,便于单测。
fn hit_test(list_area: Rect, window: usize, filtered_len: usize, col: u16, row: u16) -> Option<usize> {
    if !list_area.contains(Position::new(col, row)) {
        return None;
    }
    let offset = (row - list_area.y) as usize;
    let index = window + offset;
    (index < filtered_len).then_some(index)
}

/// 复刻 ghostty 的滚动窗口:保证 `current` 落在 `[window, window+height)` 内,返回新的 window。
fn scroll_window(window: usize, current: usize, height: usize, len: usize) -> usize {
    if len == 0 || height == 0 {
        return 0;
    }
    let mut window = window;
    let last_visible = window + height - 1;
    if current > last_visible {
        window = current + 1 - height;
    }
    if current < window {
        window = current;
    }
    if window >= len {
        window = len - 1;
    }
    window
}

/// 0xRRGGBB → ratatui `Color`(注意 ratatui 是元组变体 `Color::Rgb(u8,u8,u8)`)。
fn rgb(color: u32) -> Color {
    Color::Rgb(
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    )
}

/// 把颜色索引解析成主题里的实际颜色。ghostty 的 bat 预览用 256 色 palette,而 Eggie 只有 16 色:
/// 索引 `<16` 直接命中(10/12 等本就在其中);238 是行号槽/分隔线的中性深灰,固定近似
/// xterm-256 的第 238 号(`#444444`);其余(本预览用不到)回退到前景色。
fn palette_color(theme: &TerminalTheme, index: usize) -> u32 {
    match index {
        i if i < 16 => theme.palette[i],
        238 => 0x44_4444,
        _ => theme.foreground,
    }
}

/// 调色板格子的编号标签:十进制右对齐 3 位,或十六进制两位。
fn palette_label(index: usize, hex_mode: bool) -> String {
    if hex_mode {
        format!(" {index:02x}")
    } else {
        format!("{index:3}")
    }
}

// --- 渲染 ------------------------------------------------------------------

/// 在 buffer 的 `(x, y)` 起始写一段文本(裁到 `right` 列),返回推进后的 x。
fn put(buf: &mut Buffer, x: u16, y: u16, right: u16, text: &str, style: Style) -> u16 {
    if x >= right || y >= buf.area.bottom() {
        return x;
    }
    let max = (right - x) as usize;
    let (nx, _) = buf.set_stringn(x, y, text, max, style);
    nx
}

/// 用 `style` 填满 `area`(背景铺色)。
fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    let clamped = area.intersection(buf.area);
    buf.set_style(clamped, style);
}

fn draw(buf: &mut Buffer, area: Rect, app: &mut App) {
    // UI chrome 底色铺满整屏。
    fill(buf, area, Style::default().fg(app.ui.fg).bg(app.ui.bg));

    let list_width = LIST_WIDTH.min(area.width);
    let list_area = Rect::new(area.x, area.y, list_width, area.height);
    app.list_area = list_area;

    // 滚动窗口在渲染时按当前 current 调整。
    app.window = scroll_window(app.window, app.current, list_area.height as usize, app.filtered.len());

    draw_list(buf, list_area, app);

    if area.width > list_width {
        let preview = Rect::new(area.x + list_width, area.y, area.width - list_width, area.height);
        draw_preview(buf, preview, app);
    }
}

/// 左栏主题列表:三态(normal / hover / selected),选中行 `❯ …❮`。
fn draw_list(buf: &mut Buffer, area: Rect, app: &App) {
    let standard = Style::default().fg(app.ui.fg).bg(app.ui.bg);
    fill(buf, area, standard);
    let right = area.right();

    for row in 0..area.height {
        let index = app.window + row as usize;
        if index >= app.filtered.len() {
            break;
        }
        let y = area.y + row;
        let name = &app.names[app.filtered[index]];

        let selected = index == app.current;
        let hovered = app.hover == Some(index);

        if selected {
            let style = Style::default().fg(app.ui.sel_fg).bg(app.ui.sel_bg);
            // 整行铺选中底色。
            fill(buf, Rect::new(area.x, y, area.width, 1), style);
            put(buf, area.x, y, right, "❯ ", style);
            put(buf, area.x + 2, y, right.saturating_sub(2), name, style);
            // 右侧收尾标记。
            put(buf, right.saturating_sub(2), y, right, " ❮", style);
        } else {
            let style = if hovered {
                Style::default().fg(app.ui.fg).bg(app.ui.hover_bg)
            } else {
                standard
            };
            if hovered {
                fill(buf, Rect::new(area.x, y, area.width, 1), style);
            }
            put(buf, area.x + 2, y, right, name, style);
        }
    }
}

/// 语法高亮 token 种类,映射到主题调色板角色(参照 `bat fibonacci.ts` 的实际着色语义)。
#[derive(Clone, Copy)]
enum Tok {
    /// 普通文本 / 标点(前景色)。
    Std,
    /// 关键字 `type`(青,palette 6)。
    Kw,
    /// 类型标识符(绿,palette 2)。
    Type,
    /// 运算符 `extends`/`=`/`?`/`:`/`|`/`...`(品红,palette 5)。
    Op,
    /// 数字(蓝,palette 4 —— palette 无紫,取蓝近似)。
    Num,
    /// 字符串 `'length'`(黄,palette 3)。
    Str,
    /// 注释(中性灰,palette 238)。
    Comment,
    /// 行号槽 / 分隔线(中性灰,palette 238)。
    Grid,
}

impl Tok {
    fn style(self, theme: &TerminalTheme, bg: Color) -> Style {
        let base = Style::default().bg(bg);
        match self {
            Tok::Std => base.fg(rgb(theme.foreground)),
            Tok::Kw => base.fg(rgb(palette_color(theme, 6))),
            Tok::Type => base.fg(rgb(palette_color(theme, 2))),
            Tok::Op => base.fg(rgb(palette_color(theme, 5))),
            Tok::Num => base.fg(rgb(palette_color(theme, 4))),
            Tok::Str => base.fg(rgb(palette_color(theme, 3))),
            Tok::Comment | Tok::Grid => base.fg(rgb(palette_color(theme, 238))),
        }
    }
}

/// `fibonacci.ts` 每行的语法高亮 token(不含行号槽),按 `bat` 的实际着色拆分。
#[rustfmt::skip]
const CODE_LINES: &[&[(&str, Tok)]] = &[
    &[("type", Tok::Kw), (" ", Tok::Std), ("Fibonacci", Tok::Type), ("<", Tok::Std)],
    &[("  ", Tok::Std), ("T", Tok::Type), (" ", Tok::Std), ("extends", Tok::Op), (" ", Tok::Std), ("number", Tok::Type), (",", Tok::Std)],
    &[("  ", Tok::Std), ("No", Tok::Type), (" ", Tok::Std), ("extends", Tok::Op), (" ", Tok::Std), ("1", Tok::Num), ("[] = [", Tok::Std), ("1", Tok::Num), (", ", Tok::Std), ("1", Tok::Num), (", ", Tok::Std), ("1", Tok::Num), ("],", Tok::Std)],
    &[("  ", Tok::Std), ("N_2", Tok::Type), (" ", Tok::Std), ("extends", Tok::Op), (" ", Tok::Std), ("1", Tok::Num), ("[] = [", Tok::Std), ("1", Tok::Num), ("],", Tok::Std)],
    &[("  ", Tok::Std), ("N_1", Tok::Type), (" ", Tok::Std), ("extends", Tok::Op), (" ", Tok::Std), ("1", Tok::Num), ("[] = [", Tok::Std), ("1", Tok::Num), ("]", Tok::Std)],
    &[("> ", Tok::Std), ("=", Tok::Op), (" ", Tok::Std), ("T", Tok::Type), (" ", Tok::Std), ("extends", Tok::Op), (" ", Tok::Std), ("1", Tok::Num), (" ", Tok::Std), ("|", Tok::Op), (" ", Tok::Std), ("2", Tok::Num)],
    &[("  ", Tok::Std), ("?", Tok::Op), (" ", Tok::Std), ("1", Tok::Num)],
    &[("  ", Tok::Std), (":", Tok::Op), (" ", Tok::Std), ("T", Tok::Type), (" ", Tok::Std), ("extends", Tok::Op), (" ", Tok::Std), ("No", Tok::Type), ("[", Tok::Std), ("'length'", Tok::Str), ("]", Tok::Std)],
    &[("  ", Tok::Std), ("?", Tok::Op), (" [", Tok::Std), ("...", Tok::Op), ("N_2", Tok::Type), (", ", Tok::Std), ("...", Tok::Op), ("N_1", Tok::Type), ("][", Tok::Std), ("'length'", Tok::Str), ("]", Tok::Std)],
    &[("  ", Tok::Std), (":", Tok::Op), (" ", Tok::Std), ("Fibonacci", Tok::Type), ("<", Tok::Std), ("T", Tok::Type), (", [", Tok::Std), ("...", Tok::Op), ("No", Tok::Type), (", ", Tok::Std), ("1", Tok::Num), ("], ", Tok::Std), ("N_1", Tok::Type), (", [", Tok::Std), ("...", Tok::Op), ("N_2", Tok::Type), (", ", Tok::Std), ("...", Tok::Op), ("N_1", Tok::Type), ("]>;", Tok::Std)],
    &[],
    &[("type", Tok::Kw), (" ", Tok::Std), ("FibonacciResult2", Tok::Type), (" ", Tok::Std), ("=", Tok::Op), (" ", Tok::Std), ("Fibonacci", Tok::Type), ("<", Tok::Std), ("8", Tok::Num), (">; ", Tok::Std), ("// 21", Tok::Comment)],
];

/// lorem ipsum 排版样例(节选自 ghostty 的 lorem_ipsum.txt)。
const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Cras hendrerit aliquet turpis non dictum. Mauris pulvinar nisl sit amet dui cursus tempus. Pellentesque ut dui justo. Etiam quis magna sagittis nisi pretium consequat vitae ut nisl. Sed at metus id odio pulvinar sodales. Vestibulum sollicitudin auctor enim, non fermentum erat. Praesent reprehenderit, dui quis convallis tempus, nunc.";

/// 右栏预览:像素级复刻 ghostty,全部用主题自身颜色。
fn draw_preview(buf: &mut Buffer, area: Rect, app: &App) {
    let Some(theme) = app.selected_theme() else {
        return;
    };
    if area.width < 8 || area.height < 6 {
        return; // 空间太小
    }

    let bg = rgb(theme.background);
    let standard = Style::default().fg(rgb(theme.foreground)).bg(bg);
    let bold_italic = standard.add_modifier(Modifier::BOLD | Modifier::ITALIC);
    let x0 = area.x;
    let right = area.right();
    fill(buf, area, standard);

    let mut y = area.y;

    // 1) 标题块(高 4):第 2 行居中主题名(bold italic),ghostty 的路径行 Eggie 无路径 → 留空。
    let title_y = y + 1;
    let name_w = theme.name.chars().count() as u16;
    let name_x = x0 + (area.width.saturating_sub(name_w)) / 2;
    put(buf, name_x, title_y, right, &theme.name, bold_italic);
    y += 4;

    // 2) 16 色调色板网格(高 6):每格编号 + 两行 ████。
    for i in 0..16u16 {
        let r = i / 8;
        let c = i % 8;
        let cell_x = x0 + c * 8;
        let cell_y = y + 3 * r;
        let label = palette_label(i as usize, app.hex);
        put(buf, cell_x, cell_y, right, &label, standard);
        let swatch = Style::default().fg(rgb(palette_color(theme, i as usize))).bg(bg);
        put(buf, cell_x + 4, cell_y, right, "████", swatch);
        put(buf, cell_x + 4, cell_y + 1, right, "████", swatch);
    }
    y += 6;

    // 3) bat fibonacci.ts:命令行、文件头、带行号的代码、starship 提示符。
    let grid = Tok::Grid.style(theme, bg);
    // 命令行:→ bat fibonacci.ts
    let mut x = x0 + 2;
    x = put(buf, x, y, right, "→", Tok::Type.style(theme, bg));
    x = put(buf, x, y, right, " bat ", standard);
    put(buf, x, y, right, "fibonacci.ts", Tok::Kw.style(theme, bg).add_modifier(Modifier::UNDERLINED));
    y += 1;
    draw_rule(buf, x0 + 2, y, right, '┬', grid);
    y += 1;
    let fx = put(buf, x0 + 2, y, right, "       │ File: ", grid);
    put(buf, fx, y, right, "fibonacci.ts", standard.add_modifier(Modifier::BOLD));
    y += 1;
    draw_rule(buf, x0 + 2, y, right, '┼', grid);
    y += 1;
    // 代码行(行号槽 + token)。
    for (i, line) in CODE_LINES.iter().enumerate() {
        if y >= area.bottom() {
            break;
        }
        let gutter = format!("  {:>2}   │ ", i + 1);
        let mut cx = put(buf, x0 + 2, y, right, &gutter, grid);
        for (text, tok) in line.iter() {
            cx = put(buf, cx, y, right, text, tok.style(theme, bg));
        }
        y += 1;
    }
    draw_rule(buf, x0 + 2, y, right, '┴', grid);
    y += 1;
    // starship 提示符两行。
    let mut sx = put(buf, x0 + 2, y, right, "Eggie ", Tok::Kw.style(theme, bg));
    sx = put(buf, sx, y, right, "on ", standard);
    sx = put(buf, sx, y, right, " main ", Tok::Num.style(theme, bg));
    sx = put(buf, sx, y, right, "[+] ", Tok::Op.style(theme, bg));
    sx = put(buf, sx, y, right, "via ", standard);
    put(buf, sx, y, right, " v0.13.0", Tok::Str.style(theme, bg));
    y += 1;
    let mut ax = put(buf, x0 + 2, y, right, "✦ ", Tok::Num.style(theme, bg));
    ax = put(buf, ax, y, right, "at ", standard);
    ax = put(buf, ax, y, right, "10:36:15 ", Tok::Str.style(theme, bg));
    put(buf, ax, y, right, "→", Tok::Type.style(theme, bg));
    y += 2;

    // 4) lorem ipsum 排版样例填满剩余高度。
    draw_lorem(buf, Rect::new(x0, y, area.width, area.bottom().saturating_sub(y)), theme, bg);
}

/// 画一条分隔线:`───────<joint>` 后用 `─` 铺到 `right`。
fn draw_rule(buf: &mut Buffer, x: u16, y: u16, right: u16, joint: char, style: Style) {
    let prefix = format!("───────{joint}");
    let nx = put(buf, x, y, right, &prefix, style);
    let mut cx = nx;
    while cx < right {
        cx = put(buf, cx, y, right, "─", style);
    }
}

/// 在 `area` 内按词换行排版 lorem 样例;特定词用不同样式演示(下划线变体因 ratatui 限制统一为普通下划线)。
fn draw_lorem(buf: &mut Buffer, area: Rect, theme: &TerminalTheme, bg: Color) {
    if area.height == 0 {
        return;
    }
    let standard = Style::default().fg(rgb(theme.foreground)).bg(bg);
    let bold = standard.add_modifier(Modifier::BOLD);
    let italic = standard.add_modifier(Modifier::ITALIC);
    let bold_italic = standard.add_modifier(Modifier::BOLD | Modifier::ITALIC);
    let underline = standard.add_modifier(Modifier::UNDERLINED);

    let mut y = area.y + 1;
    let mut x = area.x + 2;
    for word in LOREM.split_whitespace() {
        let w = word.chars().count() as u16;
        if x + w > area.right() {
            y += 1;
            x = area.x + 2;
        }
        if y >= area.bottom() {
            break;
        }
        let style = match word.trim_end_matches(|c: char| !c.is_alphanumeric()) {
            "ipsum" => standard.fg(rgb(palette_color(theme, 2))),
            "consectetur" => bold,
            "reprehenderit" => italic,
            "Praesent" => bold_italic,
            // dui/erat/enim/odio 在 ghostty 是 double/dashed/dotted/curly 下划线;
            // ratatui 的 Modifier 只有 UNDERLINED,统一降级为普通下划线。
            "auctor" | "dui" | "erat" | "enim" | "odio" => underline,
            _ => standard,
        };
        let nx = put(buf, x, y, area.right(), word, style);
        x = nx + 1; // 词间空格
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_uses_tuple_variant() {
        assert_eq!(rgb(0x0a141e), Color::Rgb(0x0a, 0x14, 0x1e));
    }

    #[test]
    fn palette_color_maps_indices() {
        let theme = theme_catalog()
            .theme_by_name("Builtin Dark")
            .expect("内置深色主题应存在");
        // <16 直接命中调色板。
        assert_eq!(palette_color(theme, 2), theme.palette[2]);
        assert_eq!(palette_color(theme, 10), theme.palette[10]);
        assert_eq!(palette_color(theme, 12), theme.palette[12]);
        // 238 是固定中性灰。
        assert_eq!(palette_color(theme, 238), 0x44_4444);
        // 其余回退前景。
        assert_eq!(palette_color(theme, 200), theme.foreground);
    }

    #[test]
    fn palette_label_dec_and_hex() {
        assert_eq!(palette_label(3, false), "  3");
        assert_eq!(palette_label(3, true), " 03");
        assert_eq!(palette_label(15, true), " 0f");
    }

    #[test]
    fn scroll_window_keeps_current_visible() {
        // height=10:选中在窗口内 → 不动。
        assert_eq!(scroll_window(0, 5, 10, 100), 0);
        // 选中越过底部 → 窗口下移。
        assert_eq!(scroll_window(0, 12, 10, 100), 3);
        // 选中在窗口上方 → 窗口上移到选中。
        assert_eq!(scroll_window(5, 2, 10, 100), 2);
        // 空列表 → 0。
        assert_eq!(scroll_window(3, 0, 10, 0), 0);
    }

    #[test]
    fn hit_test_maps_row_to_index() {
        let area = Rect::new(0, 0, 32, 10);
        // 命中第 3 行,窗口顶部 5 → index 8。
        assert_eq!(hit_test(area, 5, 100, 10, 3), Some(8));
        // 列超出列表宽 → None。
        assert_eq!(hit_test(area, 0, 100, 40, 3), None);
        // 行落在有效范围外(filtered_len 太小)→ None。
        assert_eq!(hit_test(area, 0, 2, 10, 5), None);
    }

    #[test]
    fn filter_cycles_all_dark_light() {
        assert_eq!(Filter::All.next(), Filter::Dark);
        assert_eq!(Filter::Dark.next(), Filter::Light);
        assert_eq!(Filter::Light.next(), Filter::All);
    }

    #[test]
    fn key_to_action_maps_navigation() {
        let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(key_to_action(k(KeyCode::Char('q'))), Action::Quit);
        assert_eq!(key_to_action(k(KeyCode::Esc)), Action::Quit);
        assert_eq!(key_to_action(k(KeyCode::Char('j'))), Action::Down(1));
        assert_eq!(key_to_action(k(KeyCode::Char('k'))), Action::Up(1));
        assert_eq!(key_to_action(k(KeyCode::PageDown)), Action::Down(PAGE));
        assert_eq!(key_to_action(k(KeyCode::Char('g'))), Action::Home);
        assert_eq!(key_to_action(k(KeyCode::Char('G'))), Action::End);
        assert_eq!(key_to_action(k(KeyCode::Char('x'))), Action::Hex(true));
        assert_eq!(key_to_action(k(KeyCode::Char('d'))), Action::Hex(false));
        assert_eq!(key_to_action(k(KeyCode::Char('f'))), Action::CycleFilter);
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
    }

    #[test]
    fn filter_recompute_partitions_by_darkness() {
        let names: Vec<String> = theme_catalog()
            .dark_names()
            .into_iter()
            .chain(theme_catalog().light_names())
            .collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|n| n.to_ascii_lowercase());
        sorted.dedup();
        let mut app = App::new(&sorted);
        app.filter = Filter::Dark;
        app.recompute_filtered();
        for &i in &app.filtered {
            let theme = theme_catalog().theme_by_name(&sorted[i]).unwrap();
            assert!(theme.is_dark());
        }
    }

    #[test]
    fn ui_palette_differs_by_mode() {
        let dark = UiPalette::new(true);
        let light = UiPalette::new(false);
        assert_eq!(dark.bg, Color::Rgb(0, 0, 0));
        assert_eq!(light.bg, Color::Rgb(0xff, 0xff, 0xff));
    }
}
