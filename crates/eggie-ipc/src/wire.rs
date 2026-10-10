//! Length-prefixed MessagePack and generation-checked image transport.
use crate::TerminalImageChunkMetadata;
use anyhow::{Context, Result, bail};
use eggie_protocol::{DaemonResponse, TerminalImageKey};
use std::{
    fs::File,
    io::{Read, Write},
    os::fd::AsRawFd,
    process,
    sync::atomic::{AtomicU64, Ordering},
};
pub const MAX_WIRE_MESSAGE_SIZE: usize = 64 * 1024 * 1024;
pub const MAX_IMAGE_CHUNK_SIZE: usize = 16 * 1024 * 1024;
pub const MAX_SHM_IMAGE_BYTES: usize = 400 * 1024 * 1024;
pub const RAW_TERMINAL_IMAGE_WIRE_FLAG: u32 = 1 << 31;
pub const SHM_TERMINAL_IMAGE_WIRE_FLAG: u32 = 1 << 30;
pub const WIRE_HEADER_FLAG_MASK: u32 = RAW_TERMINAL_IMAGE_WIRE_FLAG | SHM_TERMINAL_IMAGE_WIRE_FLAG;
pub const RAW_TERMINAL_IMAGE_METADATA_SIZE: usize = 28;
/// How a peer should interpret a wire frame, decoded from its 4-byte length-prefix header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireFrameKind {
    /// No flag bits set — a length-prefixed MessagePack message.
    Message,
    /// [`RAW_TERMINAL_IMAGE_WIRE_FLAG`] set — raw image pixels follow inline.
    RawImage,
    /// [`SHM_TERMINAL_IMAGE_WIRE_FLAG`] set — an shm segment reference follows (no inline pixels).
    ShmImage,
}

/// Split a wire header into its frame kind and payload length. The length is the header with all
/// flag bits cleared, so it stays correct no matter which (mutually exclusive) flag is set.
pub fn classify_wire_header(header: u32) -> (WireFrameKind, usize) {
    let length = (header & !WIRE_HEADER_FLAG_MASK) as usize;
    let kind = if header & RAW_TERMINAL_IMAGE_WIRE_FLAG != 0 {
        WireFrameKind::RawImage
    } else if header & SHM_TERMINAL_IMAGE_WIRE_FLAG != 0 {
        WireFrameKind::ShmImage
    } else {
        WireFrameKind::Message
    };
    (kind, length)
}

/// Pack the 28-byte image-chunk metadata header shared by the inline and shm wire framings. The
/// layout (all little-endian) is: id[0..4], generation[4..12], width[12..16], height[16..20],
/// total_length[20..24], offset[24..28]. `#[inline]` keeps this a zero-cost stack fill on the
/// image-publish hot path.
#[inline]
pub fn pack_image_metadata(
    key: TerminalImageKey,
    width: u32,
    height: u32,
    total_length: u32,
    offset: u32,
) -> [u8; RAW_TERMINAL_IMAGE_METADATA_SIZE] {
    let mut header = [0_u8; RAW_TERMINAL_IMAGE_METADATA_SIZE];
    header[0..4].copy_from_slice(&key.id.to_le_bytes());
    header[4..12].copy_from_slice(&key.generation.to_le_bytes());
    header[12..16].copy_from_slice(&width.to_le_bytes());
    header[16..20].copy_from_slice(&height.to_le_bytes());
    header[20..24].copy_from_slice(&total_length.to_le_bytes());
    header[24..28].copy_from_slice(&offset.to_le_bytes());
    header
}

/// Unpack the 28-byte header written by [`pack_image_metadata`]. Returns `(key, width, height,
/// total_length, offset)`; `chunk_length` is framing-specific and stays with the caller.
#[inline]
pub fn unpack_image_metadata(
    header: &[u8; RAW_TERMINAL_IMAGE_METADATA_SIZE],
) -> (TerminalImageKey, u32, u32, u32, u32) {
    let read_u32 = |offset: usize| {
        u32::from_le_bytes(header[offset..offset + 4].try_into().expect("four bytes"))
    };
    let key = TerminalImageKey {
        id: read_u32(0),
        generation: u64::from_le_bytes(header[4..12].try_into().expect("eight bytes")),
    };
    (key, read_u32(12), read_u32(16), read_u32(20), read_u32(24))
}

