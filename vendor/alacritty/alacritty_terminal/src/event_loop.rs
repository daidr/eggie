//! The main event loop which performs I/O on the pseudoterminal.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use log::error;
use polling::{Event as PollingEvent, Events, PollMode, Poller};

use crate::event::{self, Event, EventListener, WindowSize};
use crate::pipeline_metrics::{self, Stage as PipelineStage};
use crate::sync::FairMutex;
use crate::term::Term;
use crate::term::kitty_graphics::{ApcOutput, ApcSplitter};
use crate::{thread, tty};
use vte::ansi;

/// Max bytes to read from the PTY before forced terminal synchronization.
pub(crate) const READ_BUFFER_SIZE: usize = 0x10_0000;

/// Maximum PTY bytes staged ahead of the parser.
///
/// The bounded queue absorbs short image bursts without allowing a producer to consume unbounded
/// memory. Four 1 MiB blocks are enough to keep the small kernel PTY buffer drained while the
/// parser is decoding a large Kitty frame.
const PARSER_QUEUE_DEPTH: usize = 4;

/// Preserve write/resize responsiveness even when a producer can continuously fill the PTY.
const MAX_PTY_DRAIN: usize = READ_BUFFER_SIZE * PARSER_QUEUE_DEPTH;

/// Messages that may be sent to the `EventLoop`.
#[derive(Debug)]
pub enum Msg {
    /// Data that should be written to the PTY.
    Input(Cow<'static, [u8]>),

    /// Indicates that the `EventLoop` should shut down, as Alacritty is shutting down.
    Shutdown,

    /// Instruction to resize the PTY.
    Resize(WindowSize),
}

/// The main event loop.
///
/// Handles all the PTY I/O and runs the PTY parser which updates terminal
/// state.
type TerminalUpdate<U> = Box<dyn Fn(&Term<U>) + Send + 'static>;

pub struct EventLoop<T: tty::EventedPty, U: EventListener> {
    poll: Arc<Poller>,
    pty: T,
    rx: PeekableReceiver<Msg>,
    tx: Sender<Msg>,
    terminal: Arc<FairMutex<Term<U>>>,
    event_proxy: U,
    terminal_update: Option<TerminalUpdate<U>>,
    terminal_update_interval: Duration,
    drain_on_exit: bool,
    ref_test: bool,
}

impl<T, U> EventLoop<T, U>
where
    T: tty::EventedPty + event::OnResize + Send + 'static,
    U: EventListener + Send + 'static,
{
    /// Create a new event loop.
    pub fn new(
        terminal: Arc<FairMutex<Term<U>>>,
        event_proxy: U,
        pty: T,
        drain_on_exit: bool,
        ref_test: bool,
    ) -> io::Result<EventLoop<T, U>> {
        let (tx, rx) = mpsc::channel();
        let poll = Poller::new()?.into();
        Ok(EventLoop {
            poll,
            pty,
            tx,
            rx: PeekableReceiver::new(rx),
            terminal,
            event_proxy,
            terminal_update: None,
            terminal_update_interval: Duration::ZERO,
            drain_on_exit,
            ref_test,
        })
    }

    /// Install a callback which observes the terminal after a renderable update while the PTY
    /// parser still owns the terminal lock.
    ///
    /// Consumers which mirror terminal state to another process can use this to publish an
    /// immutable render snapshot without racing the PTY reader for the live terminal mutex.
    pub fn with_terminal_update(mut self, callback: impl Fn(&Term<U>) + Send + 'static) -> Self {
        self.terminal_update = Some(Box::new(callback));
        self
    }

    /// Limit terminal update publication to a display-friendly cadence while always retaining a
    /// pending final update. This coalesces applications which flush a frame as many small PTY
    /// writes instead of forcing downstream renderers to rebuild for every write boundary.
    pub fn with_terminal_update_interval(mut self, interval: Duration) -> Self {
        self.terminal_update_interval = interval;
        self
    }

    pub fn channel(&self) -> EventLoopSender {
        EventLoopSender {
            sender: self.tx.clone(),
            poller: self.poll.clone(),
        }
    }

    /// Drain the channel.
    ///
    /// Returns `false` when a shutdown message was received.
    fn drain_recv_channel(&mut self, state: &mut State) -> bool {
        while let Some(msg) = self.rx.recv() {
            match msg {
                Msg::Input(input) => state.write_list.push_back(input),
                Msg::Resize(window_size) => self.pty.on_resize(window_size),
                Msg::Shutdown => return false,
            }
        }

        true
    }

    #[inline]
    fn pty_read<X>(
        &mut self,
        parser_tx: &SyncSender<ParserMsg>,
        recycle_rx: &Receiver<Vec<u8>>,
        spare: &mut Vec<u8>,
        max_drain: usize,
        mut writer: Option<&mut X>,
    ) -> io::Result<()>
    where
        X: Write,
    {
        let mut drained = 0;

        loop {
            let mut buffer = if spare.is_empty() {
                recycle_rx
                    .try_recv()
                    .unwrap_or_else(|_| vec![0; READ_BUFFER_SIZE])
            } else {
                std::mem::take(spare)
            };
            buffer.resize(READ_BUFFER_SIZE, 0);
            let read_started = Instant::now();
            let read_result = self.pty.reader().read(&mut buffer);
            pipeline_metrics::record(PipelineStage::PtyRead, read_started.elapsed());
            match read_result {
                // This is received on Windows/macOS when no more data is readable from the PTY.
                Ok(0) => {
                    *spare = buffer;
                    break;
                }
                Ok(got) => {
                    if let Some(writer) = &mut writer {
                        writer.write_all(&buffer[..got]).unwrap();
                    }
                    pipeline_metrics::record_pty_bytes(got);
                    send_parser_bytes(parser_tx, buffer, got)?;
                    drained += got;
                    if drained >= max_drain {
                        break;
                    }
                }
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted => continue,
                    ErrorKind::WouldBlock => {
                        *spare = buffer;
                        break;
                    }
                    _ => return Err(err),
                },
            }
        }

        Ok(())
    }

