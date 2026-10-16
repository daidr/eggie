//! Small, non-activating windows for menus, popovers, and tooltips.
//!
//! GPUI's parent-anchored popup is not implemented on macOS yet. This shell keeps placement,
//! window lifetime, and motion independent of the content rendered inside it.

use crate::continuous_corners::div;
use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, Context, Pixels, Point, Render, Size,
    Subscription, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, point, prelude::*, px, rgb, size,
};
use std::{
    rc::Rc,
    time::{Duration, Instant},
};

const IN_DURATION: Duration = Duration::from_millis(180);
const OUT_DURATION: Duration = Duration::from_millis(160);
const MOVE_DURATION: Duration = Duration::from_millis(150);
const INSET: f32 = 12.;
const GAP: f32 = 5.;

type Content = Rc<dyn Fn(&mut Window, &mut Context<PopupWindow>) -> AnyElement>;
type Hover = Rc<dyn Fn(bool, &mut Window, &mut App)>;

#[derive(Clone, Copy)]
struct PopupVisibility {
    from: f32,
    to: f32,
    started_at: Instant,
    duration: Duration,
}
impl PopupVisibility {
    fn value(self, now: Instant) -> f32 {
        let progress = (now.saturating_duration_since(self.started_at).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0., 1.);
        let eased = if self.to > self.from {
            gpui::ease_out_quint()(progress)
        } else {
            gpui::ease_in_out(progress)
        };
        self.from + (self.to - self.from) * eased
    }
}

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

#[derive(Clone, Copy)]
struct PopupMotion {
    from: Point<Pixels>,
    to: Point<Pixels>,
    started_at: Instant,
}

impl PopupMotion {
    fn progress(self, now: Instant) -> f32 {
        (now.saturating_duration_since(self.started_at).as_secs_f32() / MOVE_DURATION.as_secs_f32())
            .clamp(0., 1.)
    }

    fn origin(self, now: Instant) -> Point<Pixels> {
        self.from + (self.to - self.from) * gpui::ease_out_quint()(self.progress(now))
    }
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
    visibility: PopupVisibility,
    pointer_pressed: bool,
    revision: u64,
    motion: Option<PopupMotion>,
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
                    visibility: PopupVisibility {
                        from: 0.,
                        to: 1.,
                        started_at: Instant::now(),
                        duration: IN_DURATION,
                    },
                    pointer_pressed: false,
                    revision: 0,
                    motion: None,
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
        if window.bounds().size != bounds.size {
            window.resize(bounds.size);
        }
        if cx.reduce_motion() {
            self.motion = None;
            set_popup_origin(window, bounds.origin, cx);
        } else {
            // Interrupt from the painted position so rapid reversals stay continuous.
            self.motion = Some(PopupMotion {
                from: window.bounds().origin,
                to: bounds.origin,
                started_at: Instant::now(),
            });
        }
        cx.notify();
    }

    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        if cx.reduce_motion() {
            window.remove_window();
            return;
        }
        self.revision += 1;
        let now = Instant::now();
        self.visibility = PopupVisibility {
            from: self.visibility.value(now),
            to: 0.,
            started_at: now,
            duration: OUT_DURATION,
        };
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

    pub fn restore(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            self.closing = false;
            let now = Instant::now();
            self.visibility = PopupVisibility {
                from: self.visibility.value(now),
                to: 1.,
                started_at: now,
                duration: IN_DURATION,
            };
            self.revision += 1;
            cx.notify();
        }
    }
    pub fn pointer_pressed(&self) -> bool {
        self.pointer_pressed
    }
}

impl Render for PopupWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(motion) = self.motion {
            let now = Instant::now();
            set_popup_origin(window, motion.origin(now), cx);
            if motion.progress(now) < 1. {
                let entity = cx.entity_id();
                window.on_next_frame(move |_, cx| cx.notify(entity));
            } else {
                self.motion = None;
            }
        }
        let content = (self.content.clone())(window, cx);
        let visibility = self.visibility;
        let revision = self.revision;
        let hover = self.on_hover.clone();
        let release_outside = self.on_hover.clone();
        let reduced_motion = cx.reduce_motion();
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
                    .with_animation(
                        format!("popup-window-{revision}"),
                        Animation::new(visibility.duration),
                        move |surface, progress| {
                            let visible = if reduced_motion {
                                visibility.to
                            } else if progress >= 1. {
                                visibility.to
                            } else {
                                visibility.value(Instant::now())
                            };
                            surface
                                .opacity(visible)
                                .ml(px(INSET))
                                .mt(px(INSET + (1. - visible) * 6.))
                        },
                    ),
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

