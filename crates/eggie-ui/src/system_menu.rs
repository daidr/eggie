//! System-level application-menu helpers that GPUI does not expose: the standard
//! macOS About panel and Secure Keyboard Entry. Kept separate from `native_menu`
//! (which builds right-click context menus) because these drive the top menu bar.
//!
//! Non-macOS builds get inert stubs so the menu wiring compiles everywhere.

/// The next Secure Keyboard Entry state given the current one — a trivial toggle,
/// factored out so it can be unit-tested without touching the real Carbon FFI
/// (which reference-counts a process-global input mode and would leak across tests).
#[cfg(any(target_os = "macos", test))]
fn next_secure_state(current: bool) -> bool {
    !current
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cocoa::{
        appkit::NSApp,
        base::{id, nil},
        foundation::NSString,
    };
    use objc::{class, msg_send, sel, sel_impl};

    // `SecureEventInput` lives in the Carbon framework. It is reference-counted per
    // process: each Enable must be balanced by a Disable. We track our own single
    // toggle with an atomic and never call Enable/Disable more than once out of turn.
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn EnableSecureEventInput();
        fn DisableSecureEventInput();
    }

    /// Whether *we* have secure input enabled. Seeds the menu checkmark.
    static SECURE_INPUT_ON: AtomicBool = AtomicBool::new(false);

    /// Show the standard macOS About panel. Because Eggie ships without an
    /// `Info.plist`, the panel would otherwise display the bare executable name and
    /// an empty version, so we pass an options dictionary with an explicit name and
    /// the crate version.
    pub(crate) fn show_about_panel() {
        unsafe {
            let app = NSApp();
            let _: () = msg_send![app, activateIgnoringOtherApps: true];

            let name_key = NSString::alloc(nil).init_str("ApplicationName");
            let name_val = NSString::alloc(nil).init_str("Eggie");
            let version_key = NSString::alloc(nil).init_str("ApplicationVersion");
            let version_val = NSString::alloc(nil).init_str(env!("CARGO_PKG_VERSION"));

            let keys: [id; 2] = [name_key, version_key];
            let values: [id; 2] = [name_val, version_val];
            let options: id = msg_send![
                class!(NSDictionary),
                dictionaryWithObjects: values.as_ptr()
                forKeys: keys.as_ptr()
                count: 2usize
            ];

            let _: () = msg_send![app, orderFrontStandardAboutPanelWithOptions: options];

            let _: () = msg_send![name_key, release];
            let _: () = msg_send![name_val, release];
            let _: () = msg_send![version_key, release];
            let _: () = msg_send![version_val, release];
        }
    }

    /// Flip Secure Keyboard Entry and return the new state (for the menu checkmark).
    pub(crate) fn toggle_secure_input() -> bool {
        let next = super::next_secure_state(SECURE_INPUT_ON.load(Ordering::SeqCst));
        unsafe {
            if next {
                EnableSecureEventInput();
            } else {
                DisableSecureEventInput();
            }
        }
        SECURE_INPUT_ON.store(next, Ordering::SeqCst);
        next
    }

    /// Whether Secure Keyboard Entry is currently on (drives the menu checkmark).
    pub(crate) fn secure_input_enabled() -> bool {
        SECURE_INPUT_ON.load(Ordering::SeqCst)
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub(crate) fn show_about_panel() {}
    pub(crate) fn toggle_secure_input() -> bool {
        false
    }
    pub(crate) fn secure_input_enabled() -> bool {
        false
    }
}

pub(crate) use imp::{secure_input_enabled, show_about_panel, toggle_secure_input};

#[cfg(test)]
mod tests {
    use super::next_secure_state;

    #[test]
    fn next_secure_state_flips() {
        assert!(next_secure_state(false));
        assert!(!next_secure_state(true));
    }
}
