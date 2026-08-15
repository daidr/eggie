//! Standalone update window, modeled after the settings window. Shows the
//! current/new versions, release notes, and the daemon-restart warning when
//! the update changes the daemon protocol version.
//!
//! The window animates its height between states: the "available" state is
//! tall (version rows + scrollable release notes + actions), while the
//! downloading / ready / error states are compact. The resize uses AppKit's
//! native `setFrame:display:animate:` so the transition is smooth, matching
//! the behavior of first-party macOS apps.

use gpui::{
    AnyElement, App, Context, Entity, FontWeight, MouseButton, TitlebarOptions, Window,
    WindowBounds, WindowControlArea, WindowOptions, div, prelude::*, px, relative, rgb, rgba, size,
};

use crate::settings::{SettingsStore, UiColors};
use crate::update_controller::UpdateController;
use eggie_update::UpdateState;

const UPDATE_WINDOW_WIDTH: f32 = 520.;
/// Tall layout: version rows + scrollable release notes + actions.
const UPDATE_HEIGHT_TALL: f32 = 460.;
/// Compact layout: a line or two of status + actions.
const UPDATE_HEIGHT_COMPACT: f32 = 240.;
const UPDATE_TITLEBAR_HEIGHT: f32 = 52.;
const UPDATE_TRAFFIC_LIGHT_DIAMETER: f32 = 14.;
const UPDATE_TRAFFIC_LIGHT_LEFT: f32 = 12.;
const UPDATE_TITLE_LEFT_INSET: f32 = 84.;

pub(crate) fn open_update_window(
    updates: Entity<UpdateController>,
    settings: Entity<SettingsStore>,
    cx: &mut App,
) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<UpdateWindow>())
    {
        existing
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }

    cx.open_window(
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(settings.read(cx).config().language.update_window_title().into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(
                    px(UPDATE_TRAFFIC_LIGHT_LEFT),
                    px((UPDATE_TITLEBAR_HEIGHT - UPDATE_TRAFFIC_LIGHT_DIAMETER) / 2.),
                )),
            }),
            app_owns_titlebar_drag: true,
            window_bounds: Some(WindowBounds::centered(
                size(px(UPDATE_WINDOW_WIDTH), px(desired_height(updates.read(cx).state()))),
                cx,
            )),
            // Fixed-size utility window: no zoom (green) and no minimize (yellow).
            is_resizable: false,
            is_minimizable: false,
            focus: true,
            ..Default::default()
        },
        move |window, cx| {
            cx.new(|cx| UpdateWindow::new(updates.clone(), settings.clone(), window, cx))
        },
    )
    .expect("failed to open Eggie update window");
}

pub(crate) struct UpdateWindow {
    updates: Entity<UpdateController>,
    settings: Entity<SettingsStore>,
    /// True while a titlebar drag is in progress (mirrors the settings window).
    moving_window: bool,
    /// The height the window is currently sized to, so we only trigger an
    /// animated resize when the target height actually changes.
    current_height: f32,
}

impl UpdateWindow {
    fn new(
        updates: Entity<UpdateController>,
        settings: Entity<SettingsStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&updates, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        let current_height = desired_height(updates.read(cx).state());
        Self {
            updates,
            settings,
            moving_window: false,
            current_height,
        }
    }

    fn colors(&self, window: &Window, cx: &mut Context<Self>) -> UiColors {
        let settings = self.settings.read(cx);
        let system_is_dark = crate::settings_window::is_dark_appearance(window.appearance());
        UiColors::from_theme(settings.config().effective_theme(system_is_dark))
    }

    /// Titlebar with the window title, matching the settings window.
    fn render_titlebar(&self, title: &str, colors: UiColors, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("update-window-drag-region")
            .flex()
            .flex_none()
            .items_center()
            .h(px(UPDATE_TITLEBAR_HEIGHT))
            .pl(px(UPDATE_TITLE_LEFT_INSET))
            .pr_5()
            .border_b_1()
            .border_color(rgb(colors.border))
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.moving_window = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.moving_window = false),
            )
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.moving_window {
                    this.moving_window = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_string()),
            )
            .into_any_element()
    }
}