#[derive(Clone, Copy)]
pub struct TerminalImageChunk<'a> {
    pub key: TerminalImageKey,
    pub width: u32,
    pub height: u32,
    pub total_length: u32,
    pub offset: u32,
    pub pixels: &'a [u8],
    pub end: usize,
}
impl TerminalImageChunk<'_> {
    pub fn bytes(&self) -> &[u8] {
        &self.pixels[self.offset as usize..self.end]
    }
    #[cfg(unix)]
    fn all_bytes(&self) -> &[u8] {
        self.pixels
    }
}
pub fn read_wire_message<T: serde::de::DeserializeOwned>(
    reader: &mut impl Read,
    buffer: &mut Vec<u8>,
) -> Result<Option<T>> {
    let Some(header) = read_wire_header(reader)? else {
        return Ok(None);
    };
    let (kind, length) = classify_wire_header(header);
    if kind != WireFrameKind::Message {
        bail!("unexpected raw terminal image wire response");
    }
    read_message_body(reader, buffer, length).map(Some)
}

pub fn read_wire_header(reader: &mut impl Read) -> Result<Option<u32>> {
    let mut header = [0_u8; 4];
    match reader.read(&mut header[..1]) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(error) => return Err(error.into()),
    }
    reader.read_exact(&mut header[1..])?;
    Ok(Some(u32::from_le_bytes(header)))
}

pub fn read_message_body<T: serde::de::DeserializeOwned>(
    reader: &mut impl Read,
    buffer: &mut Vec<u8>,
    length: usize,
) -> Result<T> {
    if length > MAX_WIRE_MESSAGE_SIZE {
        bail!("daemon wire message exceeds {MAX_WIRE_MESSAGE_SIZE} bytes: {length}");
    }
    buffer.resize(length, 0);
    reader.read_exact(buffer)?;
    let value = rmp_serde::from_slice(buffer).context("invalid daemon wire message")?;
    Ok(value)
}

pub fn write_wire_message(
    writer: &mut impl Write,
    buffer: &mut Vec<u8>,
    value: &impl serde::Serialize,
) -> Result<()> {
    buffer.clear();
    rmp_serde::encode::write_named(&mut *buffer, value)
        .context("failed to encode daemon wire message")?;
    if buffer.len() > MAX_WIRE_MESSAGE_SIZE {
        bail!(
            "daemon wire message exceeds {MAX_WIRE_MESSAGE_SIZE} bytes: {}",
            buffer.len()
        );
    }
    let length = u32::try_from(buffer.len()).context("daemon wire message is too large")?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(buffer)?;
    Ok(())
}

/// Send one terminal-image chunk response. On unix, the first chunk of an image (`offset == 0`) is
/// delivered zero-inline-copy via a shared-memory segment: the whole image is copied into an shm
/// object and only its name travels the socket. Non-zero offsets (a client re-requesting a tail
/// after a partial read) and non-unix platforms fall back to the inline wire frame. Because a shm
/// frame carries the entire image (`chunk_length == total_length`), the client's chunk loop
/// completes in one iteration and never asks for a non-zero offset in practice.
pub fn send_terminal_image_chunk(
    writer: &mut impl Write,
    chunk: &TerminalImageChunk<'_>,
) -> Result<()> {
    #[cfg(unix)]
    {
        if chunk.offset == 0 {
            let pixels = chunk.all_bytes();
            let mut segment = create_image_shm_segment(pixels)?;
            let metadata = TerminalImageChunkMetadata {
                key: chunk.key,
                width: chunk.width,
                height: chunk.height,
                total_length: chunk.total_length,
                offset: 0,
                chunk_length: chunk.total_length,
            };
            write_terminal_image_shm_wire(writer, &metadata, segment.name_bytes())?;
            // The name is on the wire; the consumer now owns the unlink.
            segment.disarm();
            return Ok(());
        }
    }
    write_terminal_image_wire(writer, chunk)
}

pub fn write_terminal_image_wire(
    writer: &mut impl Write,
    chunk: &TerminalImageChunk<'_>,
) -> Result<()> {
    let bytes = chunk.bytes();
    let payload_length = RAW_TERMINAL_IMAGE_METADATA_SIZE
        .checked_add(bytes.len())
        .context("terminal image wire message length overflow")?;
    if payload_length > MAX_WIRE_MESSAGE_SIZE {
        bail!(
            "terminal image wire message exceeds {MAX_WIRE_MESSAGE_SIZE} bytes: {payload_length}"
        );
    }
    let payload_length =
        u32::try_from(payload_length).context("terminal image chunk is too large")?;
    let metadata = pack_image_metadata(
        chunk.key,
        chunk.width,
        chunk.height,
        chunk.total_length,
        chunk.offset,
    );
    writer.write_all(&(payload_length | RAW_TERMINAL_IMAGE_WIRE_FLAG).to_le_bytes())?;
    writer.write_all(&metadata)?;
    writer.write_all(bytes)?;
    Ok(())
}

