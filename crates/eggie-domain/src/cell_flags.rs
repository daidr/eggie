//! Stable cell attributes consumed by terminal views, independent of the terminal engine.
use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct TerminalCellFlags: u16 {
        const INVERSE = 1 << 0;
        const BOLD = 1 << 1;
        const ITALIC = 1 << 2;
        const UNDERLINE = 1 << 3;
        const WRAPLINE = 1 << 4;
        const WIDE_CHAR = 1 << 5;
        const WIDE_CHAR_SPACER = 1 << 6;
        const DIM = 1 << 7;
        const HIDDEN = 1 << 8;
        const STRIKEOUT = 1 << 9;
        const LEADING_WIDE_CHAR_SPACER = 1 << 10;
        const DOUBLE_UNDERLINE = 1 << 11;
        const UNDERCURL = 1 << 12;
        const DOTTED_UNDERLINE = 1 << 13;
        const DASHED_UNDERLINE = 1 << 14;
        const BOLD_ITALIC = Self::BOLD.bits() | Self::ITALIC.bits();
        const DIM_BOLD = Self::DIM.bits() | Self::BOLD.bits();
        const ALL_UNDERLINES = Self::UNDERLINE.bits() | Self::DOUBLE_UNDERLINE.bits()
            | Self::UNDERCURL.bits() | Self::DOTTED_UNDERLINE.bits() | Self::DASHED_UNDERLINE.bits();
    }
}