impl Render for UpdateWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.updates.read(cx).state().clone();
        let colors = self.colors(window, cx);
        let language = self.settings.read(cx).config().language;

        let body: AnyElement = match &state {
            UpdateState::Checking => centered_text(language.update_checking(), colors, true),

            UpdateState::UpToDate => centered_text(language.update_up_to_date(), colors, false),

            UpdateState::Error(message) => {
                let message = message.clone();
                let title = language.update_error_title();
                let retry = language.update_retry();
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .gap_4()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(0xff6b6b))
                            .child(format!("{title}: {message}")),
                    )
                    .child(div().flex_1().min_h_0())
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .justify_end()
                            .gap_2()
                            .child(button(
                                language.update_later(),
                                false,
                                colors,
                                cx.listener(|this, _, window, cx| {
                                    this.updates
                                        .update(cx, |controller, cx| controller.dismiss(cx));
                                    window.remove_window();
                                }),
                            ))
                            .child(button(
                                retry,
                                true,
                                colors,
                                cx.listener(|this, _, _, cx| {
                                    let channel =
                                        this.settings.read(cx).config().update_channel.slug();
                                    this.updates.update(cx, |controller, cx| {
                                        controller.check(false, channel, cx)
                                    });
                                }),
                            )),
                    )
                    .into_any_element()
            }

            UpdateState::Available(release) => {
                let compatible = release.daemon_compatible();
                let release_notes = release.release_notes.clone();
                let new_version = release.version.to_string();
                let current_version = env!("CARGO_PKG_VERSION").to_owned();
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .gap_4()
                    .child(version_rows(&current_version, &new_version, colors))
                    .when(!compatible, |this| {
                        this.child(daemon_warning(language))
                    })
                    .child(release_notes_view(&release_notes, colors))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .justify_end()
                            .gap_2()
                            .child(button(
                                language.update_later(),
                                false,
                                colors,
                                cx.listener(|this, _, window, cx| {
                                    this.updates
                                        .update(cx, |controller, cx| controller.dismiss(cx));
                                    window.remove_window();
                                }),
                            ))
                            .child(button(
                                language.update_download_and_install(),
                                true,
                                colors,
                                cx.listener(|this, _, _, cx| {
                                    let channel =
                                        this.settings.read(cx).config().update_channel.slug();
                                    this.updates.update(cx, |controller, cx| {
                                        controller.start_download(channel, cx)
                                    });
                                }),
                            )),
                    )
                    .into_any_element()
            }

            UpdateState::Downloading { progress, .. } => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(rgb(colors.text))
                        .child(language.update_downloading()),
                )
                .child(render_progress_bar(colors, *progress))
                .into_any_element(),

            UpdateState::Ready { release, .. } => {
                let compatible = release.daemon_compatible();
                let new_version = release.version.to_string();
                let current_version = env!("CARGO_PKG_VERSION").to_owned();
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .gap_4()
                    .child(version_rows(&current_version, &new_version, colors))
                    .when(!compatible, |this| {
                        this.child(daemon_warning(language))
                    })
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors.muted))
                            .child(if compatible {
                                language.update_ready_restart()
                            } else {
                                language.update_ready_restart_with_daemon()
                            }),
                    )
                    // Spacer pushes the action row to the bottom.
                    .child(div().flex_1().min_h_0())
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .justify_end()
                            .gap_2()
                            .child(button(
                                language.update_later(),
                                false,
                                colors,
                                cx.listener(|this, _, window, cx| {
                                    this.updates
                                        .update(cx, |controller, cx| controller.dismiss(cx));
                                    window.remove_window();
                                }),
                            ))
                            .child(button(
                                if compatible {
                                    language.update_restart()
                                } else {
                                    language.update_restart_with_daemon()
                                },
                                true,
                                colors,
                                cx.listener(|this, _, _, cx| {
                                    this.updates.update(cx, |controller, cx| {
                                        if let Err(error) = controller.install_and_restart(cx) {
                                            controller.report_error(format!("{error:#}"), cx);
                                        }
                                    });
                                }),
                            )),
                    )
                    .into_any_element()
            }

            UpdateState::Idle => centered_text(language.update_no_pending(), colors, true),
        };

        // Animate the window height to match the current state. We drive the
        // easing ourselves (one setFrame per frame, no AppKit animation) so
        // GPUI relayouts and repaints at the true size each frame. AppKit's
        // own animated setFrame: bitmap-scales the old frame, which visibly
        // squashes the Metal-rendered content mid-transition.
        let target_height = desired_height(&state);
        if (target_height - self.current_height).abs() > 0.5 {
            let from_height = self.current_height;
            self.current_height = target_height;
            cx.defer_in(window, move |_, window, cx| {
                let handle = window.window_handle();
                cx.spawn(async move |_this, cx| {
                    const FRAMES: u32 = 18;
                    for frame in 1..=FRAMES {
                        // Ease-out cubic for a natural settle.
                        let t = frame as f32 / FRAMES as f32;
                        let eased = 1.0 - (1.0 - t).powi(3);
                        let height = from_height + (target_height - from_height) * eased;
                        let still_open = cx.update(|cx| {
                            handle
                                .update(cx, |_, window, cx| {
                                    set_window_height(window, UPDATE_WINDOW_WIDTH, height);
                                    window.bounds_changed(cx);
                                })
                                .is_ok()
                        });
                        if !still_open {
                            break;
                        }
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(12))
                            .await;
                    }
                })
                .detach();
            });
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(colors.background))
            .text_color(rgb(colors.text))
            .font_family(".SystemUIFont")
            .child(self.render_titlebar(language.update_window_title(), colors, cx))
            .child(div().flex().flex_col().flex_1().min_h_0().p_6().child(body))
    }
}

