use block::ConcreteBlock;
use cocoa::{
    base::{NO, YES, id, nil},
    foundation::{NSPoint, NSRect, NSSize, NSString},
};
use futures::channel::oneshot;
use gpui::{App, Bounds, Pixels, Window};
use objc::{class, msg_send, rc::StrongPtr, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{cell::Cell, rc::Rc, time::Duration};

const NS_WINDOW_ANIMATION_BEHAVIOR_NONE: isize = 2;

struct NativeWindow {
    window: StrongPtr,
    screen_left: f64,
    screen_top: f64,
    alive: Cell<bool>,
    ordered: Cell<bool>,
    revision: Cell<u64>,
}

impl NativeWindow {
    fn frame(&self, bounds: Bounds<Pixels>) -> NSRect {
        let width = bounds.size.width.as_f32() as f64;
        let height = bounds.size.height.as_f32() as f64;
        NSRect::new(
            NSPoint::new(
                self.screen_left + bounds.origin.x.as_f32() as f64,
                self.screen_top - bounds.origin.y.as_f32() as f64 - height,
            ),
            NSSize::new(width, height),
        )
    }
}

pub(crate) struct PopupSurface(Rc<NativeWindow>);

impl Drop for PopupSurface {
    fn drop(&mut self) {
        // Queued AppKit work must never order a window onscreen after its GPUI owner was removed.
        self.0.alive.set(false);
    }
}

impl PopupSurface {
    pub(crate) fn new(window: &Window) -> anyhow::Result<Self> {
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("popup window handle: {error}"))?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            anyhow::bail!("popup requires an AppKit window");
        };
        unsafe {
            let native: id = msg_send![handle.ns_view.as_ptr() as id, window];
            anyhow::ensure!(native != nil, "popup has no native window");
            let frame: NSRect = msg_send![native, frame];
            let origin = window.bounds().origin;
            Ok(Self(Rc::new(NativeWindow {
                window: StrongPtr::retain(native),
                screen_left: frame.origin.x - origin.x.as_f32() as f64,
                screen_top: frame.origin.y + frame.size.height + origin.y.as_f32() as f64,
                alive: Cell::new(true),
                ordered: Cell::new(false),
                revision: Cell::new(0),
            })))
        }
    }

    pub(crate) fn transition(
        &mut self,
        bounds: Bounds<Pixels>,
        opacity: f64,
        duration: Duration,
        cx: &App,
    ) -> oneshot::Receiver<()> {
        let (done, completed) = oneshot::channel();
        let native = self.0.clone();
        let revision = native.revision.get().wrapping_add(1);
        native.revision.set(revision);
        // Frame changes synchronously invoke AppKit delegates; run them after GPUI releases App.
        cx.foreground_executor()
            .spawn(async move {
                if !native.alive.get() || native.revision.get() != revision {
                    return;
                }
                unsafe {
                    let window = *native.window;
                    let target = native.frame(bounds);
                    if !native.ordered.get() {
                        if opacity == 0. {
                            let _ = done.send(());
                            return;
                        }
                        // Disable AppKit's automatic popup effect; this context owns the transition.
                        let _: () = msg_send![window, setAnimationBehavior: NS_WINDOW_ANIMATION_BEHAVIOR_NONE];
                        let _: () = msg_send![window, setAlphaValue: 0f64];
                        let mut start = target;
                        if !duration.is_zero() {
                            start.origin.y -= 6.;
                        }
                        let _: () = msg_send![window, setFrame: start display: YES];
                        let _: () = msg_send![window, orderFront: nil];
                        native.ordered.set(true);
                    } else {
                        let mut current: NSRect = msg_send![window, frame];
                        if current.size.width != target.size.width
                            || current.size.height != target.size.height
                        {
                            current.origin.y += current.size.height - target.size.height;
                            current.size = target.size;
                            // Reflow content immediately, keeping its current top edge. Only the
                            // whole window's position and opacity animate between targets.
                            let _: () = msg_send![class!(NSAnimationContext), beginGrouping];
                            let context: id = msg_send![class!(NSAnimationContext), currentContext];
                            let _: () = msg_send![context, setDuration: 0f64];
                            let animator: id = msg_send![window, animator];
                            let _: () = msg_send![animator, setFrame: current display: YES];
                            let _: () = msg_send![class!(NSAnimationContext), endGrouping];
                        }
                    }

                    let done = Cell::new(Some(done));
                    let completion = ConcreteBlock::new(move || {
                        if let Some(done) = done.take() {
                            let _ = done.send(());
                        }
                    })
                    .copy();
                    let _: () = msg_send![class!(NSAnimationContext), beginGrouping];
                    let context: id = msg_send![class!(NSAnimationContext), currentContext];
                    let _: () = msg_send![context, setDuration: duration.as_secs_f64()];
                    let name = StrongPtr::new(NSString::alloc(nil).init_str("easeOut"));
                    let timing: id =
                        msg_send![class!(CAMediaTimingFunction), functionWithName: *name];
                    let _: () = msg_send![context, setTimingFunction: timing];
                    let _: () = msg_send![context, setCompletionHandler: &*completion];
                    // AppKit retargets an in-flight property from its current presented value.
                    let animator: id = msg_send![window, animator];
                    let _: () = msg_send![animator, setAlphaValue: opacity];
                    let _: () = msg_send![animator, setFrame: target display: NO];
                    let _: () = msg_send![class!(NSAnimationContext), endGrouping];
                }
            })
            .detach();
        completed
    }
}