pub fn read_terminal_image_wire_into(
    reader: &mut impl Read,
    payload_length: usize,
    destination: &mut Vec<u8>,
) -> Result<TerminalImageChunkMetadata> {
    if !(RAW_TERMINAL_IMAGE_METADATA_SIZE..=MAX_WIRE_MESSAGE_SIZE).contains(&payload_length) {
        bail!("invalid raw terminal image wire length: {payload_length}");
    }
    let mut metadata = [0_u8; RAW_TERMINAL_IMAGE_METADATA_SIZE];
    reader.read_exact(&mut metadata)?;
    let (key, width, height, total_length, offset) = unpack_image_metadata(&metadata);
    let chunk_length = payload_length - RAW_TERMINAL_IMAGE_METADATA_SIZE;
    let chunk_length = u32::try_from(chunk_length).context("terminal image chunk is too large")?;
    let original_length = destination.len();
    destination.resize(original_length + chunk_length as usize, 0);
    if let Err(error) = reader.read_exact(&mut destination[original_length..]) {
        destination.truncate(original_length);
        return Err(error.into());
    }
    Ok(TerminalImageChunkMetadata {
        key,
        width,
        height,
        total_length,
        offset,
        chunk_length,
    })
}

/// A daemon-created POSIX shared-memory segment holding image pixels for a single wire response.
///
/// Lifecycle (mirrors the consumer half in `kitty_graphics::read_shared_memory`, reversed): the
/// daemon creates the segment, copies pixels in, and closes its fd *without* unlinking. The
/// consumer (UI) opens the segment by name, unlinks it immediately, maps/copies the pixels, and
/// munmaps — the kernel keeps the object alive until both sides munmap. So on the success path the
/// daemon hands ownership of the unlink to the consumer via [`disarm`](Self::disarm). If the name
/// is never sent (an error before hand-off), `Drop` unlinks so the segment cannot leak. A consumer
/// crash after the name is sent but before it opens leaks one bounded segment until reboot — the
/// accepted cost of avoiding a synchronous ack.
///
/// A1 checkpoint: the daemon sends shm frames, but the UI still copies the mapped pixels into a
/// `Vec` before uploading — so the socket no longer carries pixels, but the total copy count is
/// unchanged until the UI's zero-copy `mmap`→Metal path (A2) lands.
#[cfg(unix)]
pub struct ShmImageSegment {
    name: std::ffi::CString,
    armed: bool,
}

#[cfg(unix)]
impl ShmImageSegment {
    pub fn name_bytes(&self) -> &[u8] {
        self.name.as_bytes()
    }

    /// Transfer unlink responsibility to the consumer. Call only after the name is on the wire.
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(unix)]
impl Drop for ShmImageSegment {
    fn drop(&mut self) {
        if self.armed {
            unsafe {
                libc::shm_unlink(self.name.as_ptr());
            }
        }
    }
}

/// Create a uniquely-named POSIX shm segment, copy `pixels` in, and return an armed guard. The fd
/// is closed before returning; the segment stays linked until the consumer opens+unlinks it (or the
/// guard's `Drop` unlinks it on an error path). The name is unique per daemon process and segment.
#[cfg(unix)]
pub fn create_image_shm_segment(pixels: &[u8]) -> Result<ShmImageSegment> {
    use std::os::fd::FromRawFd;

    static SHM_SEGMENT_SERIAL: AtomicU64 = AtomicU64::new(0);
    let serial = SHM_SEGMENT_SERIAL.fetch_add(1, Ordering::Relaxed);
    let name = std::ffi::CString::new(format!("/eggie-img-{}-{serial}", process::id()))
        .context("shm segment name contained a nul byte")?;
    let flags = libc::O_CREAT | libc::O_RDWR | libc::O_EXCL;
    let mut fd = unsafe { libc::shm_open(name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        // EEXIST means a prior daemon instance crashed (Drop never ran) and the OS reused its pid,
        // so this exact name is an orphaned, linked segment. It belongs to our pid namespace and no
        // live instance references it, so reclaim it and retry once rather than tearing down the
        // connection.
        if error.raw_os_error() == Some(libc::EEXIST) {
            unsafe {
                libc::shm_unlink(name.as_ptr());
            }
            fd = unsafe { libc::shm_open(name.as_ptr(), flags, 0o600) };
        }
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("shm_open failed to create image segment");
        }
    }
    // Arm the guard as soon as the object exists so every later error path unlinks it.
    let guard = ShmImageSegment { name, armed: true };
    // RAII close of the fd; the segment outlives the fd once created.
    let file = unsafe { File::from_raw_fd(fd) };
    let len = pixels.len();
    if unsafe { libc::ftruncate(file.as_raw_fd(), len as libc::off_t) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("ftruncate failed to size image segment");
    }
    if len > 0 {
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if mapping == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error())
                .context("mmap failed to write image segment");
        }
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), mapping.cast::<u8>(), len);
            libc::munmap(mapping, len);
        }
    }
    Ok(guard)
}