    #[inline]
    fn pty_write(&mut self, state: &mut State) -> io::Result<()> {
        state.ensure_next();

        'write_many: while let Some(mut current) = state.take_current() {
            'write_one: loop {
                match self.pty.writer().write(current.remaining_bytes()) {
                    Ok(0) => {
                        state.set_current(Some(current));
                        break 'write_many;
                    }
                    Ok(n) => {
                        current.advance(n);
                        if current.finished() {
                            state.goto_next();
                            break 'write_one;
                        }
                    }
                    Err(err) => {
                        state.set_current(Some(current));
                        match err.kind() {
                            ErrorKind::Interrupted | ErrorKind::WouldBlock => break 'write_many,
                            _ => return Err(err),
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn spawn(mut self) -> JoinHandle<(Self, State)>
    where
        U: Clone,
    {
        thread::spawn_named("PTY I/O", move || {
            let mut state = State::default();
            let mut read_buffer = vec![0; READ_BUFFER_SIZE];
            let (parser_tx, parser_rx) = mpsc::sync_channel(PARSER_QUEUE_DEPTH);
            let (recycle_tx, recycle_rx) = mpsc::channel();
            let parser = ParserDriver {
                terminal: Arc::clone(&self.terminal),
                event_proxy: self.event_proxy.clone(),
                terminal_update: self.terminal_update.take(),
                terminal_update_interval: self.terminal_update_interval,
                last_terminal_update: Cell::new(None),
                terminal_update_pending: Cell::new(false),
                state: ParserState::default(),
            };
            let parser_thread =
                thread::spawn_named("terminal parser", move || parser.run(parser_rx, recycle_tx));

            let poll_opts = PollMode::Level;
            let mut interest = PollingEvent::readable(0);

            // Register TTY through EventedRW interface.
            if let Err(err) = unsafe { self.pty.register(&self.poll, interest, poll_opts) } {
                error!("Event loop registration error: {err}");
                return (self, state);
            }

            let mut events = Events::with_capacity(NonZeroUsize::new(1024).unwrap());

            let mut pipe = if self.ref_test {
                Some(File::create("./alacritty.recording").expect("create alacritty recording"))
            } else {
                None
            };

            'event_loop: loop {
                events.clear();
                if let Err(err) = self.poll.wait(&mut events, None) {
                    match err.kind() {
                        ErrorKind::Interrupted => continue,
                        _ => {
                            error!("Event loop polling error: {err}");
                            break 'event_loop;
                        }
                    }
                }

                // Handle channel events, if there are any.
                if !self.drain_recv_channel(&mut state) {
                    break;
                }

                for event in events.iter() {
                    match event.key {
                        tty::PTY_CHILD_EVENT_TOKEN => {
                            if let Some(tty::ChildEvent::Exited(status)) =
                                self.pty.next_child_event()
                            {
                                if let Some(status) = status {
                                    self.event_proxy.send_event(Event::ChildExit(status));
                                }
                                if self.drain_on_exit {
                                    let _ = self.pty_read(
                                        &parser_tx,
                                        &recycle_rx,
                                        &mut read_buffer,
                                        usize::MAX,
                                        pipe.as_mut(),
                                    );
                                }
                                let _ = parser_tx.send(ParserMsg::ChildExit(status));
                                break 'event_loop;
                            }
                        }

                        tty::PTY_READ_WRITE_TOKEN => {
                            if event.is_interrupt() {
                                // Don't try to do I/O on a dead PTY.
                                continue;
                            }

                            if event.readable {
                                if let Err(err) = self.pty_read(
                                    &parser_tx,
                                    &recycle_rx,
                                    &mut read_buffer,
                                    MAX_PTY_DRAIN,
                                    pipe.as_mut(),
                                ) {
                                    // On Linux, a `read` on the master side of a PTY can fail
                                    // with `EIO` if the client side hangs up.  In that case,
                                    // just loop back round for the inevitable `Exited` event.
                                    // This sucks, but checking the process is either racy or
                                    // blocking.
                                    #[cfg(target_os = "linux")]
                                    if err.raw_os_error() == Some(libc::EIO) {
                                        continue;
                                    }

                                    error!("Error reading from PTY in event loop: {err}");
                                    break 'event_loop;
                                }
                            }

                            if event.writable {
                                if let Err(err) = self.pty_write(&mut state) {
                                    error!("Error writing to PTY in event loop: {err}");
                                    break 'event_loop;
                                }
                            }
                        }
                        _ => (),
                    }
                }

                // Register write interest if necessary.
                let needs_write = state.needs_write();
                if needs_write != interest.writable {
                    interest.writable = needs_write;

                    // Re-register with new interest.
                    self.pty
                        .reregister(&self.poll, interest, poll_opts)
                        .unwrap();
                }
            }

            // The evented instances are not dropped here so deregister them explicitly.
            let _ = self.pty.deregister(&self.poll);
            let _ = parser_tx.send(ParserMsg::Shutdown);
            let _ = parser_thread.join();

            (self, state)
        })
    }
}

fn send_parser_bytes(
    parser_tx: &SyncSender<ParserMsg>,
    mut buffer: Vec<u8>,
    length: usize,
) -> io::Result<()> {
    buffer.truncate(length);
    let queue_started = Instant::now();
    let sent = parser_tx.send(ParserMsg::Bytes(buffer));
    pipeline_metrics::record(PipelineStage::QueueWait, queue_started.elapsed());
    sent.map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "terminal parser stopped"))
}

enum ParserMsg {
    Bytes(Vec<u8>),
    ChildExit(Option<std::process::ExitStatus>),
    Shutdown,
}

#[derive(Default)]
struct ParserState {
    parser: ansi::Processor,
    kitty_apc: ApcSplitter,
    graphics_animation_deadline: Option<Instant>,
    graphics_decode_pending: bool,
}

struct ParserDriver<U: EventListener> {
    terminal: Arc<FairMutex<Term<U>>>,
    event_proxy: U,
    terminal_update: Option<TerminalUpdate<U>>,
    terminal_update_interval: Duration,
    last_terminal_update: Cell<Option<Instant>>,
    terminal_update_pending: Cell<bool>,
    state: ParserState,
}

impl<U: EventListener> ParserDriver<U> {
    fn publish_terminal_update(&self, terminal: &Term<U>, force: bool) -> bool {
        let Some(callback) = &self.terminal_update else {
            return false;
        };
        let now = Instant::now();
        let ready = force
            || self.terminal_update_interval.is_zero()
            || self.last_terminal_update.get().is_none_or(|last| {
                now.saturating_duration_since(last) >= self.terminal_update_interval
            });
        if ready {
            callback(terminal);
            self.last_terminal_update.set(Some(now));
            self.terminal_update_pending.set(false);
            true
        } else {
            self.terminal_update_pending.set(true);
            false
        }
    }

