use super::*;

#[cfg(target_os = "macos")]
fn ns_string(text: &str) -> id {
    unsafe {
        let string = NSString::alloc(nil).init_str(text);
        msg_send![string, autorelease]
    }
}

/// A document editor inside a native sheet keeps large previews out of GPUI's text layout cache.
#[cfg(target_os = "macos")]
pub(super) fn preview(
    window: &Window,
    text: &str,
    summary: &str,
    language: Language,
) -> futures::channel::oneshot::Receiver<Option<String>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    unsafe {
        let native_window = HasWindowHandle::window_handle(window)
            .ok()
            .and_then(|handle| {
                if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
                    let view = appkit.ns_view.as_ptr() as id;
                    Some(msg_send![view, window])
                } else {
                    None
                }
            })
            .unwrap_or(nil);
        if native_window == nil {
            let _ = tx.send(None);
            return rx;
        }
        let zh = language == Language::SimplifiedChinese;
        let alert: id = msg_send![class!(NSAlert), new];
        let _: () = msg_send![alert, setMessageText: ns_string(if zh { "粘贴预览" } else { "Paste preview" })];
        if !summary.is_empty() {
            let _: () = msg_send![alert, setInformativeText: ns_string(summary)];
        }
        let scroll: id = msg_send![class!(NSScrollView), alloc];
        let scroll: id = msg_send![scroll, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(480., 220.))];
        let _: () = msg_send![scroll, setHasVerticalScroller: true];
        let _: () = msg_send![scroll, setAutohidesScrollers: true];
        let _: () = msg_send![scroll, setWantsLayer: true];
        let layer: id = msg_send![scroll, layer];
        let _: () = msg_send![layer, setCornerRadius: 10.0f64];
        let _: () = msg_send![layer, setCornerCurve: ns_string("continuous")];
        let _: () = msg_send![layer, setMasksToBounds: true];
        let _: () = msg_send![layer, setBorderWidth: 1.0f64];
        let color: id = msg_send![class!(NSColor), separatorColor];
        let color: id = msg_send![color, CGColor];
        let _: () = msg_send![layer, setBorderColor: color];
        let editor: id = msg_send![class!(NSTextView), alloc];
        let editor: id = msg_send![editor, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(480., 220.))];
        let _: () = msg_send![editor, setRichText: false];
        let _: () = msg_send![editor, setAutomaticQuoteSubstitutionEnabled: false];
        let _: () = msg_send![editor, setAutomaticDashSubstitutionEnabled: false];
        let _: () = msg_send![editor, setAutomaticTextReplacementEnabled: false];
        let _: () = msg_send![editor, setAutomaticSpellingCorrectionEnabled: false];
        let _: () = msg_send![editor, setVerticallyResizable: true];
        let _: () = msg_send![editor, setHorizontallyResizable: false];
        let _: () = msg_send![editor, setTextContainerInset: NSSize::new(10., 10.)];
        let font: id =
            msg_send![class!(NSFont), monospacedSystemFontOfSize: 13.0f64 weight: 0.0f64];
        let _: () = msg_send![editor, setFont: font];
        let container: id = msg_send![editor, textContainer];
        let _: () = msg_send![container, setWidthTracksTextView: true];
        let _: () = msg_send![editor, setString: ns_string(text)];
        let _: () = msg_send![scroll, setDocumentView: editor];
        let _: () = msg_send![alert, setAccessoryView: scroll];
        let paste: id =
            msg_send![alert, addButtonWithTitle: ns_string(if zh { "粘贴" } else { "Paste" })];
        // Return remains a newline in the editor; Command-Return confirms the sheet.
        let _: () = msg_send![paste, setKeyEquivalent: ns_string("\r")];
        let _: () = msg_send![paste, setKeyEquivalentModifierMask: 1usize << 20];
        let cancel: id = msg_send![alert, addButtonWithTitle: ns_string(language.cancel())];
        let _: () = msg_send![cancel, setKeyEquivalent: ns_string("\u{1b}")];
        let sheet: id = msg_send![alert, window];
        let _: () = msg_send![sheet, setInitialFirstResponder: editor];
        let tx = std::cell::Cell::new(Some(tx));
        let block = ConcreteBlock::new(move |response: NSInteger| {
            let result = if response == 1000 {
                let value: id = msg_send![editor, string];
                let bytes: *const std::os::raw::c_char = msg_send![value, UTF8String];
                let length: usize = msg_send![value, lengthOfBytesUsingEncoding: 4usize];
                if bytes.is_null() {
                    None
                } else {
                    Some(
                        String::from_utf8_lossy(std::slice::from_raw_parts(bytes.cast(), length))
                            .into_owned(),
                    )
                }
            } else {
                None
            };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
            let _: () = msg_send![alert, release];
        })
        .copy();
        let _: () =
            msg_send![alert, beginSheetModalForWindow: native_window completionHandler: block];
        let _: () = msg_send![sheet, makeFirstResponder: editor];
        let _: () = msg_send![editor, release];
        let _: () = msg_send![scroll, release];
    }
    rx
}

#[cfg(not(target_os = "macos"))]
pub(super) fn preview(
    _window: &Window,
    _text: &str,
    _summary: &str,
    _language: Language,
) -> futures::channel::oneshot::Receiver<Option<String>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let _ = tx.send(None);
    rx
}