/// The window height a given update state should occupy.
fn desired_height(state: &UpdateState) -> f32 {
    match state {
        // The available state shows version rows + scrollable release notes.
        UpdateState::Available(_) => UPDATE_HEIGHT_TALL,
        // Every other state is a short status line plus (maybe) actions.
        _ => UPDATE_HEIGHT_COMPACT,
    }
}

/// Resize the window to `width` x `height` immediately (no AppKit animation),
/// keeping the top-left corner fixed (AppKit's origin is bottom-left, so the y
/// coordinate is adjusted by the height delta). We drive the easing ourselves
/// one frame at a time so GPUI relayouts at the true size each frame.
#[cfg(target_os = "macos")]
fn set_window_height(window: &mut Window, width: f32, height: f32) {
    use cocoa::base::{id, nil, NO};
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let native_window: id = match HasWindowHandle::window_handle(window) {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::AppKit(appkit) => {
                let ns_view = appkit.ns_view.as_ptr() as id;
                if ns_view == nil {
                    return;
                }
                unsafe { msg_send![ns_view, window] }
            }
            _ => return,
        },
        Err(_) => return,
    };
    if native_window == nil {
        return;
    }

    unsafe {
        // Current frame in screen coordinates (origin bottom-left).
        let frame: cocoa::foundation::NSRect = msg_send![native_window, frame];
        let top = frame.origin.y + frame.size.height;
        let new_frame = cocoa::foundation::NSRect::new(
            cocoa::foundation::NSPoint::new(frame.origin.x, top - height as f64),
            cocoa::foundation::NSSize::new(width as f64, height as f64),
        );
        let _: () = msg_send![native_window, setFrame: new_frame display: NO animate: NO];
    }
}

#[cfg(not(target_os = "macos"))]
fn set_window_height(window: &mut Window, width: f32, height: f32) {
    window.resize(gpui::size(px(width), px(height)));
}

