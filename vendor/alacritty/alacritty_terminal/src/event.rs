use std::borrow::Cow;
use std::fmt::{self, Debug, Formatter};
use std::process::ExitStatus;
use std::sync::Arc;

use crate::term::ClipboardType;
use crate::vte::ansi::{DesktopNotification, ProgressReport, Rgb, SemanticPrompt};

/// Terminal event.
///
/// These events instruct the UI over changes that can't be handled by the terminal emulation layer
/// itself.
#[derive(Clone)]
pub enum Event {
    /// Grid has changed possibly requiring a mouse cursor shape change.
    MouseCursorDirty,

    /// Window title change.
    Title(String),

    /// Reset to the default window title.
    ResetTitle,

    /// Terminal task progress changed through OSC 9;4.
    ProgressReport(Option<ProgressReport>),

    /// Shell working directory reported by OSC 7, OSC 9;9, or OSC 1337 CurrentDir.
    WorkingDirectory(String),

    /// Semantic shell-integration marker reported by OSC 133.
    ///
    /// The second field is the grid line the cursor was on when the marker was emitted (relative to
    /// the top of the visible screen; may be negative if in scrollback). Eggie records this so a
    /// resize can clear exactly the active prompt region instead of guessing its extent.
    ///
    /// The third field is the scrollback `history_size` at emit time, captured synchronously here
    /// because the parser holds the terminal lock. Eggie combines it with the cursor line to record
    /// a scroll-stable coordinate for jump-to-prompt, which cannot be recovered later from the
    /// throttled snapshot path.
    SemanticPrompt(SemanticPrompt, i32, usize),

    /// Desktop notification command reported by OSC 9, OSC 99, or OSC 777.
    DesktopNotification(DesktopNotification),

    /// Raw iTerm2 OSC 1337 command and its original terminator.
    Iterm2Command(String, String),

    /// Raw Kitty OSC 5522 rich clipboard command and its original terminator.
    KittyClipboard(String, String),

    /// Raw Kitty OSC 5113 file-transfer command and its original terminator.
    KittyFileTransfer(String, String),

    /// Request to store a text string in the clipboard.
    ClipboardStore(ClipboardType, String),

    /// Request to write the contents of the clipboard to the PTY.
    ///
    /// The attached function is a formatter which will correctly transform the clipboard content
    /// into the expected escape sequence format.
    ClipboardLoad(
        ClipboardType,
        Arc<dyn Fn(&str) -> String + Sync + Send + 'static>,
    ),

    /// Request to write the RGB value of a color to the PTY.
    ///
    /// The attached function is a formatter which will correctly transform the RGB color into the
    /// expected escape sequence format.
    ColorRequest(
        usize,
        Option<Rgb>,
        Arc<dyn Fn(Rgb) -> String + Sync + Send + 'static>,
    ),

    /// Write some text to the PTY.
    PtyWrite(String),

    /// Request to write the text area size.
    TextAreaSizeRequest(Arc<dyn Fn(WindowSize) -> String + Sync + Send + 'static>),

    /// Cursor blinking state has changed.
    CursorBlinkingChange,

    /// New terminal content available.
    Wakeup,

    /// Terminal bell ring.
    Bell,

    /// Shutdown request.
    Exit,

    /// Child process exited.
    ChildExit(ExitStatus),
}

impl Debug for Event {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Event::ClipboardStore(ty, text) => write!(f, "ClipboardStore({ty:?}, {text})"),
            Event::ClipboardLoad(ty, _) => write!(f, "ClipboardLoad({ty:?})"),
            Event::TextAreaSizeRequest(_) => write!(f, "TextAreaSizeRequest"),
            Event::ColorRequest(index, color, _) => {
                write!(f, "ColorRequest({index}, {color:?})")
            }
            Event::PtyWrite(text) => write!(f, "PtyWrite({text})"),
            Event::Title(title) => write!(f, "Title({title})"),
            Event::CursorBlinkingChange => write!(f, "CursorBlinkingChange"),
            Event::MouseCursorDirty => write!(f, "MouseCursorDirty"),
            Event::ResetTitle => write!(f, "ResetTitle"),
            Event::ProgressReport(progress) => write!(f, "ProgressReport({progress:?})"),
            Event::WorkingDirectory(directory) => write!(f, "WorkingDirectory({directory:?})"),
            Event::SemanticPrompt(prompt, line, history_size) => {
                write!(f, "SemanticPrompt({prompt:?}, {line}, {history_size})")
            }
            Event::DesktopNotification(notification) => {
                write!(f, "DesktopNotification({notification:?})")
            }
            Event::Iterm2Command(payload, _) => write!(f, "Iterm2Command({payload:?})"),
            Event::KittyClipboard(payload, _) => write!(f, "KittyClipboard({payload:?})"),
            Event::KittyFileTransfer(payload, _) => {
                write!(f, "KittyFileTransfer({payload:?})")
            }
            Event::Wakeup => write!(f, "Wakeup"),
            Event::Bell => write!(f, "Bell"),
            Event::Exit => write!(f, "Exit"),
            Event::ChildExit(status) => write!(f, "ChildExit({status:?})"),
        }
    }
}

/// Byte sequences are sent to a `Notify` in response to some events.
pub trait Notify {
    /// Notify that an escape sequence should be written to the PTY.
    ///
    /// TODO this needs to be able to error somehow.
    fn notify<B: Into<Cow<'static, [u8]>>>(&self, _: B);
}

#[derive(Copy, Clone, Debug)]
pub struct WindowSize {
    pub num_lines: u16,
    pub num_cols: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

/// Types that are interested in when the display is resized.
pub trait OnResize {
    fn on_resize(&mut self, window_size: WindowSize);
}

/// Event Loop for notifying the renderer about terminal events.
pub trait EventListener {
    fn send_event(&self, _event: Event) {}
}

/// Null sink for events.
pub struct VoidListener;

impl EventListener for VoidListener {}