/// Serialize an shm image frame: the 28-byte metadata header (identical layout to the inline raw
/// frame) followed by a length-prefixed segment name. The pixels themselves never touch the socket.
pub fn write_terminal_image_shm_wire(
    writer: &mut impl Write,
    metadata: &TerminalImageChunkMetadata,
    segment_name: &[u8],
) -> Result<()> {
    let payload_length = RAW_TERMINAL_IMAGE_METADATA_SIZE
        .checked_add(4)
        .and_then(|size| size.checked_add(segment_name.len()))
        .context("shm terminal image wire length overflow")?;
    if payload_length > MAX_WIRE_MESSAGE_SIZE {
        bail!(
            "shm terminal image wire message exceeds {MAX_WIRE_MESSAGE_SIZE} bytes: {payload_length}"
        );
    }
    let payload_length =
        u32::try_from(payload_length).context("shm terminal image name is too large")?;
    let name_length =
        u32::try_from(segment_name.len()).context("shm terminal image name is too large")?;
    let header = pack_image_metadata(
        metadata.key,
        metadata.width,
        metadata.height,
        metadata.total_length,
        metadata.offset,
    );
    writer.write_all(&(payload_length | SHM_TERMINAL_IMAGE_WIRE_FLAG).to_le_bytes())?;
    writer.write_all(&header)?;
    writer.write_all(&name_length.to_le_bytes())?;
    writer.write_all(segment_name)?;
    Ok(())
}

/// Read an shm image frame's header + segment name from the wire, without opening the segment.
/// Returns the frame metadata (with `chunk_length` left as `total_length`, since a shm frame always
/// carries the whole image) and the raw segment name. The consumer decides whether to copy the
/// segment into a `Vec` (daemon-side/fallback) or map it directly (the UI's zero-copy path).
pub fn read_terminal_image_shm_wire_header(
    reader: &mut impl Read,
    payload_length: usize,
) -> Result<(TerminalImageChunkMetadata, Vec<u8>)> {
    let minimum = RAW_TERMINAL_IMAGE_METADATA_SIZE + 4;
    if !(minimum..=MAX_WIRE_MESSAGE_SIZE).contains(&payload_length) {
        bail!("invalid shm terminal image wire length: {payload_length}");
    }
    let mut metadata = [0_u8; RAW_TERMINAL_IMAGE_METADATA_SIZE];
    reader.read_exact(&mut metadata)?;
    let (key, width, height, total_length, offset) = unpack_image_metadata(&metadata);
    let mut name_length = [0_u8; 4];
    reader.read_exact(&mut name_length)?;
    let name_length = u32::from_le_bytes(name_length) as usize;
    if minimum + name_length != payload_length {
        bail!("shm terminal image wire name length mismatch");
    }
    let mut name = vec![0_u8; name_length];
    reader.read_exact(&mut name)?;
    let metadata = TerminalImageChunkMetadata {
        key,
        width,
        height,
        total_length,
        offset,
        // A shm frame always carries the whole image, so the chunk spans the full length.
        chunk_length: total_length,
    };
    Ok((metadata, name))
}

/// Read an shm image frame, open the referenced segment, and copy its pixels into `destination`.
/// This is the functional (copy-into-Vec) decode used by the daemon-side read helpers and the
/// `DaemonConnection` fallback; the UI's zero-copy `mmap` path (A2) maps the segment directly.
pub fn read_terminal_image_shm_wire_into(
    reader: &mut impl Read,
    payload_length: usize,
    destination: &mut Vec<u8>,
) -> Result<TerminalImageChunkMetadata> {
    let (metadata, name) = read_terminal_image_shm_wire_header(reader, payload_length)?;
    let copied = read_image_shm_segment(&name, metadata.total_length as usize, destination)?;
    if copied != metadata.total_length as usize {
        bail!("shm terminal image segment shorter than declared length");
    }
    Ok(metadata)
}

