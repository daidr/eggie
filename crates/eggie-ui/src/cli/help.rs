//! `eggie +help` / `eggie -h` / `eggie --help`:总览帮助。

use super::Action;

pub(crate) fn run(_flags: &[String]) -> i32 {
    println!("用法: eggie [+action] [选项]");
    println!();
    println!("不带 +action 时启动 Eggie 终端;带 +action 时执行对应的辅助命令。");
    println!();
    println!("可用 action:");
    println!();

    // 对齐名字列宽,让说明整齐。
    let width = Action::ALL
        .iter()
        .map(|a| a.name().len())
        .max()
        .unwrap_or(0);
    for action in Action::ALL.iter().copied() {
        println!(
            "  +{name:<width$}  {summary}",
            name = action.name(),
            summary = action.summary(),
        );
    }

    println!();
    println!("用 `eggie +<action> --help` 查看具体命令的帮助。");

    0
}
