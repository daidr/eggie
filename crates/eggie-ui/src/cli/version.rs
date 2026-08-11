//! `eggie +version` / `eggie --version`:打印版本与构建信息。

/// 版本号,来自 crate 的 Cargo.toml。
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// 构建标识,由 build.rs 生成(内容哈希),用于区分本地构建。
const BUILD_ID: &str = env!("EGGIE_BUILD_ID");

pub(crate) fn run(_flags: &[String]) -> i32 {
    let build_mode = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    println!("Eggie {VERSION}");
    println!();
    println!("Version");
    println!("  - version : {VERSION}");
    println!("  - build id: {BUILD_ID}");
    println!("  - build   : {build_mode}");

    0
}