    fn terminal_update_timeout(&self) -> Option<Duration> {
        self.terminal_update_pending.get().then(|| {
            let elapsed = self
                .last_terminal_update
                .get()
                .map_or(self.terminal_update_interval, |last| last.elapsed());
            self.terminal_update_interval.saturating_sub(elapsed)
        })
    }

    fn timeout(&self) -> Option<Duration> {
        let sync = self
            .state
            .parser
            .sync_timeout()
            .sync_timeout()
            .map(|deadline| deadline.saturating_duration_since(Instant::now()));
        let animation = self
            .state
            .graphics_animation_deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()));
        let update = sync
            .is_none()
            .then(|| self.terminal_update_timeout())
            .flatten();
        let decode = self
            .state
            .graphics_decode_pending
            .then_some(Duration::from_millis(1));
        [sync, animation, update, decode]
            .into_iter()
            .flatten()
            .min()
    }

    fn process_bytes(&mut self, bytes: &[u8]) {
        let parser_started = Instant::now();
        let mut terminal = self.terminal.lock();
        let mut changed = false;
        let parser = &mut self.state.parser;
        self.state.kitty_apc.advance(bytes, |output| match output {
            ApcOutput::Text(bytes) => {
                let started = Instant::now();
                changed |= !bytes.is_empty();
                parser.advance(&mut *terminal, bytes);
                pipeline_metrics::record(PipelineStage::Vte, started.elapsed());
            }
            ApcOutput::Kitty(payload) => {
                let started = Instant::now();
                changed |= execute_kitty_apc(parser, &mut *terminal, payload);
                pipeline_metrics::record(PipelineStage::Kitty, started.elapsed());
            }
            ApcOutput::TerminalVersionQuery => terminal.report_terminal_version(),
        });
        changed |= terminal.flush_ready_kitty_graphics_commands();
        self.state.graphics_decode_pending = terminal.has_pending_kitty_graphics_commands();
        if parser.sync_timeout().sync_timeout().is_none() && changed {
            self.publish_terminal_update(&terminal, false);
            self.event_proxy.send_event(Event::Wakeup);
        }
        self.state.graphics_animation_deadline = terminal.next_graphics_animation_deadline();
        pipeline_metrics::record(PipelineStage::Parser, parser_started.elapsed());
        pipeline_metrics::report_if_due();
    }

    fn handle_timeout(&mut self) {
        let mut terminal = self.terminal.lock();
        let now = Instant::now();
        let synchronized_update_expired = self
            .state
            .parser
            .sync_timeout()
            .sync_timeout()
            .is_some_and(|deadline| deadline <= now);
        if synchronized_update_expired {
            self.state.parser.stop_sync(&mut *terminal);
        }
        let decode_changed = terminal.flush_ready_kitty_graphics_commands();
        self.state.graphics_decode_pending = terminal.has_pending_kitty_graphics_commands();
        let animation_changed = terminal.advance_graphics_animations(now);
        self.state.graphics_animation_deadline = terminal.next_graphics_animation_deadline();
        let synchronized = self.state.parser.sync_timeout().sync_timeout().is_some();
        let forced =
            synchronized_update_expired || !synchronized && (animation_changed || decode_changed);
        let published = !synchronized && self.publish_terminal_update(&terminal, forced);
        if forced || published {
            self.event_proxy.send_event(Event::Wakeup);
        }
    }

    fn run(mut self, rx: Receiver<ParserMsg>, recycle_tx: Sender<Vec<u8>>) {
        loop {
            let message = match self.timeout() {
                Some(timeout) => match rx.recv_timeout(timeout) {
                    Ok(message) => message,
                    Err(RecvTimeoutError::Timeout) => {
                        self.handle_timeout();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(message) => message,
                    Err(_) => break,
                },
            };
            match message {
                ParserMsg::Bytes(buffer) => {
                    self.process_bytes(&buffer);
                    let _ = recycle_tx.send(buffer);
                }
                ParserMsg::ChildExit(status) => {
                    let mut terminal = self.terminal.lock();
                    terminal.flush_kitty_graphics_commands();
                    terminal.exit();
                    self.publish_terminal_update(&terminal, true);
                    self.event_proxy.send_event(Event::Wakeup);
                    if let Some(status) = status {
                        self.event_proxy.send_event(Event::ChildExit(status));
                    }
                    break;
                }
                ParserMsg::Shutdown => break,
            }
        }
    }
}

