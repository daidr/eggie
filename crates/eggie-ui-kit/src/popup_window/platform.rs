//! Window-server transitions; content has no per-frame visibility animation.

#[cfg(all(target_os = "macos", not(any(test, feature = "test-support"))))]
#[path = "macos.rs"]
mod implementation;

#[cfg(all(target_os = "macos", not(any(test, feature = "test-support"))))]
pub(super) const NATIVE: bool = true;
#[cfg(any(not(target_os = "macos"), test, feature = "test-support"))]
pub(super) const NATIVE: bool = false;

pub(super) use implementation::PopupSurface;

// Other backends retain an ordinary window until they provide native transitions.
// Tests deliver completion with their virtual clock instead of calling AppKit.
#[cfg(any(not(target_os = "macos"), test, feature = "test-support"))]
mod implementation {
    use futures::channel::oneshot;
    use gpui::{App, Bounds, Pixels, Window};
    use std::time::Duration;

    pub(crate) struct PopupSurface;

    impl PopupSurface {
        pub(crate) fn new(_: &Window) -> anyhow::Result<Self> {
            Ok(Self)
        }

        pub(crate) fn transition(
            &mut self,
            _: Bounds<Pixels>,
            _: f64,
            duration: Duration,
            cx: &App,
        ) -> oneshot::Receiver<()> {
            let (done, completed) = oneshot::channel();
            let executor = cx.background_executor().clone();
            cx.spawn(async move |_| {
                executor.timer(duration).await;
                let _ = done.send(());
            })
            .detach();
            completed
        }
    }
}
