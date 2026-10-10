use super::*;

/// Show a native macOS text-input alert and return the entered string, or `None` if cancelled.
///
/// Uses `beginSheetModalForWindow` (asynchronous) so it does not block GPUI's event loop or
/// re-enter the app's `RefCell` borrow while a mouse event is being dispatched.
#[cfg(target_os = "macos")]
pub(super) fn prompt_for_text(
    window: &Window,
    title: &str,
    message: &str,
    placeholder: Option<&str>,
    initial: Option<&str>,
    ok_label: &str,
    cancel_label: &str,
) -> futures::channel::oneshot::Receiver<Option<String>> {
    let (tx, rx) = futures::channel::oneshot::channel();

    unsafe {
        let alert: id = msg_send![class!(NSAlert), alloc];
        let alert: id = msg_send![alert, init];
        let _: () = msg_send![
            alert,
            setMessageText: NSString::alloc(nil).init_str(title)
        ];
        let _: () = msg_send![
            alert,
            setInformativeText: NSString::alloc(nil).init_str(message)
        ];

        let text_field: id = msg_send![class!(NSTextField), alloc];
        let text_field: id = msg_send![
            text_field,
            initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(260., 24.))
        ];
        if let Some(placeholder) = placeholder {
            let _: () = msg_send![
                text_field,
                setPlaceholderString: NSString::alloc(nil).init_str(placeholder)
            ];
        }
        if let Some(initial) = initial {
            let _: () = msg_send![
                text_field,
                setStringValue: NSString::alloc(nil).init_str(initial)
            ];
        }
        let _: () = msg_send![alert, setAccessoryView: text_field];

        let _: id = msg_send![alert, addButtonWithTitle: NSString::alloc(nil).init_str(ok_label)];
        let cancel_button: id =
            msg_send![alert, addButtonWithTitle: NSString::alloc(nil).init_str(cancel_label)];
        let _: () = msg_send![
            cancel_button,
            setKeyEquivalent: NSString::alloc(nil).init_str("\u{1b}")
        ];

        let native_window = match HasWindowHandle::window_handle(window) {
            Ok(handle) => match handle.as_raw() {
                RawWindowHandle::AppKit(appkit) => {
                    let ns_view = appkit.ns_view.as_ptr() as id;
                    if ns_view == nil {
                        nil
                    } else {
                        msg_send![ns_view, window]
                    }
                }
                _ => nil,
            },
            Err(_) => nil,
        };

        if native_window == nil {
            let _ = tx.send(None);
            return rx;
        }

        let tx = std::cell::Cell::new(Some(tx));
        let block = ConcreteBlock::new(move |response: NSInteger| {
            let result = if response == 1000 {
                let value: id = msg_send![text_field, stringValue];
                let bytes: *const std::os::raw::c_char = msg_send![value, UTF8String];
                if bytes.is_null() {
                    None
                } else {
                    Some(
                        std::ffi::CStr::from_ptr(bytes)
                            .to_string_lossy()
                            .into_owned(),
                    )
                }
            } else {
                None
            };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });
        let block = block.copy();

        let _: () = msg_send![
            alert,
            beginSheetModalForWindow: native_window
            completionHandler: block
        ];
    }

    rx
}

#[cfg(not(target_os = "macos"))]
pub(super) fn prompt_for_text(
    _window: &Window,
    _title: &str,
    _message: &str,
    _placeholder: Option<&str>,
    _initial: Option<&str>,
    _ok_label: &str,
    _cancel_label: &str,
) -> futures::channel::oneshot::Receiver<Option<String>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let _ = tx.send(None);
    rx
}