/// Execute a graphics command at its exact position inside a synchronized update.
///
/// VTE defers ordinary bytes while mode 2026 is active. Kitty APCs are intercepted before VTE,
/// so the cursor-positioning bytes preceding an APC must be committed before the command reads
/// the terminal cursor. Immediately re-entering synchronized mode keeps the update atomic until
/// the application's real ESU sequence arrives.
fn execute_kitty_apc<U: EventListener>(
    parser: &mut ansi::Processor,
    terminal: &mut Term<U>,
    payload: &[u8],
) -> bool {
    let synchronized = parser.sync_timeout().sync_timeout().is_some();
    if synchronized {
        parser.stop_sync(terminal);
    }
    let changed = terminal.kitty_graphics_command(payload);
    if synchronized {
        parser.advance(terminal, b"\x1b[?2026h");
    }
    changed
}

/// Helper type which tracks how much of a buffer has been written.
struct Writing {
    source: Cow<'static, [u8]>,
    written: usize,
}

pub struct Notifier(pub EventLoopSender);

impl event::Notify for Notifier {
    fn notify<B>(&self, bytes: B)
    where
        B: Into<Cow<'static, [u8]>>,
    {
        let bytes = bytes.into();
        // Terminal hangs if we send 0 bytes through.
        if bytes.is_empty() {
            return;
        }

        let _ = self.0.send(Msg::Input(bytes));
    }
}

