//! Small, non-activating windows for menus, popovers, and tooltips.
//!
//! GPUI's parent-anchored popup is not implemented on macOS yet. This shell keeps placement,
//! window lifetime, and motion independent of the content rendered inside it.

mod platform;

use crate::continuous_corners::div;
use gpui::{
    AnyElement, App, Bounds, Context, Pixels, Render, Size, Subscription, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, point,
    prelude::*, px, rgb, size,
};
use platform::PopupSurface;
use std::{rc::Rc, time::Duration};

const IN_DURATION: Duration = Duration::from_millis(180);
const OUT_DURATION: Duration = Duration::from_millis(160);
const MOVE_DURATION: Duration = Duration::from_millis(150);
const INSET: f32 = 12.;
const GAP: f32 = 5.;

type Content = Rc<dyn Fn(&mut Window, &mut Context<PopupWindow>) -> AnyElement>;
type Hover = Rc<dyn Fn(bool, &mut Window, &mut App)>;

#[derive(Clone, Copy)]
#[allow(dead_code)] // The window shell supports menu and tooltip placement before those callers exist.
pub enum PopupPlacement {
    Beside,
    Below,
    At(gpui::Point<Pixels>),
}

pub struct PopupWindowOptions {
    /// The visible anchor in parent-local coordinates; placement aligns the popup surface.
    pub anchor: Bounds<Pixels>,
    pub content_size: Size<Pixels>,
    pub placement: PopupPlacement,
    pub background: u32,
    pub border: u32,
}

pub struct PopupWindow {
    content: Content,
    on_hover: Hover,
    background: u32,
    border: u32,
    content_size: Size<Pixels>,
    anchor: Bounds<Pixels>,
    placement: PopupPlacement,
    closing: bool,
    bounds: Bounds<Pixels>,
    surface: Option<PopupSurface>,
    pointer_pressed: bool,
    revision: u64,
    _parent_closed: Option<Subscription>,
}

impl PopupWindow {
    /// Open a window alongside an anchor measured in the parent window's local coordinates.
    /// It never takes keyboard focus from the parent. The returned handle can be dismissed with
    /// animation, and the window also closes if its parent closes.
    pub fn open(
        cx: &mut App,
        parent: &Window,
        options: PopupWindowOptions,
        content: impl Fn(&mut Window, &mut Context<Self>) -> AnyElement + 'static,
        on_hover: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> anyhow::Result<WindowHandle<Self>> {
        let display = parent.display(cx);
        let bounds = popup_bounds(parent, &options, cx);
        let parent_id = Window::window_handle(parent).window_id();
        let handle = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                kind: WindowKind::PopUp,
                focus: false,
                show: !platform::NATIVE,
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
                    anchor: options.anchor,
                    placement: options.placement,
                    closing: false,
                    bounds,
                    surface: None,
                    pointer_pressed: false,
                    revision: 0,
                    _parent_closed: None,
                })
            },
        )?;
        let initialized = handle.update(cx, |popup, window, cx| -> anyhow::Result<()> {
            popup.surface = Some(PopupSurface::new(window)?);
            // open_window has drawn the scene; submit it before exposing the native surface.
            window.present_if_needed();
            let duration = if cx.reduce_motion() {
                Duration::ZERO
            } else {
                IN_DURATION
            };
            let _ = popup
                .surface
                .as_mut()
                .unwrap()
                .transition(bounds, 1., duration, cx);
            Ok(())
        });
        if let Err(error) = initialized.and_then(|result| result) {
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
            return Err(error);
        }
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

    /// Replace the content and anchor without reopening the window or replaying its entrance.
    pub fn retarget(
        &mut self,
        parent: &Window,
        options: PopupWindowOptions,
        content: impl Fn(&mut Window, &mut Context<Self>) -> AnyElement + 'static,
        on_hover: impl Fn(bool, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = popup_bounds(parent, &options, cx);
        self.content = Rc::new(content);
        self.on_hover = Rc::new(on_hover);
        self.background = options.background;
        self.border = options.border;
        self.content_size = options.content_size;
        self.anchor = options.anchor;
        self.placement = options.placement;
        self.restore(cx);
        self.update_bounds(bounds, window, cx);
    }

    /// Reflow live content without replacing its callbacks or restarting its visibility animation.
    pub fn resize_content(
        &mut self,
        parent: &Window,
        content_size: Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closing || self.content_size == content_size {
            return;
        }
        self.content_size = content_size;
        let bounds = popup_bounds(
            parent,
            &PopupWindowOptions {
                anchor: self.anchor,
                content_size,
                placement: self.placement,
                background: self.background,
                border: self.border,
            },
            cx,
        );
        self.update_bounds(bounds, window, cx);
    }

    fn update_bounds(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bounds = bounds;
        if !platform::NATIVE && window.bounds().size != bounds.size {
            window.resize(bounds.size);
        }
        if let Some(surface) = &mut self.surface {
            let duration = if cx.reduce_motion() {
                Duration::ZERO
            } else {
                MOVE_DURATION
            };
            let _ = surface.transition(bounds, 1., duration, cx);
        }
        cx.notify();
    }

    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        if cx.reduce_motion() || self.surface.is_none() {
            window.remove_window();
            return;
        }
        self.revision += 1;
        let revision = self.revision;
        let mut exit_bounds = self.bounds;
        exit_bounds.origin.y += px(6.);
        let completed =
            self.surface
                .as_mut()
                .unwrap()
                .transition(exit_bounds, 0., OUT_DURATION, cx);
        cx.spawn_in(window, async move |popup, cx| {
            if completed.await.is_ok() {
                let _ = cx.update(|window, cx| {
                    popup
                        .update(cx, |popup, _| {
                            // AppKit also completes cancelled animations, which may belong to an old dismissal.
                            if popup.closing && popup.revision == revision {
                                window.remove_window();
                            }
                        })
                        .ok();
                });
            }
        })
        .detach();
    }

    pub fn restore(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            self.closing = false;
            self.revision += 1;
            if let Some(surface) = &mut self.surface {
                let duration = if cx.reduce_motion() {
                    Duration::ZERO
                } else {
                    IN_DURATION
                };
                let _ = surface.transition(self.bounds, 1., duration, cx);
            }
            cx.notify();
        }
    }
    pub fn pointer_pressed(&self) -> bool {
        self.pointer_pressed
    }
}

