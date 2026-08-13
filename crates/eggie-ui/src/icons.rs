use gpui::{IntoElement, div, prelude::*, px};

pub(crate) const FONT_FAMILY: &str = "Hugeicons Stroke Rounded";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IconName {
    Add,
    AlertCircle,
    Appearance,
    /// Fallback opener glyph, reserved for the upcoming custom "open with" feature.
    #[allow(dead_code)]
    AppWindow,
    ArrowDown,
    ArrowRight,
    ArrowUp,
    Browser,
    CheckmarkCircle,
    Close,
    CombineIcon,
    Copy,
    Download,
    File,
    Finder,
    Folder,
    FolderOpen,
    GitBranch,
    Info,
    Keyboard,
    PanelLeftClose,
    PanelLeftOpen,
    PanelRightClose,
    PanelRightOpen,
    Refresh,
    Search,
    Settings,
    Terminal,
    VsCode,
}

impl IconName {
    pub(crate) fn glyph(self) -> char {
        match self {
            Self::Add => '\u{F150C}',
            Self::AlertCircle => '\u{F14EB}',
            Self::Appearance => '\u{F1A23}',
            Self::AppWindow => '\u{F1FB0}',
            Self::ArrowDown => '\u{F15FC}',
            Self::ArrowRight => '\u{F1623}',
            Self::ArrowUp => '\u{F1632}',
            Self::Browser => '\u{F17C8}',
            Self::CheckmarkCircle => '\u{F1970}',
            Self::Close => '\u{F18BC}',
            Self::CombineIcon => '\u{F1A28}',
            Self::Copy => '\u{F1A75}',
            Self::Download => '\u{F1BBA}',
            Self::File => '\u{F1C6A}',
            Self::Finder => '\u{F15D1}',
            Self::Folder => '\u{F1D01}',
            Self::FolderOpen => '\u{F1D02}',
            Self::GitBranch => '\u{F1D6B}',
            Self::Info => '\u{F1E70}',
            Self::Keyboard => '\u{F1F02}',
            Self::PanelLeftClose => '\u{F22AE}',
            Self::PanelLeftOpen => '\u{F22B0}',
            Self::PanelRightClose => '\u{F22B2}',
            Self::PanelRightOpen => '\u{F22B4}',
            Self::Refresh => '\u{F2449}',
            Self::Search => '\u{F2546}',
            Self::Settings => '\u{F257F}',
            Self::Terminal => '\u{F1836}',
            Self::VsCode => '\u{F297B}',
        }
    }
}

pub(crate) fn icon(name: IconName) -> impl IntoElement {
    icon_sized(name, 16.)
}

pub(crate) fn icon_sized(name: IconName, size: f32) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .font_family(FONT_FAMILY)
        .text_size(px(size))
        .child(name.glyph().to_string())
}