// GPUI exposes resizing but no positioning API; keep the AppKit bridge in the popup shell.
#[cfg(all(target_os = "macos", not(test)))]
fn set_popup_origin(window: &mut Window, origin: Point<Pixels>, cx: &App) {
    use cocoa::{
        base::{id, nil},
        foundation::NSPoint,
    };
    use objc::{msg_send, rc::StrongPtr, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let delta = origin - window.bounds().origin;
    if delta.x.abs() < px(0.01) && delta.y.abs() < px(0.01) {
        return;
    }
    let native_window: id = match HasWindowHandle::window_handle(window) {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::AppKit(appkit) => unsafe {
                msg_send![appkit.ns_view.as_ptr() as id, window]
            },
            _ => return,
        },
        Err(_) => return,
    };
    if native_window == nil {
        return;
    }
    unsafe {
        let frame: cocoa::foundation::NSRect = msg_send![native_window, frame];
        let origin = NSPoint::new(
            frame.origin.x + delta.x.as_f32() as f64,
            frame.origin.y - delta.y.as_f32() as f64,
        );
        let native_window = StrongPtr::retain(native_window);
        // AppKit sends moved callbacks synchronously, so apply the frame after GPUI releases App.
        cx.foreground_executor()
            .spawn(async move {
                let _: () = msg_send![*native_window, setFrameOrigin: origin];
            })
            .detach();
    }
}

#[cfg(any(not(target_os = "macos"), test))]
fn set_popup_origin(_window: &mut Window, _origin: Point<Pixels>, _cx: &App) {}

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

    #[test]
    fn reversing_popup_visibility_keeps_opacity_and_translation_continuous() {
        let start = Instant::now();
        let entering = PopupVisibility {
            from: 0.,
            to: 1.,
            started_at: start,
            duration: IN_DURATION,
        };
        let dismissed = start + IN_DURATION / 3;
        let exiting = PopupVisibility {
            from: entering.value(dismissed),
            to: 0.,
            started_at: dismissed,
            duration: OUT_DURATION,
        };
        assert_eq!(entering.value(dismissed), exiting.value(dismissed));
        let middle = dismissed + OUT_DURATION / 2;
        assert!(exiting.value(middle) < exiting.from && exiting.value(middle) > 0.);
        let restored = PopupVisibility {
            from: exiting.value(middle),
            to: 1.,
            started_at: middle,
            duration: IN_DURATION,
        };
        assert_eq!(exiting.value(middle), restored.value(middle));
        assert_eq!(exiting.value(dismissed + OUT_DURATION), 0.);
        assert_eq!(restored.value(middle + IN_DURATION), 1.);
    }

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
                            let visibility = popup.visibility;
                            popup.resize_content(parent, size(px(240.), px(250.)), window, cx);
                            assert_eq!(popup.content_size, size(px(240.), px(250.)));
                            assert_eq!(popup.visibility.started_at, visibility.started_at);
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
    fn interrupted_motion_starts_at_the_visible_position_and_settles_without_overshoot() {
        let now = Instant::now();
        let first = PopupMotion {
            from: point(px(50.), px(100.)),
            to: point(px(50.), px(250.)),
            started_at: now,
        };
        let interrupted_at = now + MOVE_DURATION / 3;
        let painted = first.origin(interrupted_at);
        assert!(painted.y > first.from.y && painted.y < first.to.y);
        let reversed = PopupMotion {
            from: painted,
            to: first.from,
            started_at: interrupted_at,
        };
        assert_eq!(reversed.origin(interrupted_at), painted);
        let mut previous = painted.y;
        for elapsed in 1..=15 {
            let current = reversed.origin(interrupted_at + Duration::from_millis(elapsed * 10));
            assert!(current.y <= previous && current.y >= reversed.to.y);
            previous = current.y;
        }
        assert_eq!(reversed.origin(interrupted_at + MOVE_DURATION), reversed.to);
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