impl Render for PopupWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = (self.content.clone())(window, cx);
        let hover = self.on_hover.clone();
        let release_outside = self.on_hover.clone();
        div()
            .id("popup-window-root")
            .size_full()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|popup, _, _, _| popup.pointer_pressed = true),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|popup, _, _, _| popup.pointer_pressed = false),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(move |popup, _, window, cx| {
                    popup.pointer_pressed = false;
                    release_outside(false, window, cx);
                }),
            )
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
                    .ml(px(INSET))
                    .mt(px(INSET)),
            )
    }
}

fn popup_bounds(parent: &Window, options: &PopupWindowOptions, cx: &App) -> Bounds<Pixels> {
    let visible = parent
        .display(cx)
        .map(|display| display.visible_bounds())
        .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), parent.bounds().size));
    let extent = size(
        options.content_size.width + px(INSET * 2.),
        options.content_size.height + px(INSET * 2.),
    );
    Bounds::new(
        place_popup(
            parent.bounds().origin,
            options.anchor,
            extent,
            visible,
            options.placement,
        ),
        extent,
    )
}

fn place_popup(
    parent_origin: gpui::Point<Pixels>,
    anchor: Bounds<Pixels>,
    extent: Size<Pixels>,
    visible: Bounds<Pixels>,
    placement: PopupPlacement,
) -> gpui::Point<Pixels> {
    let surface_size = size(
        extent.width - px(INSET * 2.),
        extent.height - px(INSET * 2.),
    );
    let (x, y) = match placement {
        PopupPlacement::Beside => {
            let right = parent_origin.x + anchor.right() + px(GAP);
            let left = parent_origin.x + anchor.left() - surface_size.width - px(GAP);
            let x = if right + surface_size.width + px(INSET) <= visible.right()
                || left - px(INSET) < visible.left()
            {
                right
            } else {
                left
            };
            (x, parent_origin.y + anchor.top())
        }
        PopupPlacement::Below => {
            let below = parent_origin.y + anchor.bottom() + px(GAP);
            let above = parent_origin.y + anchor.top() - surface_size.height - px(GAP);
            let y = if below + surface_size.height + px(INSET) <= visible.bottom()
                || above - px(INSET) < visible.top()
            {
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
    // Shadow padding belongs outside the aligned surface, not between it and the anchor.
    point(
        (x - px(INSET))
            .max(visible.left())
            .min(visible.right() - extent.width),
        (y - px(INSET))
            .max(visible.top())
            .min(visible.bottom() - extent.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Parent;

    impl Render for Parent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn options(y: f32) -> PopupWindowOptions {
        PopupWindowOptions {
            anchor: Bounds::new(point(px(10.), px(y)), size(px(120.), px(24.))),
            content_size: size(px(200.), px(160.)),
            placement: PopupPlacement::Beside,
            background: 0xffffff,
            border: 0xdddddd,
        }
    }

    fn open_popup(cx: &mut gpui::TestAppContext) -> WindowHandle<PopupWindow> {
        let parent = cx.add_window(|_, _| Parent);
        cx.update(|cx| {
            parent
                .update(cx, |_, window, cx| {
                    PopupWindow::open(
                        cx,
                        window,
                        options(20.),
                        |_, _| div().into_any_element(),
                        |_, _, _| {},
                    )
                    .unwrap()
                })
                .unwrap()
        })
    }

    #[gpui::test]
    fn native_completion_closes_once_without_scheduling_content_animation(
        cx: &mut gpui::TestAppContext,
    ) {
        let popup = open_popup(cx);
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            popup
                .update(cx, |popup, window, cx| {
                    assert_eq!(window.simulate_next_frame(cx), 0);
                    popup.dismiss(window, cx);
                    popup.dismiss(window, cx);
                    assert_eq!(popup.revision, 1);
                })
                .unwrap();
        });
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| cx.windows().len()), 2);
        cx.executor().advance_clock(OUT_DURATION);
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| cx.windows().len()), 1);
    }

    #[gpui::test]
    fn restored_window_ignores_old_completion_and_can_be_dismissed_again(
        cx: &mut gpui::TestAppContext,
    ) {
        let popup = open_popup(cx);
        cx.update(|cx| {
            popup
                .update(cx, |popup, window, cx| popup.dismiss(window, cx))
                .unwrap();
        });
        cx.run_until_parked();
        cx.executor().advance_clock(OUT_DURATION / 2);
        cx.update(|cx| {
            popup
                .update(cx, |popup, _, cx| {
                    popup.restore(cx);
                    assert!(!popup.closing);
                    assert_eq!(popup.revision, 2);
                })
                .unwrap();
        });
        cx.run_until_parked();
        cx.executor().advance_clock(OUT_DURATION);
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| cx.windows().len()), 2);
        cx.update(|cx| {
            popup
                .update(cx, |popup, window, cx| popup.dismiss(window, cx))
                .unwrap();
        });
        cx.run_until_parked();
        cx.executor().advance_clock(OUT_DURATION);
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| cx.windows().len()), 1);
    }

    #[gpui::test]
    fn reduce_motion_dismisses_without_waiting_for_animation(cx: &mut gpui::TestAppContext) {
        let popup = open_popup(cx);
        cx.update(|cx| {
            cx.set_reduce_motion(true);
            popup
                .update(cx, |popup, window, cx| popup.dismiss(window, cx))
                .unwrap();
        });
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| cx.windows().len()), 1);
    }

    #[gpui::test]
    fn resizing_live_content_keeps_identity_and_visibility(cx: &mut gpui::TestAppContext) {
        let parent = cx.add_window(|_, _| Parent);
        let popup = cx.update(|cx| {
            parent
                .update(cx, |_, window, cx| {
                    PopupWindow::open(
                        cx,
                        window,
                        options(20.),
                        |_, _| div().into_any_element(),
                        |_, _, _| {},
                    )
                })
                .unwrap()
                .unwrap()
        });
        let original_id = popup.window_id();
        cx.update(|cx| {
            parent
                .update(cx, |_, parent, cx| {
                    popup
                        .update(cx, |popup, window, cx| {
                            assert!(popup.surface.is_some());
                            popup.resize_content(parent, size(px(240.), px(250.)), window, cx);
                            assert_eq!(popup.content_size, size(px(240.), px(250.)));
                            assert_eq!(popup.bounds.size, size(px(264.), px(274.)));
                            assert_eq!(popup.revision, 0);
                            assert!(!popup.closing);
                            popup.dismiss(window, cx);
                            popup.resize_content(parent, size(px(200.), px(160.)), window, cx);
                            assert!(popup.closing);
                            assert_eq!(popup.content_size, size(px(240.), px(250.)));
                        })
                        .unwrap();
                })
                .unwrap()
        });
        assert_eq!(popup.window_id(), original_id);
        assert_eq!(cx.read(|cx| cx.windows().len()), 2);
    }

    #[gpui::test]
    fn retarget_reuses_the_window_and_replaces_content_and_hover(cx: &mut gpui::TestAppContext) {
        use std::cell::Cell;
        let parent = cx.add_window(|_, _| Parent);
        let rendered = Rc::new(Cell::new(0));
        let hovered = Rc::new(Cell::new(0));
        let first_render = rendered.clone();
        let first_hover = hovered.clone();
        let popup = cx.update(|cx| {
            parent
                .update(cx, |_, window, cx| {
                    PopupWindow::open(
                        cx,
                        window,
                        options(20.),
                        move |_, _| {
                            first_render.set(1);
                            div().child("First terminal").into_any_element()
                        },
                        move |_, _, _| first_hover.set(1),
                    )
                    .unwrap()
                })
                .unwrap()
        });
        let original_id = popup.window_id();
        cx.update(|cx| {
            popup
                .update(cx, |popup, window, cx| {
                    let _ = popup.render(window, cx);
                    (popup.on_hover.clone())(true, window, cx);
                })
                .unwrap()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        assert_eq!((rendered.get(), hovered.get()), (1, 1));

        for target in 2..=4 {
            let next_render = rendered.clone();
            let next_hover = hovered.clone();
            cx.update(|cx| {
                parent
                    .update(cx, |_, parent, cx| {
                        popup
                            .update(cx, |popup, window, cx| {
                                popup.retarget(
                                    parent,
                                    options(target as f32 * 30.),
                                    move |_, _| {
                                        next_render.set(target);
                                        div().child(format!("Terminal {target}")).into_any_element()
                                    },
                                    move |_, _, _| next_hover.set(target),
                                    window,
                                    cx,
                                );
                                let _ = popup.render(window, cx);
                                (popup.on_hover.clone())(true, window, cx);
                                assert!(!popup.closing);
                                assert_eq!(
                                    popup.revision, 0,
                                    "retarget must not replay the entrance"
                                );
                            })
                            .unwrap();
                    })
                    .unwrap()
            });
            cx.refresh().unwrap();
            cx.run_until_parked();
            assert_eq!((rendered.get(), hovered.get()), (target, target));
            assert_eq!(popup.window_id(), original_id);
            assert_eq!(cx.read(|cx| cx.windows().len()), 2);
        }

        // The original parent-close subscription must still own the reused window.
        cx.update(|cx| {
            parent
                .update(cx, |_, window, _| window.remove_window())
                .unwrap()
        });
        cx.run_until_parked();
        assert!(cx.read(|cx| cx.windows().is_empty()));
    }

    #[gpui::test]
    fn moving_and_exiting_a_retargeted_popup_clear_hover_without_a_final_mouse_move(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{Modifiers, MouseExitEvent};
        use std::cell::RefCell;
        let parent = cx.add_window(|_, _| Parent);
        let events = Rc::new(RefCell::new(Vec::new()));
        let first_events = events.clone();
        let popup = cx.update(|cx| {
            parent
                .update(cx, |_, window, cx| {
                    PopupWindow::open(
                        cx,
                        window,
                        options(20.),
                        |_, _| div().into_any_element(),
                        move |hovered, _, _| first_events.borrow_mut().push((1, hovered)),
                    )
                    .unwrap()
                })
                .unwrap()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        let mut popup_cx = gpui::VisualTestContext::from_window(*popup, cx);
        popup_cx.simulate_mouse_move(point(px(20.), px(20.)), None, Modifiers::default());
        assert_eq!(&*events.borrow(), &[(1, true)]);

        let next_events = events.clone();
        cx.update(|cx| {
            parent
                .update(cx, |_, parent, cx| {
                    popup
                        .update(cx, |popup, window, cx| {
                            popup.retarget(
                                parent,
                                options(100.),
                                |_, _| div().into_any_element(),
                                move |hovered, _, _| next_events.borrow_mut().push((2, hovered)),
                                window,
                                cx,
                            );
                        })
                        .unwrap();
                })
                .unwrap()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();

        // The pointer stays physically still while the window moves out from underneath it.
        let original = cx.update(|cx| {
            cx.update_window(*popup, |_, window, _| window.bounds().origin)
                .unwrap()
        });
        popup_cx.simulate_window_move(original + point(px(0.), px(30.)), point(px(20.), px(-10.)));
        assert_eq!(&*events.borrow(), &[(1, true), (2, false)]);
        cx.run_until_parked();
        cx.refresh().unwrap();
        cx.run_until_parked();
        popup_cx.simulate_mouse_move(point(px(20.), px(20.)), None, Modifiers::default());
        // Crossing into another native window can emit only MouseExited.
        popup_cx.simulate_event(MouseExitEvent {
            position: point(px(20.), px(20.)),
            modifiers: Modifiers::default(),
            pressed_button: None,
        });
        assert_eq!(
            &*events.borrow(),
            &[(1, true), (2, false), (2, true), (2, false)]
        );
    }

    #[gpui::test]
    fn retarget_cancels_a_pending_dismiss(cx: &mut gpui::TestAppContext) {
        let parent = cx.add_window(|_, _| Parent);
        let popup = cx.update(|cx| {
            parent
                .update(cx, |_, window, cx| {
                    PopupWindow::open(
                        cx,
                        window,
                        options(20.),
                        |_, _| div().into_any_element(),
                        |_, _, _| {},
                    )
                    .unwrap()
                })
                .unwrap()
        });
        cx.update(|cx| {
            popup
                .update(cx, |popup, window, cx| popup.dismiss(window, cx))
                .unwrap()
        });
        cx.update(|cx| {
            parent
                .update(cx, |_, parent, cx| {
                    popup
                        .update(cx, |popup, window, cx| {
                            popup.retarget(
                                parent,
                                options(100.),
                                |_, _| div().into_any_element(),
                                |_, _, _| {},
                                window,
                                cx,
                            );
                        })
                        .unwrap();
                })
                .unwrap()
        });
        cx.run_until_parked();
        cx.executor().advance_clock(OUT_DURATION * 2);
        cx.run_until_parked();
        assert!(cx.update(|cx| popup.update(cx, |popup, _, _| !popup.closing).unwrap()));
        assert_eq!(cx.read(|cx| cx.windows().len()), 2);
    }

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
        assert_eq!(placed, point(px(157.), px(200.)));
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
        assert_eq!(placed, point(px(68.), px(157.)));
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

    #[test]
    fn dropdown_surface_aligns_both_edges_with_trigger() {
        let parent = point(px(100.), px(80.));
        let anchor = Bounds::new(point(px(30.), px(20.)), size(px(180.), px(34.)));
        let extent = size(px(204.), px(144.));
        let placed = place_popup(
            parent,
            anchor,
            extent,
            Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(800.))),
            PopupPlacement::Below,
        );
        let surface_left = placed.x + px(INSET);
        let surface_right = placed.x + extent.width - px(INSET);
        let surface_top = placed.y + px(INSET);
        assert_eq!(surface_left, parent.x + anchor.left());
        assert_eq!(surface_right, parent.x + anchor.right());
        assert_eq!(surface_top - (parent.y + anchor.bottom()), px(GAP));
    }

    #[test]
    fn beside_popup_surface_aligns_top_and_keeps_gap_after_flipping() {
        let visible = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(800.)));
        let anchor = Bounds::new(point(px(30.), px(20.)), size(px(180.), px(34.)));
        let extent = size(px(204.), px(144.));
        for (parent, flipped) in [
            (point(px(100.), px(80.)), false),
            (point(px(700.), px(80.)), true),
        ] {
            let placed = place_popup(parent, anchor, extent, visible, PopupPlacement::Beside);
            assert_eq!(placed.y + px(INSET), parent.y + anchor.top());
            if flipped {
                let surface_right = placed.x + extent.width - px(INSET);
                assert_eq!(parent.x + anchor.left() - surface_right, px(GAP));
            } else {
                assert_eq!(placed.x + px(INSET) - (parent.x + anchor.right()), px(GAP));
            }
        }
    }
}