fn render_progress_bar(colors: UiColors, progress: eggie_update::DownloadProgress) -> AnyElement {
    // Only compute a fraction when the total size is actually known. A missing/zero total means
    // the fill width and percentage would be meaningless, so we leave the bar in an indeterminate
    // state rather than pinning it to a false 100%.
    let fraction = progress
        .total
        .filter(|&total| total > 0)
        .map(|total| (progress.downloaded as f32 / total as f32).clamp(0., 1.));
    let mut fill = div().h(px(6.)).rounded_full().bg(rgb(colors.accent));
    match fraction {
        // Known total: fill to the exact fraction.
        Some(fraction) => fill = fill.w(relative(fraction)),
        // Unknown total: a slim moving-less sliver signals activity without claiming a percentage.
        None => fill = fill.w(relative(0.15)),
    }
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .w_full()
                .h(px(6.))
                .rounded_full()
                .bg(rgb(colors.panel_alt))
                .child(fill),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .text_size(px(12.))
                .text_color(rgb(colors.muted))
                .child(div().child(format_speed(progress.bytes_per_sec)))
                .child(div().child(match fraction {
                    Some(fraction) => format!("{:.0}%", fraction * 100.),
                    None => String::new(),
                })),
        )
        .into_any_element()
}

fn button(
    label: &str,
    primary: bool,
    colors: UiColors,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(label.to_string())
        .flex_none()
        .h(px(30.))
        .px_4()
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(primary, |this| {
            this.bg(rgb(colors.accent))
                .text_color(rgb(0xffffff))
                .hover(move |element| element.bg(mix_toward_black(colors.accent, 0.12)))
        })
        .when(!primary, |this| {
            this.bg(rgb(colors.panel_alt))
                .text_color(rgb(colors.text))
                .border_1()
                .border_color(rgb(colors.border))
                .hover(move |element| element.bg(rgb(colors.hover)))
        })
        .on_click(on_click)
        .child(label.to_string())
        .into_any_element()
}

/// Darken a packed 0xRRGGBB color toward black by `amount` (0..1). Used to give
/// primary buttons a subtle hover state without introducing new theme tokens.
fn mix_toward_black(color: u32, amount: f32) -> gpui::Rgba {
    let channel = |shift: u32| {
        let value = ((color >> shift) & 0xff) as f32;
        (value * (1. - amount)).round().clamp(0., 255.) as u32
    };
    rgb((channel(16) << 16) | (channel(8) << 8) | channel(0))
}

fn centered_text(text: &str, colors: UiColors, muted: bool) -> AnyElement {
    div()
        .flex()
        .size_full()
        .items_center()
        .justify_center()
        .text_size(px(14.))
        .text_color(rgb(if muted { colors.muted } else { colors.text }))
        .child(text.to_string())
        .into_any_element()
}

fn version_rows(current: &str, new: &str, colors: UiColors) -> AnyElement {
    fn row(label: &str, value: &str, colors: UiColors) -> AnyElement {
        div()
            .flex()
            .justify_between()
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(colors.muted))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(colors.text))
                    .child(value.to_string()),
            )
            .into_any_element()
    }
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(row("当前版本", current, colors))
        .child(row("新版本", new, colors))
        .into_any_element()
}

fn daemon_warning(language: crate::settings::Language) -> AnyElement {
    // Amber warning tint that reads on both light and dark backgrounds.
    div()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(rgb(0xd08700))
        .bg(rgba(0xd0870022))
        .text_size(px(12.))
        .text_color(rgb(0xb87400))
        .child(language.update_daemon_warning())
        .into_any_element()
}

fn release_notes_view(notes: &str, colors: UiColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .flex_1()
        .min_h(px(0.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(colors.muted))
                .child("更新内容"),
        )
        .child(
            div()
                .id("update-release-notes")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.panel))
                .child(crate::markdown::markdown_element(
                    notes,
                    crate::markdown::MarkdownStyle {
                        colors,
                        base_size: 13.,
                    },
                )),
        )
        .into_any_element()
}

fn format_speed(bytes_per_sec: u64) -> String {
    if bytes_per_sec >= 1_000_000 {
        format!("{:.1} MB/s", bytes_per_sec as f64 / 1_000_000.)
    } else if bytes_per_sec >= 1_000 {
        format!("{:.0} KB/s", bytes_per_sec as f64 / 1_000.)
    } else {
        format!("{bytes_per_sec} B/s")
    }
}
