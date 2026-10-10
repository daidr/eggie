//! Reusable GPUI controls and native popup surfaces. No workspace or terminal state.
pub mod continuous_corners;
pub mod icons;
pub mod popup_window;

pub fn init(cx: &mut gpui::App) {
    cx.text_system()
        .add_fonts(vec![std::borrow::Cow::Borrowed(include_bytes!(
            "../assets/hgi-stroke-rounded.ttf"
        ))])
        .expect("register icon font");
}

pub fn system_uses_dark_appearance() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .eq_ignore_ascii_case("dark")
            })
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub fn is_dark_appearance(appearance: gpui::WindowAppearance) -> bool {
    matches!(
        appearance,
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    )
}
