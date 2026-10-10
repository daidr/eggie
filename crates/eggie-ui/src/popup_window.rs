//! Small, non-activating windows for menus, popovers, and tooltips.
//!
//! GPUI's parent-anchored popup is not implemented on macOS yet. This shell keeps placement,
//! window lifetime, and motion independent of the content rendered inside it.

use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, Context, Pixels, Render, Size, Subscription,
    Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, div,
    point, prelude::*, px, rgb, size,
};
use std::{rc::Rc, time::Duration};

const IN_DURATION: Duration = Duration::from_millis(180);
const OUT_DURATION: Duration = Duration::from_millis(120);
const INSET: f32 = 12.;
const GAP: f32 = 5.;

type Content = Rc<dyn Fn(&mut Window, &mut Context<PopupWindow>) -> AnyElement>;
type Hover = Rc<dyn Fn(bool, &mut Window, &mut App)>;

#[derive(Clone, Copy)]
#[allow(dead_code)] // The window shell supports menu and tooltip placement before those callers exist.
pub(crate) enum PopupPlacement {
    Beside,
    Below,
    At(gpui::Point<Pixels>),
}

pub(crate) struct PopupWindowOptions {
    pub anchor: Bounds<Pixels>,
    pub content_size: Size<Pixels>,
    pub placement: PopupPlacement,
    pub background: u32,
    pub border: u32,
}

pub(crate) struct PopupWindow {
    content: Content,
    on_hover: Hover,
    background: u32,
    border: u32,
    content_size: Size<Pixels>,
    closing: bool,
    revision: u64,
    _parent_closed: Option<Subscription>,
}

impl PopupWindow {
    /// Open a window alongside an anchor measured in the parent window's local coordinates.
    /// It never takes keyboard focus from the parent. The returned handle can be dismissed with
    /// animation, and the window also closes if its parent closes.
    pub(crate) fn open(
        cx: &mut App,
        parent: &Window,
        options: PopupWindowOptions,
        content: impl Fn(&mut Window, &mut Context<Self>) -> AnyElement + 'static,
        on_hover: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> anyhow::Result<WindowHandle<Self>> {
        let display = parent.display(cx);
        let visible = display
            .as_ref()
            .map(|display| display.visible_bounds())
            .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), parent.bounds().size));
        let parent_bounds = parent.bounds();
        let extent = size(
            options.content_size.width + px(INSET * 2.),
            options.content_size.height + px(INSET * 2.),
        );
        let origin = place_popup(
            parent_bounds.origin,
            options.anchor,
            extent,
            visible,
            options.placement,
        );
        let parent_id = Window::window_handle(parent).window_id();
        let handle = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, extent))),
                kind: WindowKind::PopUp,
                focus: false,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                display_id: display.map(|display| display.id()),
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            move |_, cx| {
                cx.new(|_| Self {
                    content: Rc::new(content),
                    on_hover: Rc::new(on_hover),
                    background: options.background,
                    border: options.border,
                    content_size: options.content_size,
                    closing: false,
                    revision: 0,
                    _parent_closed: None,
                })
            },
        )?;
        let subscription = cx.on_window_closed(move |cx, closed| {
            if closed == parent_id {
                handle
                    .update(cx, |_, window, _| window.remove_window())
                    .ok();
            }
        });
        handle.update(cx, |popup, _, _| popup._parent_closed = Some(subscription))?;
        Ok(handle)
    }

    pub(crate) fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.revision += 1;
        let revision = self.revision;
        cx.notify();
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |popup, cx| {
            executor.timer(OUT_DURATION).await;
            let _ = cx.update(|window, cx| {
                popup
                    .update(cx, |popup, _| {
                        if popup.closing && popup.revision == revision {
                            window.remove_window();
                        }
                    })
                    .ok();
            });
        })
        .detach();
    }

    pub(crate) fn restore(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            self.closing = false;
            self.revision += 1;
            cx.notify();
        }
    }
}

impl Render for PopupWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = (self.content.clone())(window, cx);
        let closing = self.closing;
        let revision = self.revision;
        let hover = self.on_hover.clone();
        div()
            .id("popup-window-root")
            .size_full()
            .on_hover(move |hovered, window, cx| hover(*hovered, window, cx))
            .child(
                div()
                    .w(self.content_size.width)
                    .h(self.content_size.height)
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(self.border))
                    .bg(rgb(self.background))
                    .shadow_md()
                    .overflow_hidden()
                    .child(content)
                    .with_animation(
                        format!("popup-window-{revision}"),
                        Animation::new(if closing { OUT_DURATION } else { IN_DURATION })
                            .with_easing(gpui::ease_out_quint()),
                        move |surface, progress| {
                            let visible = if closing { 1. - progress } else { progress };
                            surface
                                .opacity(visible)
                                .ml(px(INSET))
                                .mt(px(INSET + (1. - visible) * 6.))
                        },
                    ),
            )
    }
}

fn place_popup(
    parent_origin: gpui::Point<Pixels>,
    anchor: Bounds<Pixels>,
    extent: Size<Pixels>,
    visible: Bounds<Pixels>,
    placement: PopupPlacement,
) -> gpui::Point<Pixels> {
    let (x, y) = match placement {
        PopupPlacement::Beside => {
            let right = parent_origin.x + anchor.right() + px(GAP);
            let left = parent_origin.x + anchor.left() - extent.width - px(GAP);
            let x = if right + extent.width <= visible.right() || left < visible.left() {
                right
            } else {
                left
            };
            (x, parent_origin.y + anchor.top())
        }
        PopupPlacement::Below => {
            let below = parent_origin.y + anchor.bottom() + px(GAP);
            let above = parent_origin.y + anchor.top() - extent.height - px(GAP);
            let y = if below + extent.height <= visible.bottom() || above < visible.top() {
                below
            } else {
                above
            };
            (parent_origin.x + anchor.left(), y)
        }
        PopupPlacement::At(position) => {
            (parent_origin.x + position.x, parent_origin.y + position.y)
        }
    };
    point(
        x.max(visible.left()).min(visible.right() - extent.width),
        y.max(visible.top()).min(visible.bottom() - extent.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_flips_and_clamps_inside_display() {
        let visible = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(300.)));
        let anchor = Bounds::new(point(px(80.), px(250.)), size(px(120.), px(30.)));
        let placed = place_popup(
            point(px(250.), px(20.)),
            anchor,
            size(px(180.), px(100.)),
            visible,
            PopupPlacement::Beside,
        );
        assert_eq!(placed, point(px(145.), px(200.)));
    }

    #[test]
    fn popup_below_flips_above_when_near_bottom() {
        let visible = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(300.)));
        let anchor = Bounds::new(point(px(80.), px(250.)), size(px(120.), px(30.)));
        let placed = place_popup(
            point(px(0.), px(0.)),
            anchor,
            size(px(180.), px(100.)),
            visible,
            PopupPlacement::Below,
        );
        assert_eq!(placed, point(px(80.), px(145.)));
    }

    #[test]
    fn point_popup_is_clamped_to_visible_bounds() {
        let visible = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(300.)));
        let placed = place_popup(
            point(px(100.), px(100.)),
            Bounds::default(),
            size(px(180.), px(100.)),
            visible,
            PopupPlacement::At(point(px(450.), px(250.))),
        );
        assert_eq!(placed, point(px(320.), px(200.)));
    }
}
