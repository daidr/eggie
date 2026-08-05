use gpui::{IntoElement, div, prelude::*, px};

pub(crate) const FONT_FAMILY: &str = "Hugeicons Stroke Rounded";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IconName {
    Add,
    ArrowDown,
    Browser,
    Close,
    File,
    Folder,
    GitBranch,
    Info,
    PanelLeftClose,
    PanelLeftOpen,
    PanelRightClose,
    PanelRightOpen,
    Search,
    Settings,
    Terminal,
}

impl IconName {
    pub(crate) fn glyph(self) -> char {
        match self {
            Self::Add => '\u{F150C}',
            Self::ArrowDown => '\u{F15FC}',
            Self::Browser => '\u{F17C8}',
            Self::Close => '\u{F18BC}',
            Self::File => '\u{F1C6A}',
            Self::Folder => '\u{F1D01}',
            Self::GitBranch => '\u{F1D6B}',
            Self::Info => '\u{F1E70}',
            Self::PanelLeftClose => '\u{F22AE}',
            Self::PanelLeftOpen => '\u{F22B0}',
            Self::PanelRightClose => '\u{F22B2}',
            Self::PanelRightOpen => '\u{F22B4}',
            Self::Search => '\u{F2546}',
            Self::Settings => '\u{F257F}',
            Self::Terminal => '\u{F1836}',
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
