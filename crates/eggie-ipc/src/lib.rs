//! Eggie client connections and shared wire transport, independent of daemon implementation.
mod client;
pub mod wire;
pub use client::{
    DaemonClient, DaemonConnection, DaemonInputSender, configure_latency_sensitive_thread,
    daemon_socket_path, handshake_accepted, is_daemon_invocation, terminate_daemon_at,
};
use eggie_protocol::TerminalImageKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalImageChunkMetadata {
    pub key: TerminalImageKey,
    pub width: u32,
    pub height: u32,
    pub total_length: u32,
    pub offset: u32,
    pub chunk_length: u32,
}

/// A whole terminal image fetched over the wire, either inline pixels or a reference to a POSIX
/// shm segment the caller can map directly. Returned by [`DaemonConnection::fetch_terminal_image`]
/// so a zero-copy consumer (the UI) can `mmap` the segment instead of copying it into a `Vec`.
#[derive(Debug)]
pub enum TerminalImageFrame {
    /// Pixels delivered inline on the socket; already owned by the caller.
    Inline {
        metadata: TerminalImageChunkMetadata,
        pixels: Vec<u8>,
    },
    /// A reference to a daemon-created shm segment. The caller owns the unlink: it must
    /// `shm_open` + `shm_unlink` + `mmap` the name (see [`wire::ShmImageSegment`] for the lifecycle).
    Shm {
        metadata: TerminalImageChunkMetadata,
        segment_name: Vec<u8>,
    },
    /// The image is larger than one inline wire chunk, so the whole-image fetch only saw its first
    /// chunk. The caller must fall back to [`DaemonConnection::append_terminal_image_chunk`] to walk
    /// the remaining chunks. A typed variant (rather than a stringly-matched error) makes the
    /// fallback contract explicit and compiler-checked.
    InlineTooLarge {
        metadata: TerminalImageChunkMetadata,
    },
}