/// A native snippet editor prompt: a single-line name field, a multi-line content editor, and an
/// "auto-run" checkbox (when checked, Eggie sends a real Enter after injecting so the command runs;
/// unchecked just types it in). A native dialog gives multi-line editing real space and avoids
/// squeezing the narrow right sidebar. Resolves Some((name, content, auto_run)) on OK, None otherwise.
#[cfg(target_os = "macos")]
pub(super) fn prompt_for_snippet(
    window: &Window,
    title: &str,
    name_seed: &str,
    content_seed: &str,
    auto_run_seed: bool,
    name_placeholder: &str,
    content_placeholder: &str,
    auto_run_label: &str,
    ok_label: &str,
    cancel_label: &str,
) -> futures::channel::oneshot::Receiver<Option<(String, String, bool)>> {
    let (tx, rx) = futures::channel::oneshot::channel();

    unsafe {
        let alert: id = msg_send![class!(NSAlert), alloc];
        let alert: id = msg_send![alert, init];
        let _: () = msg_send![alert, setMessageText: NSString::alloc(nil).init_str(title)];

        const WIDTH: f64 = 320.;
        const NAME_H: f64 = 24.;
        const CONTENT_H: f64 = 120.;
        const CHECK_H: f64 = 20.;
        const GAP: f64 = 8.;
        let total_h = NAME_H + GAP + CONTENT_H + GAP + CHECK_H;
        let container: id = msg_send![class!(NSView), alloc];
        let container: id = msg_send![
            container,
            initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(WIDTH, total_h))
        ];

        // Layout is bottom-up (AppKit y grows upward): checkbox at the bottom, then content, then name.
        let check_y = 0.;
        let content_y = CHECK_H + GAP;
        let name_y = content_y + CONTENT_H + GAP;

        // Name field (single line) at the top.
        let name_field: id = msg_send![class!(NSTextField), alloc];
        let name_field: id = msg_send![
            name_field,
            initWithFrame: NSRect::new(NSPoint::new(0., name_y), NSSize::new(WIDTH, NAME_H))
        ];
        let _: () = msg_send![name_field, setStringValue: NSString::alloc(nil).init_str(name_seed)];
        let _: () = msg_send![
            name_field,
            setPlaceholderString: NSString::alloc(nil).init_str(name_placeholder)
        ];
        let _: () = msg_send![container, addSubview: name_field];

        // Content editor: NSTextView inside an NSScrollView.
        let scroll: id = msg_send![class!(NSScrollView), alloc];
        let scroll: id = msg_send![
            scroll,
            initWithFrame: NSRect::new(NSPoint::new(0., content_y), NSSize::new(WIDTH, CONTENT_H))
        ];
        let _: () = msg_send![scroll, setHasVerticalScroller: true];
        // Rounded, hairline-bordered container to match modern macOS text fields. NSScrollView
        // border types have no rounded option, so drop the built-in border and round the layer.
        let _: () = msg_send![scroll, setBorderType: 0 as NSInteger]; // NSNoBorder
        let _: () = msg_send![scroll, setWantsLayer: true];
        let scroll_layer: id = msg_send![scroll, layer];
        if scroll_layer != nil {
            let _: () = msg_send![scroll_layer, setCornerRadius: 6.0f64];
            let _: () = msg_send![scroll_layer, setMasksToBounds: true];
            let _: () = msg_send![scroll_layer, setBorderWidth: 1.0f64];
            let separator: id = msg_send![class!(NSColor), separatorColor];
            let border_cg: id = msg_send![separator, CGColor];
            let _: () = msg_send![scroll_layer, setBorderColor: border_cg];
        }
        let text_view: id = msg_send![class!(NSTextView), alloc];
        let text_view: id = msg_send![
            text_view,
            initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(WIDTH, CONTENT_H))
        ];
        let _: () = msg_send![text_view, setRichText: false];
        let _: () = msg_send![text_view, setString: NSString::alloc(nil).init_str(content_seed)];
        let _: () = msg_send![scroll, setDocumentView: text_view];
        let _: () = msg_send![container, addSubview: scroll];

        // Auto-run checkbox at the bottom. NSButton checkbox = buttonWithTitle then setButtonType:switch.
        let checkbox: id = msg_send![class!(NSButton), alloc];
        let checkbox: id = msg_send![
            checkbox,
            initWithFrame: NSRect::new(NSPoint::new(0., check_y), NSSize::new(WIDTH, CHECK_H))
        ];
        let _: () = msg_send![checkbox, setButtonType: 3 as NSInteger]; // NSButtonTypeSwitch
        let _: () = msg_send![checkbox, setTitle: NSString::alloc(nil).init_str(auto_run_label)];
        let _: () = msg_send![checkbox, setState: (if auto_run_seed { 1 } else { 0 }) as NSInteger];
        let _: () = msg_send![container, addSubview: checkbox];

        // NSTextView has no native placeholder; surface the content hint as the dialog body only
        // when creating a new snippet (empty content), so an editing dialog is not cluttered.
        if content_seed.is_empty() {
            let _: () = msg_send![
                alert,
                setInformativeText: NSString::alloc(nil).init_str(content_placeholder)
            ];
        }
        let _: () = msg_send![alert, setAccessoryView: container];

        let _: id = msg_send![alert, addButtonWithTitle: NSString::alloc(nil).init_str(ok_label)];
        let cancel_button: id =
            msg_send![alert, addButtonWithTitle: NSString::alloc(nil).init_str(cancel_label)];
        let _: () = msg_send![
            cancel_button,
            setKeyEquivalent: NSString::alloc(nil).init_str("\u{1b}")
        ];

        let native_window = match HasWindowHandle::window_handle(window) {
            Ok(handle) => match handle.as_raw() {
                RawWindowHandle::AppKit(appkit) => {
                    let ns_view = appkit.ns_view.as_ptr() as id;
                    if ns_view == nil {
                        nil
                    } else {
                        msg_send![ns_view, window]
                    }
                }
                _ => nil,
            },
            Err(_) => nil,
        };

        if native_window == nil {
            let _ = tx.send(None);
            return rx;
        }

        let tx = std::cell::Cell::new(Some(tx));
        let block = ConcreteBlock::new(move |response: NSInteger| {
            let result = if response == 1000 {
                let name_value: id = msg_send![name_field, stringValue];
                let name_bytes: *const std::os::raw::c_char = msg_send![name_value, UTF8String];
                let name = if name_bytes.is_null() {
                    String::new()
                } else {
                    std::ffi::CStr::from_ptr(name_bytes)
                        .to_string_lossy()
                        .into_owned()
                };
                let content_value: id = msg_send![text_view, string];
                let content_bytes: *const std::os::raw::c_char =
                    msg_send![content_value, UTF8String];
                let content = if content_bytes.is_null() {
                    String::new()
                } else {
                    std::ffi::CStr::from_ptr(content_bytes)
                        .to_string_lossy()
                        .into_owned()
                };
                let state: NSInteger = msg_send![checkbox, state];
                Some((name, content, state != 0))
            } else {
                None
            };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });
        let block = block.copy();

        let _: () = msg_send![
            alert,
            beginSheetModalForWindow: native_window
            completionHandler: block
        ];
    }

    rx
}

#[cfg(not(target_os = "macos"))]
pub(super) fn prompt_for_snippet(
    _window: &Window,
    _title: &str,
    _name_seed: &str,
    _content_seed: &str,
    _auto_run_seed: bool,
    _name_placeholder: &str,
    _content_placeholder: &str,
    _auto_run_label: &str,
    _ok_label: &str,
    _cancel_label: &str,
) -> futures::channel::oneshot::Receiver<Option<(String, String, bool)>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let _ = tx.send(None);
    rx
}
