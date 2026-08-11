//! `eggie +list-colors`:列出内置的 X11 命名颜色。
//!
//! 管道/重定向或 `--plain` 时输出对齐 ghostty 的纯文本形式:按颜色名大小写不敏感
//! 排序,逐行 `name = #rrggbb`,便于被 Unix 工具消费。在真实终端里(且未加 `--plain`)
//! 则参考 ghostty 的 `prettyPrint`,以真彩色色块 + 多列布局展示。同色的多个命名别名
//! 都会列出。

use std::io::IsTerminal;

use super::colors::x11_colors;

/// 无法探测终端宽度时的回退列宽。
const FALLBACK_WIDTH: usize = 80;

pub(crate) fn run(flags: &[String]) -> i32 {
    let mut plain = false;
    for flag in flags {
        match flag.as_str() {
            "--help" | "-h" => {
                print_help();
                return 0;
            }
            "--plain" => plain = true,
            other => {
                eprintln!("错误:未知选项 `{other}`。运行 `eggie +list-colors --help` 查看用法。");
                return 1;
            }
        }
    }

    let mut colors: Vec<&(String, (u8, u8, u8))> = x11_colors().iter().collect();
    colors.sort_by_key(|(name, _)| name.to_ascii_lowercase());

    if !plain && std::io::stdout().is_terminal() {
        print!("{}", render_pretty(&colors, terminal_width()));
    } else {
        print!("{}", render_plain(&colors));
    }

    0
}

/// 纯文本:每行 `name = #rrggbb`,无 ANSI,便于管道消费。
fn render_plain(colors: &[&(String, (u8, u8, u8))]) -> String {
    let mut out = String::new();
    for (name, (r, g, b)) in colors {
        out.push_str(&format!("{name} = #{r:02x}{g:02x}{b:02x}\n"));
    }
    out
}

/// 多列彩色布局,参考 ghostty 的 `prettyPrint`:每格是「名称 = #rrggbb ▉▉」,hex 用该色
/// 作前景、末尾两格实色块用该色作背景。列数由终端宽度决定,按列主序填充(先填满一列的所有
/// 行,再下一列),这样每一列内部是排序连续的。
fn render_pretty(colors: &[&(String, (u8, u8, u8))], width: usize) -> String {
    if colors.is_empty() {
        return String::new();
    }

    let max_name = colors.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    // 每格内容宽度 = 名称 + " = #rrggbb " + 两格色块(2),外加列间 2 空格 gutter。
    // " = " (3) + "#rrggbb" (7) + " " (1) + 色块 (2) + gutter (2) = 15。
    let column_size = max_name + 15;
    // 至少一列;宽度不足以容纳一整列时退化为单列。
    let columns = ((width + 2) / column_size).max(1);
    // 每列的行数(向上取整):这样 columns 列足以放下全部条目。
    let rows = colors.len().div_ceil(columns);

    let mut out = String::new();
    for row in 0..rows {
        for col in 0..columns {
            // 列主序:第 col 列第 row 行对应排序后的第 row + rows*col 项。
            let index = row + rows * col;
            let Some((name, (r, g, b))) = colors.get(index).copied() else {
                continue;
            };
            let last_in_row = col + 1 == columns || index + rows >= colors.len();
            let hex = format!("#{r:02x}{g:02x}{b:02x}");
            // 名称左对齐补齐到 max_name,再接 " = "、着色 hex、空格、实色块。
            out.push_str(&format!("{name:<max_name$} = "));
            out.push_str(&format!("\x1b[38;2;{r};{g};{b}m{hex}\x1b[0m "));
            out.push_str(&format!("\x1b[48;2;{r};{g};{b}m  \x1b[0m"));
            if !last_in_row {
                out.push_str("  "); // 列间 gutter
            }
        }
        out.push('\n');
    }
    out
}

/// 终端宽度(列数),探测失败时回退到 [`FALLBACK_WIDTH`]。
fn terminal_width() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(cols, _)| cols as usize)
        .filter(|&cols| cols > 0)
        .unwrap_or(FALLBACK_WIDTH)
}

fn print_help() {
    println!("用法: eggie +list-colors [--plain]");
    println!();
    println!("列出内置的 X11 命名颜色。在终端中运行时以彩色多列布局展示,");
    println!("管道输出时退化为纯文本 `name = #rrggbb`(按名称排序)。");
    println!();
    println!("选项:");
    println!("  --plain  强制纯文本列表(即使在终端中)");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<(String, (u8, u8, u8))> {
        vec![
            ("red".to_owned(), (255, 0, 0)),
            ("green".to_owned(), (0, 128, 0)),
            ("blue".to_owned(), (0, 0, 255)),
        ]
    }

    #[test]
    fn plain_output_is_ansi_free_and_sorted_format() {
        let colors = sample();
        let refs: Vec<&(String, (u8, u8, u8))> = colors.iter().collect();
        let out = render_plain(&refs);
        assert_eq!(out, "red = #ff0000\ngreen = #008000\nblue = #0000ff\n");
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn pretty_output_carries_truecolor_sequences() {
        let colors = sample();
        let refs: Vec<&(String, (u8, u8, u8))> = colors.iter().collect();
        let out = render_pretty(&refs, 80);
        // 前景色 hex + 背景色块的真彩色序列都应出现。
        assert!(out.contains("\x1b[38;2;255;0;0m#ff0000\x1b[0m"));
        assert!(out.contains("\x1b[48;2;0;0;255m  \x1b[0m"));
        // 每个名称都在。
        for name in ["red", "green", "blue"] {
            assert!(out.contains(name), "缺少 {name}");
        }
    }

    #[test]
    fn narrow_width_falls_back_to_single_column() {
        let colors = sample();
        let refs: Vec<&(String, (u8, u8, u8))> = colors.iter().collect();
        // 宽度不足一列 → 单列 → 3 行。
        let out = render_pretty(&refs, 1);
        assert_eq!(out.lines().count(), 3);
    }

    #[test]
    fn wide_width_uses_multiple_columns() {
        let colors = sample();
        let refs: Vec<&(String, (u8, u8, u8))> = colors.iter().collect();
        // 足够宽 → 3 项排进 1 行(多列)。
        let out = render_pretty(&refs, 500);
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn empty_input_produces_no_output() {
        assert_eq!(render_pretty(&[], 80), "");
        assert_eq!(render_plain(&[]), "");
    }
}
