//! 内置的 X11 命名颜色表,供 `eggie +list-colors` 使用。
//!
//! 数据 `assets/rgb.txt` 源自 X11 项目(MIT/X11 许可),见 `rgb.txt.LICENSE`。
//! 每行格式为 `R G B<TAB><TAB>name`:R/G/B 是十进制 0-255,name 可含空格,
//! 同色可有多个命名别名(各占一行),全部保留、不去重 —— 与 ghostty 一致。

use std::sync::OnceLock;

/// 一个命名颜色:名称与其 RGB 分量。
pub(super) type NamedColor = (String, (u8, u8, u8));

/// 编译期内置的 rgb.txt 原文。
const RGB_TXT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/rgb.txt"));

/// 解析一行 rgb.txt。成功返回 `(name, (r, g, b))`;空行或格式不符返回 `None`。
fn parse_rgb_line(line: &str) -> Option<NamedColor> {
    // name 前是制表符,颜色分量在制表符之前、以空白分隔。
    let (numbers, name) = line.split_once('\t')?;
    let mut parts = numbers.split_whitespace();
    let r = parts.next()?.parse::<u8>().ok()?;
    let g = parts.next()?.parse::<u8>().ok()?;
    let b = parts.next()?.parse::<u8>().ok()?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((name.to_owned(), (r, g, b)))
}

/// 惰性解析出的全部命名颜色,保持 rgb.txt 中的原始顺序与别名。
///
/// 仅在用户显式运行 `+list-colors` 时才会解析一次,GUI 启动路径零成本。
pub(super) fn x11_colors() -> &'static [NamedColor] {
    static COLORS: OnceLock<Vec<NamedColor>> = OnceLock::new();
    COLORS.get_or_init(|| RGB_TXT.lines().filter_map(parse_rgb_line).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_simple_line() {
        assert_eq!(
            parse_rgb_line("255 250 250\t\tsnow"),
            Some(("snow".to_owned(), (255, 250, 250)))
        );
    }

    #[test]
    fn parses_a_name_with_spaces() {
        assert_eq!(
            parse_rgb_line("248 248 255\t\tghost white"),
            Some(("ghost white".to_owned(), (248, 248, 255)))
        );
    }

    #[test]
    fn rejects_blank_or_malformed_lines() {
        assert_eq!(parse_rgb_line(""), None);
        assert_eq!(parse_rgb_line("not a color"), None);
        assert_eq!(parse_rgb_line("1 2\t\tmissing-blue"), None);
    }

    #[test]
    fn built_in_table_has_the_full_x11_set() {
        // rgb.txt 是 782 行且无空行/注释,应全部解析成功。
        assert_eq!(x11_colors().len(), 782);
    }

    #[test]
    fn known_colors_resolve_to_expected_values() {
        let colors = x11_colors();
        let teal = colors.iter().find(|(name, _)| name == "teal").unwrap();
        assert_eq!(teal.1, (0, 128, 128));
    }
}