impl event::OnResize for Notifier {
    fn on_resize(&mut self, window_size: WindowSize) {
        let _ = self.0.send(Msg::Resize(window_size));
    }
}

#[derive(Debug)]
pub enum EventLoopSendError {
    /// Error polling the event loop.
    Io(io::Error),

    /// Error sending a message to the event loop.
    Send(mpsc::SendError<Msg>),
}

impl Display for EventLoopSendError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            EventLoopSendError::Io(err) => err.fmt(f),
            EventLoopSendError::Send(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for EventLoopSendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EventLoopSendError::Io(err) => err.source(),
            EventLoopSendError::Send(err) => err.source(),
        }
    }
}

#[derive(Clone)]
pub struct EventLoopSender {
    sender: Sender<Msg>,
    poller: Arc<Poller>,
}

impl EventLoopSender {
    pub fn send(&self, msg: Msg) -> Result<(), EventLoopSendError> {
        self.sender.send(msg).map_err(EventLoopSendError::Send)?;
        self.poller.notify().map_err(EventLoopSendError::Io)
    }
}

/// All of the mutable state needed to run the event loop.
///
/// Contains list of items to write, current write state, etc. Anything that
/// would otherwise be mutated on the `EventLoop` goes here.
#[derive(Default)]
pub struct State {
    write_list: VecDeque<Cow<'static, [u8]>>,
    writing: Option<Writing>,
}

impl State {
    #[inline]
    fn ensure_next(&mut self) {
        if self.writing.is_none() {
            self.goto_next();
        }
    }

    #[inline]
    fn goto_next(&mut self) {
        self.writing = self.write_list.pop_front().map(Writing::new);
    }

    #[inline]
    fn take_current(&mut self) -> Option<Writing> {
        self.writing.take()
    }

    #[inline]
    fn needs_write(&self) -> bool {
        self.writing.is_some() || !self.write_list.is_empty()
    }

    #[inline]
    fn set_current(&mut self, new: Option<Writing>) {
        self.writing = new;
    }
}

impl Writing {
    #[inline]
    fn new(c: Cow<'static, [u8]>) -> Writing {
        Writing {
            source: c,
            written: 0,
        }
    }

    #[inline]
    fn advance(&mut self, n: usize) {
        self.written += n;
    }

    #[inline]
    fn remaining_bytes(&self) -> &[u8] {
        &self.source[self.written..]
    }

    #[inline]
    fn finished(&self) -> bool {
        self.written >= self.source.len()
    }
}

struct PeekableReceiver<T> {
    rx: Receiver<T>,
}

impl<T> PeekableReceiver<T> {
    fn new(rx: Receiver<T>) -> Self {
        Self { rx }
    }

    fn recv(&mut self) -> Option<T> {
        match self.rx.try_recv() {
            Err(TryRecvError::Disconnected) => panic!("event loop channel closed"),
            res => res.ok(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::event::VoidListener;
    use crate::term::Config;
    use crate::term::test::TermSize;

    #[test]
    fn kitty_apc_observes_cursor_moves_inside_synchronized_updates() {
        let size = TermSize::new(80, 24);
        let mut terminal = Term::new(Config::default(), &size, VoidListener);
        terminal.set_kitty_graphics_cell_size(8, 18);
        let mut parser = ansi::Processor::new();
        let mut splitter = ApcSplitter::default();

        splitter.advance(
            b"\x1b[?2026h\x1b[9;56H\x1b_Ga=T,f=32,s=1,v=1,i=7,c=1,r=1,C=1;AQID/w==\x1b\\\x1b[?2026l",
            |output| match output {
                ApcOutput::Text(bytes) => parser.advance(&mut terminal, bytes),
                ApcOutput::Kitty(payload) => {
                    execute_kitty_apc(&mut parser, &mut terminal, payload);
                },
                ApcOutput::TerminalVersionQuery => terminal.report_terminal_version(),
            },
        );

        let snapshot = terminal.kitty_graphics_snapshot();
        assert_eq!(snapshot.placements.len(), 1);
        assert_eq!(
            (snapshot.placements[0].line, snapshot.placements[0].column),
            (8, 55)
        );
        assert!(parser.sync_timeout().sync_timeout().is_none());
    }
}
