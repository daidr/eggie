use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAppearance {
    /// ANSI colors 0 through 15, encoded as `0xRRGGBB`.
    pub palette: [u32; 16],
    /// Default terminal colors, encoded as `0xRRGGBB`.
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    pub cursor_text: u32,
}

/// The 16 ANSI palette entries used when no theme overrides them. Shared by
/// [`TerminalAppearance::default`] and by the UI's theme seeding so the fallback colors have a
/// single source of truth.
pub const DEFAULT_PALETTE: [u32; 16] = [
    0x1d2027, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x5c6370,
    0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
];

impl Default for TerminalAppearance {
    fn default() -> Self {
        Self {
            palette: DEFAULT_PALETTE,
            foreground: 0xabb2bf,
            background: 0x1d2027,
            cursor: 0xabb2bf,
            cursor_text: 0x1d2027,
        }
    }
}

impl TerminalAppearance {
    pub fn color(&self, index: usize) -> Option<u32> {
        match index {
            0..=15 => Some(self.palette[index]),
            16..=231 => {
                const STEPS: [u32; 6] = [0, 95, 135, 175, 215, 255];
                let value = index - 16;
                Some((STEPS[value / 36] << 16) | (STEPS[(value / 6) % 6] << 8) | STEPS[value % 6])
            }
            232..=255 => {
                let value = 8 + 10 * (index as u32 - 232);
                Some((value << 16) | (value << 8) | value)
            }
            256 | 267 => Some(self.foreground),
            257 => Some(self.background),
            258 => Some(self.cursor),
            259..=266 => Some(dim_rgb(self.palette[index - 259])),
            268 => Some(dim_rgb(self.foreground)),
            _ => None,
        }
    }
}

fn dim_rgb(color: u32) -> u32 {
    let dim = |shift: u32| ((((color >> shift) & 0xff) as f32 * 0.66) as u32) & 0xff;
    (dim(16) << 16) | (dim(8) << 8) | dim(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_appearance_resolves_the_complete_alacritty_palette() {
        let appearance = TerminalAppearance::default();
        assert_eq!(appearance.color(0), Some(appearance.palette[0]));
        assert_eq!(appearance.color(16), Some(0x000000));
        assert_eq!(appearance.color(231), Some(0xffffff));
        assert_eq!(appearance.color(232), Some(0x080808));
        assert_eq!(appearance.color(255), Some(0xeeeeee));
        assert_eq!(appearance.color(256), Some(appearance.foreground));
        assert_eq!(appearance.color(257), Some(appearance.background));
        assert_eq!(appearance.color(258), Some(appearance.cursor));
        assert_eq!(appearance.color(268), Some(dim_rgb(appearance.foreground)));
        assert_eq!(dim_rgb(0x5f5f5f), 0x3e3e3e);
        assert_eq!(appearance.color(269), None);
    }
}
