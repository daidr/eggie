//! `eggie +list-themes`:列出可用主题。
//!
//! 非交互(管道/重定向)或 `--plain` 时,按名称大小写不敏感排序逐行输出主题名,
//! 便于被 Unix 工具消费。在真实终端里(且未加 `--plain`)则进入交互式 TUI 预览
//! (见 [`super::tui`],阶段 4)。
//!
//! 说明:Eggie 的主题在编译期内置(`assets/ghostty-themes/`),运行时没有稳定可移植
//! 的磁盘路径,因此不提供 ghostty 的 `--path`。

use std::io::IsTerminal;

use crate::settings::theme_catalog;

/// 主题的明暗过滤范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    All,
    Dark,
    Light,
}

pub(crate) fn run(flags: &[String]) -> i32 {
    if flags.iter().any(|f| f == "--help" || f == "-h") {
        print_help();
        return 0;
    }

    let mut scope = Scope::All;
    let mut plain = false;
    for flag in flags {
        match flag.as_str() {
            "--dark" => scope = Scope::Dark,
            "--light" => scope = Scope::Light,
            "--plain" => plain = true,
            other => {
                eprintln!("错误:未知选项 `{other}`。运行 `eggie +list-themes --help` 查看用法。");
                return 1;
            }
        }
    }

    let names = theme_names(scope);

    // 仅在真实终端且未强制纯文本时进入 TUI 预览;否则输出纯文本列表。
    if !plain && std::io::stdout().is_terminal() {
        return super::tui::run(&names);
    }

    let mut out = String::new();
    for name in &names {
        out.push_str(name);
        out.push('\n');
    }
    print!("{out}");
    0
}

/// 按范围取出主题名,大小写不敏感排序并去重。
fn theme_names(scope: Scope) -> Vec<String> {
    let catalog = theme_catalog();
    let mut names = match scope {
        Scope::Dark => catalog.dark_names(),
        Scope::Light => catalog.light_names(),
        Scope::All => {
            let mut all = catalog.dark_names();
            all.extend(catalog.light_names());
            all
        }
    };
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names.dedup();
    names
}

fn print_help() {
    println!("用法: eggie +list-themes [--dark | --light] [--plain]");
    println!();
    println!("列出可用主题。在终端中运行时进入交互式预览,管道输出时退化为纯文本列表。");
    println!();
    println!("选项:");
    println!("  --dark   只列出深色主题");
    println!("  --light  只列出浅色主题");
    println!("  --plain  强制纯文本列表(即使在终端中)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_scope_is_sorted_and_deduped() {
        let names = theme_names(Scope::All);
        assert!(!names.is_empty());
        // 排序(大小写不敏感)且无相邻重复。
        for pair in names.windows(2) {
            assert!(
                pair[0].to_ascii_lowercase() <= pair[1].to_ascii_lowercase(),
                "未按大小写不敏感排序: {:?}",
                pair
            );
            assert_ne!(pair[0], pair[1], "存在重复项: {}", pair[0]);
        }
    }

    #[test]
    fn dark_scope_lists_only_dark_themes() {
        let catalog = theme_catalog();
        for name in theme_names(Scope::Dark) {
            let theme = catalog.theme_by_name(&name).expect("主题应存在");
            assert!(theme.is_dark(), "{name} 不是深色主题");
        }
    }

    #[test]
    fn light_scope_lists_only_light_themes() {
        let catalog = theme_catalog();
        for name in theme_names(Scope::Light) {
            let theme = catalog.theme_by_name(&name).expect("主题应存在");
            assert!(!theme.is_dark(), "{name} 不是浅色主题");
        }
    }

    #[test]
    fn dark_and_light_partition_all() {
        let all = theme_names(Scope::All).len();
        let dark = theme_names(Scope::Dark).len();
        let light = theme_names(Scope::Light).len();
        // All 是 dark+light 去重后的结果;考虑潜在同名去重,总数不超过分项之和。
        assert!(all <= dark + light);
        assert!(all >= dark);
        assert!(all >= light);
    }
}