/// Open the named POSIX shm segment read-only, unlink it (so it is reclaimed once munmapped), map
/// it, append exactly `total_length` bytes to `destination`, and munmap. `total_length` comes from
/// the wire metadata rather than the segment's `metadata().len()`, because POSIX shm objects are
/// rounded up to a page boundary (so the object is often larger than the pixel payload). Mirrors
/// `kitty_graphics::read_shared_memory`; returns the number of bytes appended.
#[cfg(unix)]
pub fn read_image_shm_segment(
    name: &[u8],
    total_length: usize,
    destination: &mut Vec<u8>,
) -> Result<usize> {
    use std::os::fd::FromRawFd;

    let name = std::ffi::CString::new(name).context("shm segment name contained a nul byte")?;
    let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .context("shm_open failed to read image segment");
    }
    // Unlink immediately: the kernel keeps the object alive until we munmap, and this guarantees the
    // segment is reclaimed even if the rest of this function fails.
    unsafe {
        libc::shm_unlink(name.as_ptr());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    if total_length > MAX_SHM_IMAGE_BYTES {
        bail!("image segment exceeds {MAX_SHM_IMAGE_BYTES} bytes: {total_length}");
    }
    if total_length == 0 {
        return Ok(0);
    }
    let object_len = file
        .metadata()
        .context("failed to stat image segment")?
        .len();
    if (object_len as usize) < total_length {
        bail!("image segment holds {object_len} bytes, expected at least {total_length}");
    }
    let map_len = object_len as usize;
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            map_len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if mapping == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error()).context("mmap failed to read image segment");
    }
    let original_length = destination.len();
    destination.resize(original_length + total_length, 0);
    unsafe {
        std::ptr::copy_nonoverlapping(
            mapping.cast::<u8>(),
            destination[original_length..].as_mut_ptr(),
            total_length,
        );
        libc::munmap(mapping, map_len);
    }
    Ok(total_length)
}

#[cfg(not(unix))]
pub fn read_image_shm_segment(
    _name: &[u8],
    _total_length: usize,
    _destination: &mut Vec<u8>,
) -> Result<usize> {
    bail!("shared-memory image transport is only supported on unix")
}

pub fn read_daemon_response(
    reader: &mut impl Read,
    buffer: &mut Vec<u8>,
) -> Result<Option<DaemonResponse>> {
    let Some(header) = read_wire_header(reader)? else {
        return Ok(None);
    };
    let (kind, length) = classify_wire_header(header);
    let mut bytes = Vec::new();
    let metadata = match kind {
        WireFrameKind::Message => return read_message_body(reader, buffer, length).map(Some),
        WireFrameKind::RawImage => read_terminal_image_wire_into(reader, length, &mut bytes)?,
        WireFrameKind::ShmImage => read_terminal_image_shm_wire_into(reader, length, &mut bytes)?,
    };
    Ok(Some(DaemonResponse::TerminalImage {
        key: metadata.key,
        width: metadata.width,
        height: metadata.height,
        total_length: metadata.total_length,
        offset: metadata.offset,
        bytes,
    }))
}

/// Open the daemon-created POSIX shm segment read-only, unlink it immediately (the kernel keeps it
/// alive until both sides unmap, so this reclaims it once we drop the mapping), and map it. The
/// returned `Mmap` is the image's pixels read straight from shared memory — never copied into a
/// `Vec`. `expected_length` is validated against the segment size.
#[cfg(unix)]
pub fn map_image_shm_segment(name: &[u8], expected_length: usize) -> anyhow::Result<memmap2::Mmap> {
    use std::os::fd::FromRawFd;

    let cname = std::ffi::CString::new(name)
        .map_err(|_| anyhow::anyhow!("shm segment name contained a nul byte"))?;
    let fd = unsafe { libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0) };
    if fd < 0 {
        return Err(anyhow::anyhow!(
            "shm_open failed for image segment: {}",
            std::io::Error::last_os_error()
        ));
    }
    // Reclaim the name immediately; the mapping keeps the object alive until it is dropped.
    unsafe {
        libc::shm_unlink(cname.as_ptr());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let object_len = file
        .metadata()
        .map_err(|error| anyhow::anyhow!("failed to stat image segment: {error}"))?
        .len();
    if (object_len as usize) < expected_length {
        anyhow::bail!(
            "image segment holds {object_len} bytes, expected at least {expected_length}"
        );
    }
    // SAFETY: the segment is a fixed-size, read-only POSIX shm object we just opened; no other
    // process mutates it (the daemon writes once before sending the name).
    let mmap = unsafe {
        memmap2::Mmap::map(&file)
            .map_err(|error| anyhow::anyhow!("mmap failed for image segment: {error}"))?
    };
    Ok(mmap)
}

#[cfg(not(unix))]
pub fn map_image_shm_segment(
    _name: &[u8],
    _expected_length: usize,
) -> anyhow::Result<memmap2::Mmap> {
    anyhow::bail!("shared-memory image transport is only supported on unix")
}
