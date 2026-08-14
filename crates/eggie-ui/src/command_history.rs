//! 命令历史面板的纯逻辑:退出码染色分类与相对时间格式化。
//!
//! 数据源是协议层已有的 OSC 133 shell 集成状态([`eggie_protocol::TerminalShellIntegrationState`]),
//! 由 app.rs 侧的「仅当 Commands tab 可见时」轮询拉取并缓存。渲染与轮询在 app.rs;这里只放两个
//! **纯函数**(退出码分类、相对时间),独立可测(仿 `text_input.rs` 里 `should_coalesce` 的可测风格)。

/// 命令的完成状态,决定列表条目左侧圆点的语义颜色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandStatus {
    /// 尚在运行(无 `exit_code`)—— 中性/进行中。
    Running,
    /// 成功退出(`exit_code == Some(0)`)—— 绿。
    Success,
    /// 失败退出(`exit_code == Some(非0)`)—— 红,并展示退出码。
    Failure(i32),
}

impl CommandStatus {
    /// 从可选退出码分类。语义就是 handoff 约定的三态:进行中 / 0 / 非0。
    pub(crate) fn from_exit_code(exit_code: Option<i32>) -> Self {
        match exit_code {
            None => Self::Running,
            Some(0) => Self::Success,
            Some(code) => Self::Failure(code),
        }
    }
}

/// 把「命令开始的 Unix 毫秒」相对「当前 Unix 毫秒」格式化成紧凑英数相对时间。
/// 未来时间(时钟偏移)钳到「just now」。粒度:秒 / 分 / 时 / 天。
pub(crate) fn format_relative_time(started_at_unix_ms: u64, now_unix_ms: u64) -> String {
    let elapsed_ms = now_unix_ms.saturating_sub(started_at_unix_ms);
    let seconds = elapsed_ms / 1_000;
    if seconds < 5 {
        return "just now".to_owned();
    }
    if seconds < 60 {
        return format!("{seconds}s ago");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_exit_codes_into_three_states() {
        assert_eq!(CommandStatus::from_exit_code(None), CommandStatus::Running);
        assert_eq!(CommandStatus::from_exit_code(Some(0)), CommandStatus::Success);
        assert_eq!(
            CommandStatus::from_exit_code(Some(1)),
            CommandStatus::Failure(1)
        );
        assert_eq!(
            CommandStatus::from_exit_code(Some(130)),
            CommandStatus::Failure(130)
        );
    }

    #[test]
    fn formats_relative_time_across_granularities() {
        let base = 1_000_000_000u64;
        assert_eq!(format_relative_time(base, base), "just now");
        assert_eq!(format_relative_time(base, base + 3_000), "just now");
        assert_eq!(format_relative_time(base, base + 12_000), "12s ago");
        assert_eq!(format_relative_time(base, base + 90_000), "1m ago");
        assert_eq!(format_relative_time(base, base + 3 * 3_600_000), "3h ago");
        assert_eq!(format_relative_time(base, base + 2 * 86_400_000), "2d ago");
    }

    #[test]
    fn clamps_future_timestamps_to_just_now() {
        let base = 1_000_000_000u64;
        // now 在 started 之前(时钟偏移/回拨):不 panic,视作刚刚。
        assert_eq!(format_relative_time(base + 5_000, base), "just now");
    }
}
