//! Kitty terminal graphics protocol state.
//!
//! The PTY parser owns this state so image placement observes exactly the same cursor and
//! scrolling operations as text. Decoded pixels are immutable and shared with consumers; normal
//! terminal snapshots only carry small descriptors and placements.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as Base64;
use flate2::read::ZlibDecoder;
use image::{ImageFormat, ImageReader, Limits};
use memchr::memchr;

use crate::pipeline_metrics::{self, Stage as PipelineStage};

thread_local! {
    /// libdeflate is optimized for the known-size, one-shot buffers used by Kitty raw-pixel
    /// transmissions. Reusing one decoder per PTY thread avoids allocating its tables per frame.
    static LIBDEFLATE: RefCell<libdeflater::Decompressor> =
        RefCell::new(libdeflater::Decompressor::new());
}

use super::kitty_diacritics::DIACRITICS;

const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 400 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 10_000;
const DEFAULT_STORAGE_LIMIT: usize = 320 * 1_000 * 1_000;
const BACKGROUND_Z_LIMIT: i32 = i32::MIN / 2;
const DECODE_WORKERS: usize = 2;
const DECODE_QUEUE_DEPTH: usize = 4;
const MAX_PENDING_DECODES: usize = 3;
const MAX_POOLED_BUFFERS: usize = 8;
const MAX_POOLED_BUFFER_BYTES: usize = 64 * 1024 * 1024;

fn pixel_buffer_pool() -> &'static Mutex<Vec<Vec<u8>>> {
    static POOL: OnceLock<Mutex<Vec<Vec<u8>>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(Vec::new()))
}

fn take_pixel_buffer(minimum_capacity: usize) -> Vec<u8> {
    let mut pool = pixel_buffer_pool()
        .lock()
        .expect("pixel buffer pool poisoned");
    let maximum_reuse = minimum_capacity.max(1024 * 1024).saturating_mul(4);
    let candidate = pool
        .iter()
        .enumerate()
        .filter(|(_, buffer)| {
            buffer.capacity() >= minimum_capacity && buffer.capacity() <= maximum_reuse
        })
        .min_by_key(|(_, buffer)| buffer.capacity())
        .map(|(index, _)| index);
    candidate.map_or_else(
        || Vec::with_capacity(minimum_capacity),
        |index| pool.swap_remove(index),
    )
}

fn recycle_pixel_buffer(mut buffer: Vec<u8>) {
    if buffer.capacity() > MAX_POOLED_BUFFER_BYTES {
        return;
    }
    buffer.clear();
    let mut pool = pixel_buffer_pool()
        .lock()
        .expect("pixel buffer pool poisoned");
    if pool.len() < MAX_POOLED_BUFFERS {
        pool.push(buffer);
    }
}

/// Immutable decoded pixels whose allocation returns to the decoder pool after the final image,
/// snapshot and daemon reference is released.
pub struct PixelBuffer {
    data: Option<Vec<u8>>,
}

impl PixelBuffer {
    pub fn from_vec(data: Vec<u8>) -> Self {
        Self { data: Some(data) }
    }
}

impl std::fmt::Debug for PixelBuffer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PixelBuffer")
            .field("len", &self.len())
            .finish()
    }
}

impl std::ops::Deref for PixelBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.data.as_deref().unwrap_or_default()
    }
}

impl<const N: usize> PartialEq<[u8; N]> for PixelBuffer {
    fn eq(&self, other: &[u8; N]) -> bool {
        &**self == other
    }
}

impl Drop for PixelBuffer {
    fn drop(&mut self) {
        if let Some(data) = self.data.take() {
            recycle_pixel_buffer(data);
        }
    }
}

type Pixels = Arc<PixelBuffer>;

fn shared_pixels(data: Vec<u8>) -> Pixels {
    Arc::new(PixelBuffer::from_vec(data))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageKey {
    pub id: u32,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageDescriptor {
    pub key: ImageKey,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub image: ImageKey,
    pub placement_id: u32,
    pub line: i32,
    pub column: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub columns: u32,
    pub rows: u32,
    requested_columns: u32,
    requested_rows: u32,
    iterm2_layout: Option<Iterm2ImageLayout>,
    layout_columns: u32,
    layout_rows: u32,
    pub destination_width: u32,
    pub destination_height: u32,
    pub z: i32,
    pub virtual_placement: bool,
    pub parent_id: u32,
    pub parent_placement_id: u32,
    pub horizontal_offset: i32,
    pub vertical_offset: i32,
}

impl Placement {
    pub fn layer(&self) -> PlacementLayer {
        if self.z < BACKGROUND_Z_LIMIT {
            PlacementLayer::BelowBackground
        } else if self.z < 0 {
            PlacementLayer::BelowText
        } else {
            PlacementLayer::AboveText
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementLayer {
    BelowBackground,
    BelowText,
    AboveText,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ScrollParams {
    pub region_start: i32,
    pub region_end: i32,
    pub delta: i32,
    pub preserve_history: bool,
    pub has_margins: bool,
    pub cell_height: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub images: Vec<ImageDescriptor>,
    pub placements: Vec<Placement>,
}

impl Snapshot {
    /// Drop placements which cannot intersect the displayed terminal viewport.
    ///
    /// The graphics store keeps every placement required by scrollback.  Render snapshots only
    /// need the visible subset; retaining the rest needlessly sends their descriptors over IPC
    /// and submits clipped image instances to the renderer on every frame.
    pub fn retain_visible(&mut self, rows: usize, columns: usize) {
        let rows = i32::try_from(rows).unwrap_or(i32::MAX);
        let columns = u32::try_from(columns).unwrap_or(u32::MAX);
        self.placements.retain(|placement| {
            let placement_rows = i32::try_from(placement.rows.max(1)).unwrap_or(i32::MAX);
            let placement_bottom = placement.line.saturating_add(placement_rows);
            placement.line < rows && placement_bottom > 0 && placement.column < columns
        });

        let visible_images = self
            .placements
            .iter()
            .map(|placement| placement.image)
            .collect::<HashSet<_>>();
        self.images
            .retain(|image| visible_images.contains(&image.key));
    }
}

#[derive(Clone, Debug)]
pub struct VirtualCell {
    pub line: i32,
    pub column: u32,
    pub image_id_low: u32,
    pub placement_id: u32,
    pub diacritics: Vec<char>,
}

#[derive(Debug)]
struct Image {
    id: u32,
    number: u32,
    order: u64,
    generation: u64,
    width: u32,
    height: u32,
    pixels: Pixels,
    frames: Vec<AnimationFrame>,
    current_frame: usize,
    animation: Animation,
    transient: bool,
    last_used: u64,
}

#[derive(Debug)]
struct AnimationFrame {
    pixels: Pixels,
    gap_ms: i32,
    transient: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AnimationState {
    #[default]
    Stopped,
    Loading,
    Running,
}

#[derive(Debug, Default)]
struct Animation {
    state: AnimationState,
    loops_remaining: Option<u32>,
    next_frame_at: Option<Instant>,
}

#[derive(Debug)]
struct Screen {
    images: HashMap<u32, Image>,
    placements: Vec<Placement>,
    loading: Option<Loading>,
    next_image_id: u32,
    next_image_order: u64,
    next_placement_id: u32,
    clock: u64,
    storage_bytes: usize,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            images: HashMap::new(),
            placements: Vec::new(),
            loading: None,
            // Match Kitty/Ghostty's collision-avoidance range for automatically assigned IDs.
            next_image_id: 2_147_483_646,
            next_image_order: 0,
            next_placement_id: 0,
            clock: 0,
            storage_bytes: 0,
        }
    }
}

impl Screen {
    fn next_image_id(&mut self) -> u32 {
        loop {
            self.next_image_id = self.next_image_id.wrapping_add(1).max(1);
            if !self.images.contains_key(&self.next_image_id) {
                return self.next_image_id;
            }
        }
    }

    fn next_placement_id(&mut self) -> u32 {
        self.next_placement_id = self.next_placement_id.wrapping_add(1).max(1);
        self.next_placement_id
    }

    fn next_image_order(&mut self) -> u64 {
        self.next_image_order = self.next_image_order.wrapping_add(1).max(1);
        self.next_image_order
    }

    fn touch(&mut self, image_id: u32) {
        self.clock = self.clock.wrapping_add(1);
        if let Some(image) = self.images.get_mut(&image_id) {
            image.last_used = self.clock;
        }
    }

    fn remove_image(&mut self, image_id: u32) {
        if let Some(image) = self.images.remove(&image_id) {
            let bytes = image.frames.iter().map(|frame| frame.pixels.len()).sum();
            self.storage_bytes = self.storage_bytes.saturating_sub(bytes);
        }
        self.placements
            .retain(|placement| placement.image.id != image_id);
    }

    fn image_is_used(&self, image_id: u32) -> bool {
        self.placements
            .iter()
            .any(|placement| placement.image.id == image_id)
    }

    fn eviction_candidate(&self, replacing: Option<u32>) -> Option<u32> {
        self.images
            .values()
            .filter(|image| Some(image.id) != replacing)
            .min_by_key(|image| {
                // Kitty recommends four eviction buckets, then oldest-first within a bucket:
                // transient+unused, persistent+unused, transient+used, persistent+used.
                let used = self.image_is_used(image.id);
                let bucket = u8::from(!image.transient) + 2 * u8::from(used);
                (bucket, image.last_used)
            })
            .map(|image| image.id)
    }

    fn make_room(
        &mut self,
        bytes: usize,
        limit: usize,
        replacing: Option<u32>,
    ) -> Result<(), Error> {
        let replaced_bytes = replacing
            .and_then(|id| self.images.get(&id))
            .map_or(0, |image| {
                image.frames.iter().map(|frame| frame.pixels.len()).sum()
            });
        if bytes > limit {
            return Err(Error::OutOfMemory);
        }
        while self
            .storage_bytes
            .saturating_sub(replaced_bytes)
            .saturating_add(bytes)
            > limit
        {
            let candidate = self
                .eviction_candidate(replacing)
                .ok_or(Error::OutOfMemory)?;
            self.remove_image(candidate);
        }
        Ok(())
    }

    fn make_animation_room(
        &mut self,
        additional_bytes: usize,
        limit: usize,
        owner: u32,
    ) -> Result<(), Error> {
        if additional_bytes > limit {
            return Err(Error::OutOfMemory);
        }
        while self.storage_bytes.saturating_add(additional_bytes) > limit {
            let candidate = self
                .eviction_candidate(Some(owner))
                .ok_or(Error::OutOfMemory)?;
            self.remove_image(candidate);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Loading {
    transmission: Transmission,
    display: Option<Display>,
    frame: Option<FrameParameters>,
    quiet: Quiet,
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
enum PendingKind {
    Transmit {
        context: CommandContext,
        cursor_already_moved: bool,
    },
    AnimationFrame,
}

struct PendingDecode {
    alternate: bool,
    kind: PendingKind,
    quiet: Quiet,
    response_ids: (u32, u32, u32),
    result: PendingResult,
}

enum PendingResult {
    Background(Receiver<DecodeResult>),
    Ready(Option<DecodeResult>),
}

impl PendingResult {
    fn try_take(&mut self) -> Option<Result<DecodeResult, Error>> {
        match self {
            Self::Background(receiver) => match receiver.try_recv() {
                Ok(result) => Some(Ok(result)),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(Error::DecompressionFailed)),
            },
            Self::Ready(result) => result.take().map(Ok),
        }
    }

    fn wait(&mut self) -> Result<DecodeResult, Error> {
        match self {
            Self::Background(receiver) => receiver.recv().map_err(|_| Error::DecompressionFailed),
            Self::Ready(result) => result.take().ok_or(Error::DecompressionFailed),
        }
    }
}

struct DecodeResult {
    loading: Loading,
    decoded: Result<Decoded, Error>,
}

struct DecodeTask {
    loading: Loading,
    result: SyncSender<DecodeResult>,
}

struct DecodePool {
    sender: SyncSender<DecodeTask>,
}

impl DecodePool {
    fn shared() -> &'static Self {
        static POOL: OnceLock<DecodePool> = OnceLock::new();
        POOL.get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel::<DecodeTask>(DECODE_QUEUE_DEPTH);
            let receiver = Arc::new(Mutex::new(receiver));
            for worker in 0..DECODE_WORKERS {
                let receiver = Arc::clone(&receiver);
                thread::Builder::new()
                    .name(format!("kitty decode {worker}"))
                    .spawn(move || {
                        loop {
                            let task = {
                                let receiver =
                                    receiver.lock().expect("Kitty decode queue poisoned");
                                receiver.recv()
                            };
                            let Ok(mut task) = task else {
                                break;
                            };
                            let decoded = decode_loading(&mut task.loading);
                            let _ = task.result.send(DecodeResult {
                                loading: task.loading,
                                decoded,
                            });
                        }
                    })
                    .expect("failed to spawn Kitty decode worker");
            }
            DecodePool { sender }
        })
    }

    fn submit(&self, loading: Loading) -> Result<Receiver<DecodeResult>, Error> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(DecodeTask {
                loading,
                result: sender,
            })
            .map_err(|_| Error::DecompressionFailed)?;
        Ok(receiver)
    }
}

#[derive(Clone, Copy, Debug)]
struct FrameParameters {
    x: u32,
    y: u32,
    base_frame: u32,
    edit_frame: u32,
    gap_ms: i32,
    replace: bool,
    background: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quiet {
    No,
    Success,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PixelFormat {
    Rgb,
    Rgba,
    Png,
    /// Encoded raster image from iTerm2 OSC 1337.
    ///
    /// Kitty deliberately accepts only raw RGB(A) and PNG; this internal format keeps iTerm2's
    /// broader image contract on the existing asynchronous decode path without weakening the
    /// strict `f=100` PNG handling.
    EncodedImage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Iterm2Dimension {
    #[default]
    Auto,
    Cells(u32),
    Pixels(u32),
    Percent(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Iterm2ImageLayout {
    pub width: Iterm2Dimension,
    pub height: Iterm2Dimension,
    pub preserve_aspect_ratio: bool,
}

impl Default for Iterm2ImageLayout {
    fn default() -> Self {
        Self {
            width: Iterm2Dimension::Auto,
            height: Iterm2Dimension::Auto,
            preserve_aspect_ratio: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Medium {
    Direct,
    File,
    TemporaryFile,
    SharedMemory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Compression {
    None,
    Zlib,
}

#[derive(Clone, Copy, Debug)]
struct Transmission {
    format: PixelFormat,
    medium: Medium,
    width: u32,
    height: u32,
    size: u32,
    offset: u32,
    image_id: u32,
    image_number: u32,
    placement_id: u32,
    compression: Compression,
    more: bool,
    transient: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Display {
    image_id: u32,
    image_number: u32,
    placement_id: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    x_offset: u32,
    y_offset: u32,
    columns: u32,
    rows: u32,
    no_cursor_movement: bool,
    virtual_placement: bool,
    parent_id: u32,
    parent_placement_id: u32,
    horizontal_offset: i32,
    vertical_offset: i32,
    z: i32,
    iterm2_layout: Option<Iterm2ImageLayout>,
}

#[derive(Clone, Copy, Debug)]
pub struct CommandContext {
    pub line: i32,
    pub column: u32,
    pub columns: u32,
    pub rows: u32,
    pub cell_width: u32,
    pub cell_height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorMovement {
    pub start_column: u32,
    pub columns: u32,
    pub rows: u32,
}

#[derive(Debug, Default)]
pub struct Execution {
    pub response: Option<String>,
    pub cursor_movement: Option<CursorMovement>,
    pub changed: bool,
}

fn merge_execution(target: &mut Execution, mut next: Execution) {
    if let Some(response) = next.response.take() {
        target
            .response
            .get_or_insert_with(String::new)
            .push_str(&response);
    }
    target.changed |= next.changed;
    if next.cursor_movement.is_some() {
        target.cursor_movement = next.cursor_movement;
    }
}

pub struct Graphics {
    primary: Screen,
    alternate: Screen,
    generation: u64,
    storage_limit: usize,
    pending_decodes: VecDeque<PendingDecode>,
}

impl std::fmt::Debug for Graphics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Graphics")
            .field("primary", &self.primary)
            .field("alternate", &self.alternate)
            .field("generation", &self.generation)
            .field("storage_limit", &self.storage_limit)
            .field("pending_decodes", &self.pending_decodes.len())
            .finish()
    }
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            primary: Screen::default(),
            alternate: Screen::default(),
            generation: 0,
            storage_limit: DEFAULT_STORAGE_LIMIT,
            pending_decodes: VecDeque::new(),
        }
    }
}

impl Graphics {
    pub fn set_storage_limit(&mut self, limit: usize) {
        self.storage_limit = limit;
        for screen in [&mut self.primary, &mut self.alternate] {
            while screen.storage_bytes > limit {
                let Some(candidate) = screen.eviction_candidate(None) else {
                    break;
                };
                screen.remove_image(candidate);
            }
        }
    }

    pub fn next_animation_deadline(&self) -> Option<Instant> {
        [&self.primary, &self.alternate]
            .into_iter()
            .flat_map(|screen| screen.images.values())
            .filter_map(|image| image.animation.next_frame_at)
            .min()
    }

    pub fn advance_animations(&mut self, now: Instant) -> bool {
        let mut any_changed = false;
        for alternate in [false, true] {
            let due = self
                .screen(alternate)
                .images
                .values()
                .filter(|image| image.animation.next_frame_at.is_some_and(|at| at <= now))
                .map(|image| image.id)
                .collect::<Vec<_>>();
            for image_id in due {
                let mut changed = false;
                {
                    let image = self
                        .screen_mut(alternate)
                        .images
                        .get_mut(&image_id)
                        .expect("animation image exists");
                    // Gapless frames may advance several times in a single tick. Bound the loop
                    // by the frame count so malformed all-gapless animations cannot spin.
                    for _ in 0..image.frames.len().max(1) {
                        if image.animation.next_frame_at.is_none_or(|at| at > now) {
                            break;
                        }
                        let next = image.current_frame + 1;
                        if next < image.frames.len() {
                            image.current_frame = next;
                        } else {
                            match image.animation.state {
                                AnimationState::Stopped => {
                                    image.animation.next_frame_at = None;
                                    break;
                                }
                                AnimationState::Loading => {
                                    image.animation.next_frame_at = None;
                                    break;
                                }
                                AnimationState::Running => {
                                    if let Some(remaining) =
                                        image.animation.loops_remaining.as_mut()
                                    {
                                        if *remaining == 0 {
                                            image.animation.state = AnimationState::Stopped;
                                            image.animation.next_frame_at = None;
                                            break;
                                        }
                                        *remaining -= 1;
                                    }
                                    image.current_frame = 0;
                                }
                            }
                        }
                        image.pixels = Arc::clone(&image.frames[image.current_frame].pixels);
                        image.transient = image.frames[image.current_frame].transient;
                        changed = true;
                        schedule_animation(image, now);
                    }
                }
                if changed {
                    self.refresh_image_generation(alternate, image_id);
                    any_changed = true;
                }
            }
        }
        any_changed
    }

    fn screen(&self, alternate: bool) -> &Screen {
        if alternate {
            &self.alternate
        } else {
            &self.primary
        }
    }

    fn screen_mut(&mut self, alternate: bool) -> &mut Screen {
        if alternate {
            &mut self.alternate
        } else {
            &mut self.primary
        }
    }

    pub fn clear_alternate(&mut self) {
        self.alternate = Screen::default();
    }

    pub fn erase_display(&mut self, alternate: bool) {
        let screen = self.screen_mut(alternate);
        screen
            .placements
            .retain(|placement| placement.virtual_placement);
        let unused = screen
            .images
            .keys()
            .copied()
            .filter(|image_id| !screen.image_is_used(*image_id))
            .collect::<Vec<_>>();
        for image_id in unused {
            screen.remove_image(image_id);
        }
    }

    pub fn reset(&mut self) {
        self.primary = Screen::default();
        self.alternate = Screen::default();
    }

    pub fn image(&self, alternate: bool, key: ImageKey) -> Option<Arc<PixelBuffer>> {
        self.screen(alternate)
            .images
            .get(&key.id)
            .filter(|image| image.generation == key.generation)
            .map(|image| Arc::clone(&image.pixels))
    }

    pub fn image_with_metadata(
        &self,
        alternate: bool,
        key: ImageKey,
    ) -> Option<(ImageDescriptor, Arc<PixelBuffer>)> {
        self.screen(alternate)
            .images
            .get(&key.id)
            .filter(|image| image.generation == key.generation)
            .map(|image| {
                (
                    ImageDescriptor {
                        key,
                        width: image.width,
                        height: image.height,
                    },
                    Arc::clone(&image.pixels),
                )
            })
    }

    pub fn snapshot(&self, alternate: bool, display_offset: usize) -> Snapshot {
        let screen = self.screen(alternate);
        let mut placements = screen
            .placements
            .iter()
            .enumerate()
            .map(|(ref_order, placement)| {
                let mut placement = placement.clone();
                placement.line += display_offset as i32;
                let image_order = screen
                    .images
                    .get(&placement.image.id)
                    .map_or(u64::MAX, |image| image.order);
                (placement.z, image_order, ref_order, placement)
            })
            .collect::<Vec<_>>();
        placements
            .sort_unstable_by_key(|(z, image_order, ref_order, _)| (*z, *image_order, *ref_order));
        let placements = placements
            .into_iter()
            .map(|(_, _, _, placement)| placement)
            .collect();
        let mut images = screen
            .images
            .values()
            .map(|image| {
                (
                    image.order,
                    ImageDescriptor {
                        key: ImageKey {
                            id: image.id,
                            generation: image.generation,
                        },
                        width: image.width,
                        height: image.height,
                    },
                )
            })
            .collect::<Vec<_>>();
        images.sort_unstable_by_key(|(order, _)| *order);
        let images = images.into_iter().map(|(_, image)| image).collect();
        Snapshot { images, placements }
    }

    /// Snapshot only placements that can affect the current viewport.
    ///
    /// Scrollback can retain many image references. Filtering before cloning and sorting keeps
    /// interactive scrolling proportional to the number of visible images instead of the total
    /// number of historical placements.
    pub fn snapshot_visible(
        &self,
        alternate: bool,
        display_offset: usize,
        rows: usize,
        columns: usize,
    ) -> Snapshot {
        let screen = self.screen(alternate);
        let display_offset = i32::try_from(display_offset).unwrap_or(i32::MAX);
        let rows = i32::try_from(rows).unwrap_or(i32::MAX);
        let columns = u32::try_from(columns).unwrap_or(u32::MAX);
        let mut referenced_images = HashSet::new();
        let mut placements = screen
            .placements
            .iter()
            .enumerate()
            .filter_map(|(ref_order, placement)| {
                let line = placement.line.saturating_add(display_offset);
                let placement_rows = i32::try_from(placement.rows.max(1)).unwrap_or(i32::MAX);
                let visible = placement.virtual_placement
                    || line < rows
                        && line.saturating_add(placement_rows) > 0
                        && placement.column < columns;
                if !visible {
                    return None;
                }

                referenced_images.insert(placement.image);
                let mut placement = placement.clone();
                placement.line = line;
                let image_order = screen
                    .images
                    .get(&placement.image.id)
                    .map_or(u64::MAX, |image| image.order);
                Some((placement.z, image_order, ref_order, placement))
            })
            .collect::<Vec<_>>();
        placements
            .sort_unstable_by_key(|(z, image_order, ref_order, _)| (*z, *image_order, *ref_order));
        let placements = placements
            .into_iter()
            .map(|(_, _, _, placement)| placement)
            .collect();
        let mut images = screen
            .images
            .values()
            .filter(|image| {
                referenced_images.contains(&ImageKey {
                    id: image.id,
                    generation: image.generation,
                })
            })
            .map(|image| {
                (
                    image.order,
                    ImageDescriptor {
                        key: ImageKey {
                            id: image.id,
                            generation: image.generation,
                        },
                        width: image.width,
                        height: image.height,
                    },
                )
            })
            .collect::<Vec<_>>();
        images.sort_unstable_by_key(|(order, _)| *order);
        let images = images.into_iter().map(|(_, image)| image).collect();
        Snapshot { images, placements }
    }

    pub fn resolve_virtual_placeholders(
        &self,
        alternate: bool,
        snapshot: &mut Snapshot,
        cells: &[VirtualCell],
        cell_width: u32,
        cell_height: u32,
    ) {
        let screen = self.screen(alternate);
        snapshot
            .placements
            .retain(|placement| !placement.virtual_placement);
        let mut run: Option<VirtualRun> = None;
        for cell in cells {
            let decoded = decode_virtual_cell(cell);
            if let Some(existing) = run.as_mut()
                && existing.can_append(cell, &decoded)
            {
                existing.width += 1;
                continue;
            }
            if let Some(run) = run.take() {
                append_virtual_run(screen, snapshot, run, cell_width, cell_height);
            }
            run = Some(VirtualRun {
                line: cell.line,
                column: cell.column,
                image_id: (decoded.image_id_low & 0x00ff_ffff)
                    | (u32::from(decoded.image_id_high.unwrap_or(0)) << 24),
                placement_id: decoded.placement_id,
                image_row: decoded.row.unwrap_or(0),
                image_column: decoded.column.unwrap_or(0),
                width: 1,
            });
        }
        if let Some(run) = run {
            append_virtual_run(screen, snapshot, run, cell_width, cell_height);
        }
        snapshot.placements.sort_by_key(|placement| {
            let image_order = screen
                .images
                .get(&placement.image.id)
                .map_or(u64::MAX, |image| image.order);
            (placement.z, image_order)
        });
    }

    pub(super) fn scroll(&mut self, alternate: bool, params: ScrollParams) {
        let ScrollParams {
            region_start,
            region_end,
            delta,
            preserve_history,
            has_margins,
            cell_height,
        } = params;
        if delta == 0 {
            return;
        }
        let screen = self.screen_mut(alternate);
        if !has_margins {
            // Full-screen scrolling moves every image reference, including references which have
            // already entered negative scrollback coordinates. Restricting this to the visible
            // region pins an image at -1 after it first leaves the screen; adding display_offset
            // while browsing history then moves it independently of its surrounding text.
            for placement in &mut screen.placements {
                if !placement.virtual_placement {
                    placement.line = placement.line.saturating_add(delta);
                }
            }
            if !preserve_history {
                screen.placements.retain(|placement| {
                    placement.virtual_placement
                        || placement.line.saturating_add(placement.rows.max(1) as i32)
                            > region_start
                });
            }
            return;
        }

        // Kitty's margin rule moves only images initially contained by the page area. Images
        // crossing a margin are left untouched; contained images which cross a margin because of
        // the scroll are clipped permanently at that boundary.
        let cell_height = cell_height.max(1);
        screen.placements.retain_mut(|placement| {
            if placement.virtual_placement {
                return true;
            }
            let rows = i32::try_from(placement.rows.max(1)).unwrap_or(i32::MAX);
            let bottom = placement.line.saturating_add(rows);
            if placement.line < region_start || bottom > region_end {
                return true;
            }

            placement.line = placement.line.saturating_add(delta);
            if placement.line >= region_end
                || placement
                    .line
                    .saturating_add(i32::try_from(placement.rows).unwrap_or(i32::MAX))
                    <= region_start
            {
                return false;
            }
            clip_placement_to_rows(placement, region_start, region_end, cell_height)
        });
    }

    /// Return the physical placement anchors which must be carried through a grid reflow.
    pub(super) fn reflow_anchors(&self, alternate: bool) -> Vec<Option<(i32, u32)>> {
        self.screen(alternate)
            .placements
            .iter()
            .map(|placement| {
                (!placement.virtual_placement).then_some((placement.line, placement.column))
            })
            .collect()
    }

    /// Apply coordinates recovered from the resized grid and discard anchors pruned with history.
    pub(super) fn apply_reflow_anchors(
        &mut self,
        alternate: bool,
        anchors: Vec<Option<(i32, u32)>>,
    ) {
        let mut index = 0;
        self.screen_mut(alternate)
            .placements
            .retain_mut(|placement| {
                let anchor = anchors.get(index).copied().flatten();
                index += 1;
                if placement.virtual_placement {
                    return true;
                }
                let Some((line, column)) = anchor else {
                    return false;
                };
                placement.line = line;
                placement.column = column;
                true
            });
    }

    /// Recompute cell-sized placements after the renderer's physical cell dimensions change.
    pub fn rescale(&mut self, cell_width: u32, cell_height: u32) {
        let cell_width = cell_width.max(1);
        let cell_height = cell_height.max(1);
        for screen in [&mut self.primary, &mut self.alternate] {
            for placement in &mut screen.placements {
                if placement.virtual_placement {
                    continue;
                }
                placement.x_offset = placement.x_offset.min(cell_width - 1);
                placement.y_offset = placement.y_offset.min(cell_height - 1);
                let display = Display {
                    columns: placement.requested_columns,
                    rows: placement.requested_rows,
                    x_offset: placement.x_offset,
                    y_offset: placement.y_offset,
                    iterm2_layout: placement.iterm2_layout,
                    ..Display::default()
                };
                let context = CommandContext {
                    line: placement.line,
                    column: placement.column,
                    columns: placement.layout_columns.max(1),
                    rows: placement.layout_rows.max(1),
                    cell_width,
                    cell_height,
                };
                if let Some(layout) = placement.iterm2_layout {
                    let resolved = resolve_iterm2_layout(
                        placement.source_width,
                        placement.source_height,
                        layout,
                        context,
                    );
                    placement.destination_width = resolved.destination_width;
                    placement.destination_height = resolved.destination_height;
                    placement.x_offset = resolved.x_padding;
                    placement.y_offset = resolved.y_padding;
                    placement.columns = resolved.box_width.div_ceil(cell_width).max(1);
                    placement.rows = resolved.box_height.div_ceil(cell_height).max(1);
                } else {
                    (placement.destination_width, placement.destination_height) =
                        placement_pixel_size(
                            placement.source_width,
                            placement.source_height,
                            display,
                            context,
                        );
                    placement.columns = placement
                        .destination_width
                        .saturating_add(placement.x_offset)
                        .div_ceil(cell_width)
                        .max(1);
                    placement.rows = placement
                        .destination_height
                        .saturating_add(placement.y_offset)
                        .div_ceil(cell_height)
                        .max(1);
                }
            }
        }
    }

    pub fn prune_before(&mut self, alternate: bool, minimum_line: i32) {
        self.screen_mut(alternate).placements.retain(|placement| {
            placement.virtual_placement
                || placement.line.saturating_add(placement.rows.max(1) as i32) > minimum_line
        });
    }

    /// Commit every completed decode in submission order.
    ///
    /// Workers may finish out of order, but terminal image IDs, animation frame indices and
    /// protocol responses are observable state. Waiting on the queue head preserves those
    /// semantics while still allowing the expensive decompression work to overlap.
    pub fn flush_pending(&mut self) -> Execution {
        let mut execution = Execution::default();
        while !self.pending_decodes.is_empty() {
            merge_execution(&mut execution, self.flush_one_pending());
        }
        execution
    }

    pub fn has_pending(&self) -> bool {
        !self.pending_decodes.is_empty()
    }

    /// Commit the ready prefix without blocking the PTY parser.
    pub fn flush_ready(&mut self) -> Execution {
        let mut execution = Execution::default();
        while let Some(pending) = self.pending_decodes.front_mut() {
            let Some(result) = pending.result.try_take() else {
                break;
            };
            let pending = self
                .pending_decodes
                .pop_front()
                .expect("pending decode queue head exists");
            merge_execution(&mut execution, self.finish_pending(pending, result));
        }
        execution
    }

    fn flush_one_pending(&mut self) -> Execution {
        let Some(mut pending) = self.pending_decodes.pop_front() else {
            return Execution::default();
        };
        let wait_started = Instant::now();
        let result = pending.result.wait();
        pipeline_metrics::record(PipelineStage::DecodeWait, wait_started.elapsed());
        self.finish_pending(pending, result)
    }

    fn finish_pending(
        &mut self,
        pending: PendingDecode,
        result: Result<DecodeResult, Error>,
    ) -> Execution {
        let commit_started = Instant::now();
        let result = result.and_then(|result| {
            let decoded = result.decoded?;
            match pending.kind {
                PendingKind::Transmit {
                    context,
                    cursor_already_moved,
                } => {
                    let mut execution =
                        self.commit_transmit(pending.alternate, result.loading, context, decoded)?;
                    if cursor_already_moved {
                        execution.cursor_movement = None;
                    }
                    Ok(execution)
                }
                PendingKind::AnimationFrame => {
                    self.commit_frame(pending.alternate, result.loading, decoded)
                }
            }
        });
        pipeline_metrics::record(PipelineStage::Commit, commit_started.elapsed());
        match result {
            Ok(mut execution) => {
                if pending.quiet != Quiet::No {
                    execution.response = None;
                }
                execution
            }
            Err(error) => Execution {
                response: error.response(
                    pending.response_ids.0,
                    pending.response_ids.1,
                    pending.response_ids.2,
                    pending.quiet,
                ),
                ..Execution::default()
            },
        }
    }

    fn queue_decode(
        &mut self,
        alternate: bool,
        kind: PendingKind,
        mut loading: Loading,
        background: bool,
    ) -> Result<Execution, Error> {
        let mut completed = Execution::default();
        if self.pending_decodes.len() >= MAX_PENDING_DECODES {
            merge_execution(&mut completed, self.flush_one_pending());
        }
        let quiet = loading.quiet;
        let response_ids = (
            loading.transmission.image_id,
            loading.transmission.image_number,
            loading.transmission.placement_id,
        );
        let result = if background {
            PendingResult::Background(DecodePool::shared().submit(loading)?)
        } else {
            let decoded = decode_loading(&mut loading);
            PendingResult::Ready(Some(DecodeResult { loading, decoded }))
        };
        self.pending_decodes.push_back(PendingDecode {
            alternate,
            kind,
            quiet,
            response_ids,
            result,
        });
        pipeline_metrics::record_pending(self.pending_decodes.len());
        Ok(completed)
    }

    pub fn execute(
        &mut self,
        alternate: bool,
        payload: &[u8],
        context: CommandContext,
    ) -> Execution {
        if self.storage_limit == 0 {
            return Execution::default();
        }
        let parsed = match Command::parse(payload) {
            Ok(command) => command,
            Err(error) => {
                let mut execution = self.flush_pending();
                merge_execution(
                    &mut execution,
                    Execution {
                        response: error.response(0, 0, 0, Quiet::No),
                        ..Execution::default()
                    },
                );
                return execution;
            }
        };
        self.execute_parsed(alternate, parsed, context)
    }

    pub(super) fn execute_iterm2_image(
        &mut self,
        alternate: bool,
        encoded: &[u8],
        layout: Iterm2ImageLayout,
        context: CommandContext,
    ) -> Execution {
        if self.storage_limit == 0 {
            return Execution::default();
        }
        let transmission = Transmission {
            format: if encoded.starts_with(b"iVBORw0KGgo") {
                PixelFormat::Png
            } else {
                PixelFormat::EncodedImage
            },
            medium: Medium::Direct,
            width: 0,
            height: 0,
            size: 0,
            offset: 0,
            image_id: 0,
            image_number: 0,
            placement_id: 0,
            compression: Compression::None,
            more: false,
            transient: false,
        };
        let command = Command {
            action: Action::TransmitAndDisplay,
            quiet: Quiet::All,
            params: Params::default(),
            encoded,
            transmission_override: Some(transmission),
            display_override: Some(Display {
                iterm2_layout: Some(layout),
                ..Display::default()
            }),
        };
        self.execute_parsed(alternate, command, context)
    }

    fn execute_parsed(
        &mut self,
        alternate: bool,
        parsed: Command<'_>,
        context: CommandContext,
    ) -> Execution {
        let quiet = if parsed.quiet == Quiet::No {
            self.screen(alternate)
                .loading
                .as_ref()
                .map_or(Quiet::No, |loading| loading.quiet)
        } else {
            parsed.quiet
        };
        let context = CommandContext {
            cell_width: context.cell_width.max(1),
            cell_height: context.cell_height.max(1),
            ..context
        };
        let response_ids = (
            parsed.image_id(),
            parsed.image_number(),
            parsed.placement_id(),
        );
        let transmission = self
            .screen(alternate)
            .loading
            .as_ref()
            .map(|loading| loading.transmission)
            .or_else(|| parsed.transmission().ok());
        let expensive_decode = transmission.is_some_and(|transmission| {
            transmission.compression == Compression::Zlib
                || matches!(
                    transmission.format,
                    PixelFormat::Png | PixelFormat::EncodedImage
                )
        });
        let display = parsed.display();
        let eager_cursor_movement = (parsed.action == Action::TransmitAndDisplay)
            .then(|| eager_cursor_movement(display, context))
            .flatten();
        let queueable_display = parsed.action == Action::TransmitAndDisplay
            && expensive_decode
            && self.screen(alternate).loading.is_none()
            && (display.no_cursor_movement
                || display.virtual_placement
                || eager_cursor_movement.is_some()
                || display.iterm2_layout.is_some());
        let queueable = match parsed.action {
            Action::Transmit => self
                .screen(alternate)
                .loading
                .as_ref()
                .is_none_or(|loading| loading.display.is_none()),
            Action::TransmitAndDisplay => queueable_display,
            Action::TransmitAnimationFrame => true,
            _ => false,
        };
        let mut completed = if queueable {
            Execution::default()
        } else {
            self.flush_pending()
        };
        let result = match parsed.action {
            Action::Transmit | Action::TransmitAndDisplay if queueable => self.queue_transmit(
                alternate,
                parsed,
                context,
                expensive_decode,
                eager_cursor_movement,
            ),
            Action::TransmitAnimationFrame => self.queue_frame(alternate, parsed, expensive_decode),
            _ => self.execute_command(alternate, parsed, context),
        };
        let current = match result {
            Ok(mut execution) => {
                if quiet != Quiet::No {
                    execution.response = None;
                }
                execution
            }
            Err(error) => Execution {
                response: error.response(response_ids.0, response_ids.1, response_ids.2, quiet),
                ..Execution::default()
            },
        };
        merge_execution(&mut completed, current);
        completed
    }

    fn execute_command(
        &mut self,
        alternate: bool,
        command: Command<'_>,
        context: CommandContext,
    ) -> Result<Execution, Error> {
        match command.action {
            Action::Query => {
                let transmission = command.transmission()?;
                if transmission.image_id == 0 {
                    return Err(Error::ImageIdRequired);
                }
                let mut loading = Self::start_loading(command, transmission)?;
                if transmission.more {
                    return Err(Error::InvalidData);
                }
                let _ = decode_loading(&mut loading)?;
                Ok(Execution {
                    response: response(
                        transmission.image_id,
                        transmission.image_number,
                        transmission.placement_id,
                        "OK",
                    ),
                    ..Execution::default()
                })
            }
            Action::Transmit | Action::TransmitAndDisplay => {
                self.transmit(alternate, command, context)
            }
            Action::TransmitAnimationFrame => self.transmit_frame(alternate, command),
            Action::ControlAnimation => self.control_animation(alternate, command),
            Action::Display => {
                let display = command.display();
                self.display(alternate, display, context)
            }
            Action::Delete => {
                self.delete(alternate, command.params, context);
                Ok(Execution {
                    changed: true,
                    ..Execution::default()
                })
            }
            Action::ComposeAnimation => Err(Error::UnimplementedAction),
        }
    }

    fn start_loading(command: Command<'_>, transmission: Transmission) -> Result<Loading, Error> {
        if transmission.image_id != 0 && transmission.image_number != 0 {
            return Err(Error::MutuallyExclusiveIds);
        }
        let initial_capacity = if transmission.medium == Medium::Direct
            && transmission.compression == Compression::None
        {
            let channels = match transmission.format {
                PixelFormat::Rgb => Some(3usize),
                PixelFormat::Rgba => Some(4usize),
                PixelFormat::Png | PixelFormat::EncodedImage => None,
            };
            channels
                .and_then(|channels| {
                    usize::try_from(transmission.width)
                        .ok()?
                        .checked_mul(usize::try_from(transmission.height).ok()?)?
                        .checked_mul(channels)
                })
                .filter(|capacity| *capacity <= MAX_IMAGE_BYTES)
                .unwrap_or_else(|| decoded_capacity(command.encoded.len()))
        } else {
            decoded_capacity(command.encoded.len())
        };
        let mut decoded = take_pixel_buffer(initial_capacity);
        append_base64(&mut decoded, command.encoded)?;
        let data = load_medium(transmission, decoded)?;
        Ok(Loading {
            transmission,
            display: (command.action == Action::TransmitAndDisplay).then(|| command.display()),
            frame: (command.action == Action::TransmitAnimationFrame)
                .then(|| command.frame_parameters()),
            quiet: command.quiet,
            data,
        })
    }

    fn transmit(
        &mut self,
        alternate: bool,
        command: Command<'_>,
        context: CommandContext,
    ) -> Result<Execution, Error> {
        let transmission = command.transmission()?;
        let existing = self.screen_mut(alternate).loading.take();
        let mut loading = if let Some(mut loading) = existing {
            append_base64(&mut loading.data, command.encoded)?;
            if command.quiet != Quiet::No {
                loading.quiet = command.quiet;
            }
            loading
        } else {
            Self::start_loading(command, transmission)?
        };
        if transmission.more {
            self.screen_mut(alternate).loading = Some(loading);
            return Ok(Execution::default());
        }

        let decoded = decode_loading(&mut loading)?;
        self.commit_transmit(alternate, loading, context, decoded)
    }

    fn queue_transmit(
        &mut self,
        alternate: bool,
        command: Command<'_>,
        context: CommandContext,
        background: bool,
        eager_cursor_movement: Option<CursorMovement>,
    ) -> Result<Execution, Error> {
        let transmission = command.transmission()?;
        let existing = self.screen_mut(alternate).loading.take();
        let loading = if let Some(mut loading) = existing {
            append_base64(&mut loading.data, command.encoded)?;
            if command.quiet != Quiet::No {
                loading.quiet = command.quiet;
            }
            loading
        } else {
            Self::start_loading(command, transmission)?
        };
        if transmission.more {
            self.screen_mut(alternate).loading = Some(loading);
            return Ok(Execution::default());
        }
        // Auto-sized iTerm2 images need the intrinsic dimensions before cursor movement can be
        // determined. Reading only the compressed image header here keeps cursor state
        // synchronous with the escape sequence while leaving full pixel decoding to the worker.
        let eager_cursor_movement = eager_cursor_movement
            .or_else(|| iterm2_cursor_movement_from_loading(&loading, context));
        let mut execution = self.queue_decode(
            alternate,
            PendingKind::Transmit {
                context,
                cursor_already_moved: eager_cursor_movement.is_some(),
            },
            loading,
            background,
        )?;
        execution.cursor_movement = eager_cursor_movement;
        Ok(execution)
    }

    fn commit_transmit(
        &mut self,
        alternate: bool,
        loading: Loading,
        context: CommandContext,
        decoded: Decoded,
    ) -> Result<Execution, Error> {
        let mut image_id = loading.transmission.image_id;
        if image_id == 0 {
            // Image numbers identify the newest matching transmission; they do not cause a
            // later transmission to overwrite the previous image with that number.
            image_id = self.screen_mut(alternate).next_image_id();
        }
        let storage_limit = self.storage_limit;
        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        let screen = self.screen_mut(alternate);
        screen.make_room(decoded.pixels.len(), storage_limit, Some(image_id))?;
        let image_order = if let Some(image) = screen.images.get(&image_id) {
            image.order
        } else {
            screen.next_image_order()
        };
        let replaced = screen.images.insert(
            image_id,
            Image {
                id: image_id,
                number: loading.transmission.image_number,
                order: image_order,
                generation,
                width: decoded.width,
                height: decoded.height,
                pixels: Arc::clone(&decoded.pixels),
                frames: vec![AnimationFrame {
                    pixels: decoded.pixels,
                    gap_ms: 0,
                    transient: loading.transmission.transient,
                }],
                current_frame: 0,
                animation: Animation::default(),
                transient: loading.transmission.transient,
                last_used: screen.clock,
            },
        );
        if let Some(replaced) = replaced {
            let replaced_bytes = replaced.frames.iter().map(|frame| frame.pixels.len()).sum();
            screen.storage_bytes = screen.storage_bytes.saturating_sub(replaced_bytes);
        }
        screen.storage_bytes = screen.storage_bytes.saturating_add(decoded.byte_len);
        for placement in &mut screen.placements {
            if placement.image.id == image_id {
                placement.image.generation = generation;
            }
        }
        screen.touch(image_id);

        let mut result = Execution {
            changed: true,
            response: if loading.transmission.image_id != 0
                || loading.transmission.image_number != 0
            {
                response(
                    image_id,
                    loading.transmission.image_number,
                    loading.transmission.placement_id,
                    "OK",
                )
            } else {
                None
            },
            ..Execution::default()
        };
        if let Some(mut display) = loading.display {
            display.image_id = image_id;
            let displayed = self.display(alternate, display, context)?;
            result.cursor_movement = displayed.cursor_movement;
            result.response = displayed.response;
        }
        Ok(result)
    }

    fn transmit_frame(
        &mut self,
        alternate: bool,
        command: Command<'_>,
    ) -> Result<Execution, Error> {
        let transmission = command.transmission()?;
        let existing = self.screen_mut(alternate).loading.take();
        let mut loading = if let Some(mut loading) = existing {
            if loading.frame.is_none() {
                return Err(Error::InvalidData);
            }
            append_base64(&mut loading.data, command.encoded)?;
            if command.quiet != Quiet::No {
                loading.quiet = command.quiet;
            }
            loading
        } else {
            Self::start_loading(command, transmission)?
        };
        if transmission.more {
            self.screen_mut(alternate).loading = Some(loading);
            return Ok(Execution::default());
        }

        let decoded = decode_loading(&mut loading)?;
        self.commit_frame(alternate, loading, decoded)
    }

    fn queue_frame(
        &mut self,
        alternate: bool,
        command: Command<'_>,
        background: bool,
    ) -> Result<Execution, Error> {
        let transmission = command.transmission()?;
        let existing = self.screen_mut(alternate).loading.take();
        let loading = if let Some(mut loading) = existing {
            if loading.frame.is_none() {
                return Err(Error::InvalidData);
            }
            append_base64(&mut loading.data, command.encoded)?;
            if command.quiet != Quiet::No {
                loading.quiet = command.quiet;
            }
            loading
        } else {
            Self::start_loading(command, transmission)?
        };
        if transmission.more {
            self.screen_mut(alternate).loading = Some(loading);
            return Ok(Execution::default());
        }
        self.queue_decode(alternate, PendingKind::AnimationFrame, loading, background)
    }

    fn commit_frame(
        &mut self,
        alternate: bool,
        loading: Loading,
        decoded: Decoded,
    ) -> Result<Execution, Error> {
        let frame = loading.frame.ok_or(Error::InvalidData)?;
        let storage_limit = self.storage_limit;
        let screen = self.screen_mut(alternate);
        let image_id = resolve_image_id(
            screen,
            loading.transmission.image_id,
            loading.transmission.image_number,
        )?;
        let image = screen.images.get(&image_id).ok_or(Error::ImageNotFound)?;
        let width = image.width;
        let height = image.height;
        if frame.x.saturating_add(decoded.width) > width
            || frame.y.saturating_add(decoded.height) > height
        {
            return Err(Error::InvalidData);
        }
        let target_index = frame
            .edit_frame
            .checked_sub(1)
            .and_then(|index| usize::try_from(index).ok());
        if target_index.is_some_and(|index| index >= image.frames.len()) {
            return Err(Error::ImageNotFound);
        }
        let base_pixels = if let Some(index) = target_index {
            Arc::clone(&image.frames[index].pixels)
        } else if frame.base_frame != 0 {
            let index = usize::try_from(frame.base_frame - 1).map_err(|_| Error::ImageNotFound)?;
            Arc::clone(&image.frames.get(index).ok_or(Error::ImageNotFound)?.pixels)
        } else {
            solid_frame(width, height, frame.background)?
        };
        let pixels = compose_frame(
            &base_pixels,
            width,
            height,
            &decoded.pixels,
            decoded.width,
            decoded.height,
            frame.x,
            frame.y,
            frame.replace,
        )?;
        let frame_bytes = pixels.len();
        let additional = target_index.map_or(frame_bytes, |index| {
            frame_bytes.saturating_sub(image.frames[index].pixels.len())
        });
        screen.make_animation_room(additional, storage_limit, image_id)?;

        let mut current_changed = false;
        let image = screen
            .images
            .get_mut(&image_id)
            .ok_or(Error::ImageNotFound)?;
        let new_frame = AnimationFrame {
            pixels: shared_pixels(pixels),
            gap_ms: if frame.gap_ms == 0 { 40 } else { frame.gap_ms },
            transient: loading.transmission.transient,
        };
        if let Some(index) = target_index {
            let old_len = image.frames[index].pixels.len();
            let gap_ms = if frame.gap_ms == 0 {
                image.frames[index].gap_ms
            } else {
                frame.gap_ms
            };
            image.frames[index] = AnimationFrame {
                gap_ms,
                ..new_frame
            };
            screen.storage_bytes = screen.storage_bytes.saturating_sub(old_len);
            screen.storage_bytes = screen.storage_bytes.saturating_add(frame_bytes);
            if image.current_frame == index {
                image.pixels = Arc::clone(&image.frames[index].pixels);
                image.transient = image.frames[index].transient;
                current_changed = true;
            }
        } else {
            image.frames.push(new_frame);
            screen.storage_bytes = screen.storage_bytes.saturating_add(frame_bytes);
            if image.animation.state == AnimationState::Loading
                && image.current_frame + 1 == image.frames.len() - 1
            {
                schedule_animation(image, Instant::now());
            }
        }
        screen.touch(image_id);
        if current_changed {
            self.refresh_image_generation(alternate, image_id);
        }
        Ok(Execution {
            response: response(
                image_id,
                loading.transmission.image_number,
                loading.transmission.placement_id,
                "OK",
            ),
            changed: current_changed,
            ..Execution::default()
        })
    }

    fn control_animation(
        &mut self,
        alternate: bool,
        command: Command<'_>,
    ) -> Result<Execution, Error> {
        let image_id = {
            let screen = self.screen(alternate);
            resolve_image_id(screen, command.image_id(), command.image_number())?
        };
        let now = Instant::now();
        let mut current_changed = false;
        {
            let image = self
                .screen_mut(alternate)
                .images
                .get_mut(&image_id)
                .ok_or(Error::ImageNotFound)?;
            if let Some(frame_number) = command.params.u32(b'c').filter(|frame| *frame != 0) {
                let index = usize::try_from(frame_number - 1).map_err(|_| Error::ImageNotFound)?;
                let frame = image.frames.get(index).ok_or(Error::ImageNotFound)?;
                image.current_frame = index;
                image.pixels = Arc::clone(&frame.pixels);
                image.transient = frame.transient;
                current_changed = true;
            }
            if let Some(frame_number) = command.params.u32(b'r').filter(|frame| *frame != 0) {
                let index = usize::try_from(frame_number - 1).map_err(|_| Error::ImageNotFound)?;
                let frame = image.frames.get_mut(index).ok_or(Error::ImageNotFound)?;
                if let Some(gap) = command.params.i32(b'z').filter(|gap| *gap != 0) {
                    frame.gap_ms = gap;
                }
            }
            if let Some(loops) = command.params.u32(b'v').filter(|loops| *loops != 0) {
                image.animation.loops_remaining = (loops != 1).then_some(loops - 1);
            }
            if let Some(state) = command.params.u32(b's').filter(|state| *state != 0) {
                image.animation.state = match state {
                    1 => AnimationState::Stopped,
                    2 => AnimationState::Loading,
                    3 => AnimationState::Running,
                    _ => return Err(Error::InvalidData),
                };
                if image.animation.state == AnimationState::Stopped {
                    image.animation.next_frame_at = None;
                    image.animation.loops_remaining = None;
                }
            }
            schedule_animation(image, now);
        }
        if current_changed {
            self.refresh_image_generation(alternate, image_id);
        }
        Ok(Execution {
            response: response(
                image_id,
                command.image_number(),
                command.placement_id(),
                "OK",
            ),
            changed: current_changed,
            ..Execution::default()
        })
    }

    fn refresh_image_generation(&mut self, alternate: bool, image_id: u32) {
        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        let screen = self.screen_mut(alternate);
        if let Some(image) = screen.images.get_mut(&image_id) {
            image.generation = generation;
        }
        for placement in &mut screen.placements {
            if placement.image.id == image_id {
                placement.image.generation = generation;
            }
        }
    }

    fn display(
        &mut self,
        alternate: bool,
        mut display: Display,
        context: CommandContext,
    ) -> Result<Execution, Error> {
        if display.image_id == 0 && display.image_number == 0 {
            return Err(Error::ImageIdOrNumberRequired);
        }
        if display.virtual_placement && display.parent_id != 0 {
            return Err(Error::VirtualPlacementHasParent);
        }
        let screen = self.screen_mut(alternate);
        let image_id = if display.image_id != 0 {
            display.image_id
        } else {
            screen
                .images
                .values()
                .filter(|image| image.number == display.image_number)
                .max_by_key(|image| image.generation)
                .map(|image| image.id)
                .ok_or(Error::ImageNotFound)?
        };
        let (image_width, image_height, image_generation) = screen
            .images
            .get(&image_id)
            .map(|image| (image.width, image.height, image.generation))
            .ok_or(Error::ImageNotFound)?;
        let source_x = display.x.min(image_width);
        let source_y = display.y.min(image_height);
        let source_width = if display.width == 0 {
            image_width.saturating_sub(source_x)
        } else {
            display.width.min(image_width.saturating_sub(source_x))
        };
        let source_height = if display.height == 0 {
            image_height.saturating_sub(source_y)
        } else {
            display.height.min(image_height.saturating_sub(source_y))
        };
        if source_width == 0 || source_height == 0 {
            return Err(Error::InvalidData);
        }
        // Kitty clamps cell offsets to the last pixel in the cell. Apart from matching the
        // reference implementation, this keeps malformed clients from turning an offset into a
        // destination rectangle extending past the explicitly requested cell box.
        display.x_offset = display.x_offset.min(context.cell_width.saturating_sub(1));
        display.y_offset = display.y_offset.min(context.cell_height.saturating_sub(1));
        let (destination_width, destination_height, occupied_columns, occupied_rows) =
            if let Some(layout) = display.iterm2_layout {
                let resolved = resolve_iterm2_layout(source_width, source_height, layout, context);
                display.x_offset = resolved.x_padding;
                display.y_offset = resolved.y_padding;
                (
                    resolved.destination_width,
                    resolved.destination_height,
                    resolved.box_width.div_ceil(context.cell_width).max(1),
                    resolved.box_height.div_ceil(context.cell_height).max(1),
                )
            } else {
                let (width, height) =
                    placement_pixel_size(source_width, source_height, display, context);
                (
                    width,
                    height,
                    width
                        .saturating_add(display.x_offset)
                        .div_ceil(context.cell_width)
                        .max(1),
                    height
                        .saturating_add(display.y_offset)
                        .div_ceil(context.cell_height)
                        .max(1),
                )
            };
        let placement_id = if display.placement_id == 0 {
            screen.next_placement_id()
        } else {
            display.placement_id
        };
        display.image_id = image_id;
        let placement = Placement {
            image: ImageKey {
                id: image_id,
                generation: image_generation,
            },
            placement_id,
            line: context.line,
            column: context.column,
            source_x,
            source_y,
            source_width,
            source_height,
            x_offset: display.x_offset,
            y_offset: display.y_offset,
            columns: if display.virtual_placement {
                display.columns
            } else {
                occupied_columns
            },
            rows: if display.virtual_placement {
                display.rows
            } else {
                occupied_rows
            },
            requested_columns: display.columns,
            requested_rows: display.rows,
            iterm2_layout: display.iterm2_layout,
            layout_columns: context.columns,
            layout_rows: context.rows,
            destination_width,
            destination_height,
            z: display.z,
            virtual_placement: display.virtual_placement,
            parent_id: display.parent_id,
            parent_placement_id: display.parent_placement_id,
            horizontal_offset: display.horizontal_offset,
            vertical_offset: display.vertical_offset,
        };
        if display.placement_id != 0 {
            screen.placements.retain(|existing| {
                existing.image.id != image_id || existing.placement_id != display.placement_id
            });
        }
        screen.placements.push(placement);
        screen.touch(image_id);
        Ok(Execution {
            response: response(image_id, display.image_number, display.placement_id, "OK"),
            cursor_movement: (!display.no_cursor_movement && !display.virtual_placement).then_some(
                CursorMovement {
                    start_column: context.column,
                    columns: occupied_columns,
                    rows: occupied_rows,
                },
            ),
            changed: true,
        })
    }

    fn delete(&mut self, alternate: bool, params: Params, context: CommandContext) {
        let screen = self.screen_mut(alternate);
        let selector = params.byte(b'd').unwrap_or(b'a');
        let delete_images = selector.is_ascii_uppercase();
        let selector = selector.to_ascii_lowercase();
        let x = params.u32(b'x').unwrap_or(0);
        let y = params.u32(b'y').unwrap_or(0);
        let image_id = params.u32(b'i').unwrap_or(0);
        let image_number = params.u32(b'I').unwrap_or(0);
        let placement_id = params.u32(b'p').unwrap_or(0);
        let z = params.i32(b'z').unwrap_or(0);
        let cell_x = x.checked_sub(1);
        let cell_y = y.checked_sub(1).and_then(|line| i32::try_from(line).ok());
        let newest_number_image = (image_number != 0)
            .then(|| {
                screen
                    .images
                    .values()
                    .filter(|image| image.number == image_number)
                    .max_by_key(|image| image.generation)
                    .map(|image| image.id)
            })
            .flatten();
        let mut deletion_candidates = if delete_images {
            match selector {
                b'a' => screen.images.keys().copied().collect::<Vec<_>>(),
                b'i' if image_id != 0 => vec![image_id],
                b'n' => newest_number_image.into_iter().collect(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let mut removed_images = Vec::new();
        screen.placements.retain(|placement| {
            let intersects = |x: u32, y: i32| {
                x >= placement.column
                    && x < placement.column.saturating_add(placement.columns.max(1))
                    && y >= placement.line
                    && y < placement.line.saturating_add(placement.rows.max(1) as i32)
            };
            let remove = match selector {
                // Geometry-based deletion cannot address a virtual placement because it has no
                // pinned rectangle. ID/number/range deletion can still remove it.
                b'a' => !placement.virtual_placement,
                b'i' => {
                    placement.image.id == image_id
                        && (placement_id == 0 || placement.placement_id == placement_id)
                }
                b'n' => {
                    newest_number_image == Some(placement.image.id)
                        && (placement_id == 0 || placement.placement_id == placement_id)
                }
                b'c' => !placement.virtual_placement && intersects(context.column, context.line),
                b'p' => {
                    !placement.virtual_placement
                        && cell_x.zip(cell_y).is_some_and(|(x, y)| intersects(x, y))
                }
                b'q' => {
                    !placement.virtual_placement
                        && cell_x.zip(cell_y).is_some_and(|(x, y)| intersects(x, y))
                        && placement.z == z
                }
                b'r' => {
                    x > 0 && y > 0 && x <= y && placement.image.id >= x && placement.image.id <= y
                }
                b'x' => {
                    !placement.virtual_placement
                        && cell_x.is_some_and(|x| {
                            x >= placement.column
                                && x < placement.column.saturating_add(placement.columns.max(1))
                        })
                }
                b'y' => {
                    !placement.virtual_placement
                        && cell_y.is_some_and(|y| {
                            y >= placement.line
                                && y < placement.line.saturating_add(placement.rows.max(1) as i32)
                        })
                }
                b'z' => !placement.virtual_placement && placement.z == z,
                b'f' => false,
                _ => false,
            };
            if remove {
                removed_images.push(placement.image.id);
            }
            !remove
        });
        if delete_images {
            deletion_candidates.extend(removed_images);
            deletion_candidates.sort_unstable();
            deletion_candidates.dedup();
            for image_id in deletion_candidates {
                if !screen.image_is_used(image_id) {
                    screen.remove_image(image_id);
                }
            }
        }
    }
}

fn clip_placement_to_rows(
    placement: &mut Placement,
    region_start: i32,
    region_end: i32,
    cell_height: u32,
) -> bool {
    let cell_height = i64::from(cell_height.max(1));
    let destination_height = i64::from(placement.destination_height.max(1));
    let image_top = i64::from(placement.line)
        .saturating_mul(cell_height)
        .saturating_add(i64::from(placement.y_offset));
    let image_bottom = image_top.saturating_add(destination_height);
    let region_top = i64::from(region_start).saturating_mul(cell_height);
    let region_bottom = i64::from(region_end).saturating_mul(cell_height);
    let visible_top = image_top.max(region_top);
    let visible_bottom = image_bottom.min(region_bottom);
    if visible_top >= visible_bottom {
        return false;
    }

    let top_clip = u64::try_from(visible_top - image_top).unwrap_or(u64::MAX);
    let visible_end = u64::try_from(visible_bottom - image_top).unwrap_or(0);
    let destination_height = u64::try_from(destination_height).unwrap_or(u64::MAX);
    let source_height = u64::from(placement.source_height.max(1));
    let source_start = source_height.saturating_mul(top_clip) / destination_height;
    let source_end = source_height
        .saturating_mul(visible_end)
        .div_ceil(destination_height)
        .min(source_height);
    if source_start >= source_end {
        return false;
    }

    placement.source_y = placement
        .source_y
        .saturating_add(u32::try_from(source_start).unwrap_or(u32::MAX));
    placement.source_height = u32::try_from(source_end - source_start)
        .unwrap_or(u32::MAX)
        .max(1);
    placement.destination_height = u32::try_from(visible_bottom - visible_top)
        .unwrap_or(u32::MAX)
        .max(1);
    if visible_top > image_top {
        placement.line = region_start;
        placement.y_offset = 0;
    }
    placement.rows = placement
        .destination_height
        .saturating_add(placement.y_offset)
        .div_ceil(u32::try_from(cell_height).unwrap_or(u32::MAX))
        .max(1);
    true
}

fn placement_pixel_size(
    source_width: u32,
    source_height: u32,
    display: Display,
    context: CommandContext,
) -> (u32, u32) {
    match (display.columns, display.rows) {
        (0, 0) => (source_width, source_height),
        (columns, rows) if columns != 0 && rows != 0 => (
            context
                .cell_width
                .saturating_mul(columns)
                .saturating_sub(display.x_offset)
                .max(1),
            context
                .cell_height
                .saturating_mul(rows)
                .saturating_sub(display.y_offset)
                .max(1),
        ),
        (columns, 0) => {
            let width = context
                .cell_width
                .saturating_mul(columns)
                .saturating_sub(display.x_offset)
                .max(1);
            let height = ((u64::from(width) * u64::from(source_height)
                + u64::from(source_width) / 2)
                / u64::from(source_width)) as u32;
            (width, height.max(1))
        }
        (0, rows) => {
            let height = context
                .cell_height
                .saturating_mul(rows)
                .saturating_sub(display.y_offset)
                .max(1);
            let width = ((u64::from(height) * u64::from(source_width)
                + u64::from(source_height) / 2)
                / u64::from(source_height)) as u32;
            (width.max(1), height)
        }
        _ => unreachable!(),
    }
}

fn resolve_iterm2_dimension(
    dimension: Iterm2Dimension,
    horizontal: bool,
    context: CommandContext,
) -> Option<u32> {
    let (cells, cell_size) = if horizontal {
        (context.columns, context.cell_width)
    } else {
        (context.rows, context.cell_height)
    };
    match dimension {
        Iterm2Dimension::Auto => None,
        Iterm2Dimension::Cells(value) => Some(value.saturating_mul(cell_size).max(1)),
        Iterm2Dimension::Pixels(value) => Some(value.max(1)),
        Iterm2Dimension::Percent(value) => {
            let viewport = u64::from(cells).saturating_mul(u64::from(cell_size));
            Some(
                u32::try_from(
                    viewport
                        .saturating_mul(u64::from(value.min(100)))
                        .div_ceil(100),
                )
                .unwrap_or(u32::MAX)
                .max(1),
            )
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Iterm2ResolvedLayout {
    destination_width: u32,
    destination_height: u32,
    box_width: u32,
    box_height: u32,
    x_padding: u32,
    y_padding: u32,
}

fn resolve_iterm2_layout(
    source_width: u32,
    source_height: u32,
    layout: Iterm2ImageLayout,
    context: CommandContext,
) -> Iterm2ResolvedLayout {
    let width = resolve_iterm2_dimension(layout.width, true, context);
    let height = resolve_iterm2_dimension(layout.height, false, context);
    let (destination_width, destination_height) = fit_pixel_size(
        source_width,
        source_height,
        width,
        height,
        layout.preserve_aspect_ratio,
    );
    let box_width = width.unwrap_or(destination_width);
    let box_height = height.unwrap_or(destination_height);
    Iterm2ResolvedLayout {
        destination_width,
        destination_height,
        box_width,
        box_height,
        x_padding: box_width.saturating_sub(destination_width) / 2,
        y_padding: box_height.saturating_sub(destination_height) / 2,
    }
}

fn fit_pixel_size(
    source_width: u32,
    source_height: u32,
    width: Option<u32>,
    height: Option<u32>,
    preserve_aspect_ratio: bool,
) -> (u32, u32) {
    match (width, height) {
        (None, None) => (source_width, source_height),
        (Some(width), None) => (width, scale_dimension(width, source_height, source_width)),
        (None, Some(height)) => (scale_dimension(height, source_width, source_height), height),
        (Some(width), Some(height)) if !preserve_aspect_ratio => (width, height),
        (Some(width), Some(height)) => {
            if u64::from(width).saturating_mul(u64::from(source_height))
                <= u64::from(height).saturating_mul(u64::from(source_width))
            {
                (
                    width,
                    scale_dimension(width, source_height, source_width).min(height),
                )
            } else {
                (
                    scale_dimension(height, source_width, source_height).min(width),
                    height,
                )
            }
        }
    }
}

fn scale_dimension(target: u32, source_axis: u32, source_reference: u32) -> u32 {
    u32::try_from(
        (u64::from(target)
            .saturating_mul(u64::from(source_axis))
            .saturating_add(u64::from(source_reference) / 2))
            / u64::from(source_reference.max(1)),
    )
    .unwrap_or(u32::MAX)
    .max(1)
}

fn eager_cursor_movement(display: Display, context: CommandContext) -> Option<CursorMovement> {
    if display.no_cursor_movement || display.virtual_placement {
        return None;
    }
    if let Some(layout) = display.iterm2_layout {
        let width = resolve_iterm2_dimension(layout.width, true, context)?;
        let height = resolve_iterm2_dimension(layout.height, false, context)?;
        return Some(CursorMovement {
            start_column: context.column,
            columns: width.div_ceil(context.cell_width).max(1),
            rows: height.div_ceil(context.cell_height).max(1),
        });
    }
    (display.columns > 0 && display.rows > 0).then_some(CursorMovement {
        start_column: context.column,
        columns: display.columns,
        rows: display.rows,
    })
}

fn iterm2_cursor_movement_from_loading(
    loading: &Loading,
    context: CommandContext,
) -> Option<CursorMovement> {
    let display = loading.display?;
    if display.no_cursor_movement || display.virtual_placement {
        return None;
    }
    let layout = display.iterm2_layout?;
    let (source_width, source_height) = probe_image_dimensions(loading)?;
    let resolved = resolve_iterm2_layout(source_width, source_height, layout, context);
    Some(CursorMovement {
        start_column: context.column,
        columns: resolved.box_width.div_ceil(context.cell_width).max(1),
        rows: resolved.box_height.div_ceil(context.cell_height).max(1),
    })
}

/// Read intrinsic dimensions without allocating or decoding the image's pixel buffer.
fn probe_image_dimensions(loading: &Loading) -> Option<(u32, u32)> {
    if loading.transmission.compression != Compression::None {
        return None;
    }
    let started = Instant::now();
    let dimensions = (|| {
        let mut reader = match loading.transmission.format {
            PixelFormat::Png => {
                ImageReader::with_format(Cursor::new(loading.data.as_slice()), ImageFormat::Png)
            }
            PixelFormat::EncodedImage => ImageReader::new(Cursor::new(loading.data.as_slice()))
                .with_guessed_format()
                .ok()?,
            PixelFormat::Rgb | PixelFormat::Rgba => return None,
        };
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
        limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
        limits.max_alloc = Some(MAX_IMAGE_BYTES as u64);
        reader.limits(limits);
        let dimensions = reader.into_dimensions().ok()?;
        validate_dimensions(dimensions.0, dimensions.1).ok()?;
        Some(dimensions)
    })();
    pipeline_metrics::record(PipelineStage::ImageProbe, started.elapsed());
    dimensions
}

#[derive(Clone, Copy)]
struct DecodedVirtualCell {
    image_id_low: u32,
    image_id_high: Option<u8>,
    placement_id: u32,
    row: Option<u32>,
    column: Option<u32>,
}

#[derive(Clone, Copy)]
struct VirtualRun {
    line: i32,
    column: u32,
    image_id: u32,
    placement_id: u32,
    image_row: u32,
    image_column: u32,
    width: u32,
}

impl VirtualRun {
    fn can_append(&self, cell: &VirtualCell, next: &DecodedVirtualCell) -> bool {
        let image_id_high = (self.image_id >> 24) as u8;
        self.line == cell.line
            && self.column.saturating_add(self.width) == cell.column
            && self.image_id & 0x00ff_ffff == next.image_id_low & 0x00ff_ffff
            && next.image_id_high.is_none_or(|high| high == image_id_high)
            && self.placement_id == next.placement_id
            && next.row.is_none_or(|row| row == self.image_row)
            && next
                .column
                .is_none_or(|column| column == self.image_column.saturating_add(self.width))
    }
}

fn diacritic_index(character: char) -> Option<u32> {
    DIACRITICS
        .binary_search(&(character as u32))
        .ok()
        .and_then(|index| u32::try_from(index).ok())
}

fn decode_virtual_cell(cell: &VirtualCell) -> DecodedVirtualCell {
    let row = cell
        .diacritics
        .first()
        .and_then(|character| diacritic_index(*character));
    let column = cell
        .diacritics
        .get(1)
        .and_then(|character| diacritic_index(*character));
    let image_id_high = cell
        .diacritics
        .get(2)
        .and_then(|character| diacritic_index(*character))
        .and_then(|value| u8::try_from(value).ok());
    DecodedVirtualCell {
        image_id_low: cell.image_id_low,
        image_id_high,
        placement_id: cell.placement_id,
        row,
        column,
    }
}

fn append_virtual_run(
    screen: &Screen,
    snapshot: &mut Snapshot,
    run: VirtualRun,
    cell_width: u32,
    cell_height: u32,
) {
    let Some(image) = screen.images.get(&run.image_id) else {
        return;
    };
    let Some(base) = screen.placements.iter().find(|placement| {
        placement.virtual_placement
            && placement.image.id == run.image_id
            && (run.placement_id == 0 || placement.placement_id == run.placement_id)
    }) else {
        return;
    };
    let grid_columns = if base.columns == 0 {
        image.width.div_ceil(cell_width.max(1))
    } else {
        base.columns
    }
    .max(1);
    let grid_rows = if base.rows == 0 {
        image.height.div_ceil(cell_height.max(1))
    } else {
        base.rows
    }
    .max(1);
    if run.image_column >= grid_columns || run.image_row >= grid_rows {
        return;
    }

    let grid_width = f64::from(grid_columns.saturating_mul(cell_width));
    let grid_height = f64::from(grid_rows.saturating_mul(cell_height));
    let scale = (grid_width / f64::from(image.width)).min(grid_height / f64::from(image.height));
    let scaled_width = f64::from(image.width) * scale;
    let scaled_height = f64::from(image.height) * scale;
    let image_left = (grid_width - scaled_width) / 2.;
    let image_top = (grid_height - scaled_height) / 2.;

    let fragment_left = f64::from(run.image_column.saturating_mul(cell_width));
    let fragment_top = f64::from(run.image_row.saturating_mul(cell_height));
    let fragment_right = f64::from(
        run.image_column
            .saturating_add(run.width)
            .min(grid_columns)
            .saturating_mul(cell_width),
    );
    let fragment_bottom = f64::from(
        run.image_row
            .saturating_add(1)
            .min(grid_rows)
            .saturating_mul(cell_height),
    );
    let left = fragment_left.max(image_left);
    let top = fragment_top.max(image_top);
    let right = fragment_right.min(image_left + scaled_width);
    let bottom = fragment_bottom.min(image_top + scaled_height);
    if right <= left || bottom <= top {
        return;
    }

    let source_x = ((left - image_left) / scale).round().max(0.) as u32;
    let source_y = ((top - image_top) / scale).round().max(0.) as u32;
    let source_right = ((right - image_left) / scale)
        .round()
        .min(f64::from(image.width)) as u32;
    let source_bottom = ((bottom - image_top) / scale)
        .round()
        .min(f64::from(image.height)) as u32;
    snapshot.placements.push(Placement {
        image: ImageKey {
            id: image.id,
            generation: image.generation,
        },
        placement_id: base.placement_id,
        line: run.line,
        column: run.column,
        source_x,
        source_y,
        source_width: source_right.saturating_sub(source_x).max(1),
        source_height: source_bottom.saturating_sub(source_y).max(1),
        x_offset: (left - fragment_left).round().max(0.) as u32,
        y_offset: (top - fragment_top).round().max(0.) as u32,
        columns: run.width,
        rows: 1,
        requested_columns: run.width,
        requested_rows: 1,
        iterm2_layout: None,
        layout_columns: 0,
        layout_rows: 0,
        destination_width: (right - left).round().max(1.) as u32,
        destination_height: (bottom - top).round().max(1.) as u32,
        z: -1,
        virtual_placement: false,
        parent_id: 0,
        parent_placement_id: 0,
        horizontal_offset: 0,
        vertical_offset: 0,
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Query,
    Transmit,
    TransmitAndDisplay,
    Display,
    Delete,
    TransmitAnimationFrame,
    ControlAnimation,
    ComposeAnimation,
}

#[derive(Clone, Copy, Debug)]
struct Params {
    // Protocol values are limited to u32 or i32, so i64::MIN is an impossible value. Using it as
    // the empty marker keeps this hot parser table half the size of `[Option<i64>; 128]`.
    values: [i64; 128],
}

impl Default for Params {
    fn default() -> Self {
        Self {
            values: [i64::MIN; 128],
        }
    }
}

impl Params {
    fn get(&self, key: u8) -> Option<i64> {
        self.values
            .get(key as usize)
            .copied()
            .filter(|value| *value != i64::MIN)
    }

    fn u32(&self, key: u8) -> Option<u32> {
        self.get(key).and_then(|value| u32::try_from(value).ok())
    }

    fn i32(&self, key: u8) -> Option<i32> {
        self.get(key).and_then(|value| i32::try_from(value).ok())
    }

    fn byte(&self, key: u8) -> Option<u8> {
        self.get(key).and_then(|value| u8::try_from(value).ok())
    }
}

#[derive(Clone, Copy, Debug)]
struct Command<'a> {
    action: Action,
    quiet: Quiet,
    params: Params,
    encoded: &'a [u8],
    transmission_override: Option<Transmission>,
    display_override: Option<Display>,
}

impl<'a> Command<'a> {
    fn parse(payload: &'a [u8]) -> Result<Self, Error> {
        if payload.len() > MAX_COMMAND_BYTES {
            return Err(Error::InvalidData);
        }
        let (control, encoded) = payload
            .iter()
            .position(|byte| *byte == b';')
            .map_or((payload, &[][..]), |separator| {
                (&payload[..separator], &payload[separator + 1..])
            });
        let mut params = Params::default();
        if !control.is_empty() {
            for pair in control.split(|byte| *byte == b',') {
                let Some(separator) = pair.iter().position(|byte| *byte == b'=') else {
                    return Err(Error::InvalidFormat);
                };
                let (key, value) = (&pair[..separator], &pair[separator + 1..]);
                if key.len() != 1 || value.is_empty() || key[0] >= 128 {
                    return Err(Error::InvalidFormat);
                }
                let parsed = if value.len() == 1 && !value[0].is_ascii_digit() && value[0] != b'-' {
                    i64::from(value[0])
                } else {
                    std::str::from_utf8(value)
                        .ok()
                        .and_then(|value| value.parse::<i64>().ok())
                        .ok_or(Error::InvalidFormat)?
                };
                let signed = matches!(key[0], b'z' | b'H' | b'V');
                if signed {
                    if i32::try_from(parsed).is_err() {
                        return Err(Error::InvalidFormat);
                    }
                } else if u32::try_from(parsed).is_err() {
                    return Err(Error::InvalidFormat);
                }
                params.values[key[0] as usize] = parsed;
            }
        }
        let action = match params.byte(b'a').unwrap_or(b't') {
            b'q' => Action::Query,
            b't' => Action::Transmit,
            b'T' => Action::TransmitAndDisplay,
            b'p' => Action::Display,
            b'd' => Action::Delete,
            b'f' => Action::TransmitAnimationFrame,
            b'a' => Action::ControlAnimation,
            b'c' => Action::ComposeAnimation,
            _ => return Err(Error::InvalidFormat),
        };
        let quiet = match params.u32(b'q').unwrap_or(0) {
            0 => Quiet::No,
            1 => Quiet::Success,
            2 => Quiet::All,
            _ => return Err(Error::InvalidFormat),
        };
        Ok(Self {
            action,
            quiet,
            params,
            encoded,
            transmission_override: None,
            display_override: None,
        })
    }

    fn image_id(&self) -> u32 {
        self.params.u32(b'i').unwrap_or(0)
    }

    fn image_number(&self) -> u32 {
        self.params.u32(b'I').unwrap_or(0)
    }

    fn placement_id(&self) -> u32 {
        self.params.u32(b'p').unwrap_or(0)
    }

    fn transmission(&self) -> Result<Transmission, Error> {
        if let Some(transmission) = self.transmission_override {
            return Ok(transmission);
        }
        let format = match self.params.u32(b'f').unwrap_or(32) {
            24 => PixelFormat::Rgb,
            32 => PixelFormat::Rgba,
            100 => PixelFormat::Png,
            _ => return Err(Error::UnsupportedFormat),
        };
        let medium = match self.params.byte(b't').unwrap_or(b'd') {
            b'd' => Medium::Direct,
            b'f' => Medium::File,
            b't' => Medium::TemporaryFile,
            b's' => Medium::SharedMemory,
            _ => return Err(Error::UnsupportedMedium),
        };
        let compression = match self.params.byte(b'o') {
            None => Compression::None,
            Some(b'z') => Compression::Zlib,
            _ => return Err(Error::UnsupportedFormat),
        };
        Ok(Transmission {
            format,
            medium,
            width: self.params.u32(b's').unwrap_or(0),
            height: self.params.u32(b'v').unwrap_or(0),
            size: self.params.u32(b'S').unwrap_or(0),
            offset: self.params.u32(b'O').unwrap_or(0),
            image_id: self.image_id(),
            image_number: self.image_number(),
            placement_id: self.placement_id(),
            compression,
            more: medium == Medium::Direct && self.params.u32(b'm').unwrap_or(0) != 0,
            transient: self.params.u32(b'N').unwrap_or(0) & 1 != 0,
        })
    }

    fn display(&self) -> Display {
        if let Some(display) = self.display_override {
            return display;
        }
        Display {
            image_id: self.image_id(),
            image_number: self.image_number(),
            placement_id: self.placement_id(),
            x: self.params.u32(b'x').unwrap_or(0),
            y: self.params.u32(b'y').unwrap_or(0),
            width: self.params.u32(b'w').unwrap_or(0),
            height: self.params.u32(b'h').unwrap_or(0),
            x_offset: self.params.u32(b'X').unwrap_or(0),
            y_offset: self.params.u32(b'Y').unwrap_or(0),
            columns: self.params.u32(b'c').unwrap_or(0),
            rows: self.params.u32(b'r').unwrap_or(0),
            no_cursor_movement: self.params.u32(b'C').unwrap_or(0) == 1,
            virtual_placement: self.params.u32(b'U').unwrap_or(0) == 1,
            parent_id: self.params.u32(b'P').unwrap_or(0),
            parent_placement_id: self.params.u32(b'Q').unwrap_or(0),
            horizontal_offset: self.params.i32(b'H').unwrap_or(0),
            vertical_offset: self.params.i32(b'V').unwrap_or(0),
            z: self.params.i32(b'z').unwrap_or(0),
            iterm2_layout: None,
        }
    }

    fn frame_parameters(&self) -> FrameParameters {
        FrameParameters {
            x: self.params.u32(b'x').unwrap_or(0),
            y: self.params.u32(b'y').unwrap_or(0),
            base_frame: self.params.u32(b'c').unwrap_or(0),
            edit_frame: self.params.u32(b'r').unwrap_or(0),
            gap_ms: self.params.i32(b'z').unwrap_or(0),
            replace: self.params.u32(b'X').unwrap_or(0) == 1,
            background: self.params.u32(b'Y').unwrap_or(0),
        }
    }
}

#[derive(Debug)]
struct Decoded {
    width: u32,
    height: u32,
    byte_len: usize,
    pixels: Pixels,
}

fn resolve_image_id(screen: &Screen, image_id: u32, image_number: u32) -> Result<u32, Error> {
    if image_id != 0 && image_number != 0 {
        return Err(Error::MutuallyExclusiveIds);
    }
    if image_id != 0 {
        return screen
            .images
            .contains_key(&image_id)
            .then_some(image_id)
            .ok_or(Error::ImageNotFound);
    }
    if image_number == 0 {
        return Err(Error::ImageIdOrNumberRequired);
    }
    screen
        .images
        .values()
        .filter(|image| image.number == image_number)
        .max_by_key(|image| image.generation)
        .map(|image| image.id)
        .ok_or(Error::ImageNotFound)
}

fn solid_frame(width: u32, height: u32, rgba: u32) -> Result<Pixels, Error> {
    let byte_len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(Error::DimensionsTooLarge)?;
    let color = rgba.to_be_bytes();
    let mut pixels = take_pixel_buffer(byte_len);
    pixels.resize(byte_len, 0);
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }
    Ok(shared_pixels(pixels))
}

#[allow(clippy::too_many_arguments)]
fn compose_frame(
    base: &[u8],
    base_width: u32,
    base_height: u32,
    source: &[u8],
    source_width: u32,
    source_height: u32,
    destination_x: u32,
    destination_y: u32,
    replace: bool,
) -> Result<Vec<u8>, Error> {
    let expected_base = usize::try_from(base_width)
        .ok()
        .and_then(|width| {
            usize::try_from(base_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(Error::DimensionsTooLarge)?;
    let expected_source = usize::try_from(source_width)
        .ok()
        .and_then(|width| {
            usize::try_from(source_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(Error::DimensionsTooLarge)?;
    if base.len() != expected_base || source.len() != expected_source {
        return Err(Error::InvalidData);
    }
    let mut output = base.to_vec();
    for source_y in 0..source_height {
        for source_x in 0..source_width {
            let source_index = usize::try_from((source_y * source_width + source_x) * 4)
                .map_err(|_| Error::DimensionsTooLarge)?;
            let target_x = destination_x + source_x;
            let target_y = destination_y + source_y;
            let target_index = usize::try_from((target_y * base_width + target_x) * 4)
                .map_err(|_| Error::DimensionsTooLarge)?;
            let src = &source[source_index..source_index + 4];
            let dst = &mut output[target_index..target_index + 4];
            if replace || src[3] == 255 {
                dst.copy_from_slice(src);
            } else if src[3] != 0 {
                alpha_blend(dst, src);
            }
        }
    }
    Ok(output)
}

fn alpha_blend(destination: &mut [u8], source: &[u8]) {
    let source_alpha = u32::from(source[3]);
    let destination_alpha = u32::from(destination[3]);
    let inverse = 255 - source_alpha;
    let output_alpha = source_alpha + (destination_alpha * inverse + 127) / 255;
    if output_alpha == 0 {
        destination.copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    for channel in 0..3 {
        let source_premultiplied = u32::from(source[channel]) * source_alpha;
        let destination_premultiplied =
            u32::from(destination[channel]) * destination_alpha * inverse / 255;
        destination[channel] =
            ((source_premultiplied + destination_premultiplied) / output_alpha).min(255) as u8;
    }
    destination[3] = output_alpha.min(255) as u8;
}

fn schedule_animation(image: &mut Image, now: Instant) {
    if image.animation.state == AnimationState::Stopped {
        image.animation.next_frame_at = None;
        return;
    }
    let Some(frame) = image.frames.get(image.current_frame) else {
        image.animation.next_frame_at = None;
        return;
    };
    image.animation.next_frame_at = match frame.gap_ms {
        0 => None,
        gap if gap < 0 => Some(now),
        gap => Some(now + Duration::from_millis(gap as u64)),
    };
}

#[inline]
fn decoded_capacity(encoded_len: usize) -> usize {
    encoded_len.saturating_add(3) / 4 * 3
}

/// Decode one Kitty direct-transmission chunk straight into the persistent loading buffer.
///
/// Kitty applications normally split images into thousands of small APCs. Decoding each APC into
/// a temporary Vec and then copying it into `Loading::data` turns a single frame into hundreds of
/// allocator operations. `decode_vec` appends in place, retaining the buffer across chunks.
fn append_base64(data: &mut Vec<u8>, encoded: &[u8]) -> Result<(), Error> {
    if encoded.is_empty() {
        return Ok(());
    }
    let original_len = data.len();
    data.reserve(decoded_capacity(encoded.len()).min(MAX_IMAGE_BYTES.saturating_sub(original_len)));
    let started = Instant::now();
    let decoded = Base64.decode_vec(encoded, data);
    pipeline_metrics::record(PipelineStage::Base64, started.elapsed());
    if decoded.is_err() || data.len() > MAX_IMAGE_BYTES {
        data.truncate(original_len);
        return Err(Error::InvalidData);
    }
    Ok(())
}

fn decode_loading(loading: &mut Loading) -> Result<Decoded, Error> {
    let mut data = std::mem::take(&mut loading.data);
    if loading.transmission.compression == Compression::Zlib {
        let capacity = match loading.transmission.format {
            PixelFormat::Rgb | PixelFormat::Rgba => {
                validate_dimensions(loading.transmission.width, loading.transmission.height)?;
                let channels = if loading.transmission.format == PixelFormat::Rgb {
                    3
                } else {
                    4
                };
                usize::try_from(loading.transmission.width)
                    .ok()
                    .and_then(|width| {
                        usize::try_from(loading.transmission.height)
                            .ok()
                            .and_then(|height| width.checked_mul(height))
                    })
                    .and_then(|pixels| pixels.checked_mul(channels))
                    .ok_or(Error::DimensionsTooLarge)?
            }
            PixelFormat::Png | PixelFormat::EncodedImage => 0,
        };
        let decompressed = if capacity != 0 {
            let mut output = take_pixel_buffer(capacity);
            output.resize(capacity, 0);
            let started = Instant::now();
            let result = LIBDEFLATE
                .with(|decoder| decoder.borrow_mut().zlib_decompress(&data, &mut output))
                .map_err(|_| Error::DecompressionFailed);
            pipeline_metrics::record(PipelineStage::Zlib, started.elapsed());
            let written = result?;
            output.truncate(written);
            output
        } else {
            // PNG dimensions are part of the image payload, so there is no trustworthy output
            // bound before decoding. Keep the streaming implementation for that uncommon case.
            let mut decoder = ZlibDecoder::new(data.as_slice());
            let mut output = Vec::new();
            let started = Instant::now();
            let result = decoder
                .by_ref()
                .take((MAX_IMAGE_BYTES + 1) as u64)
                .read_to_end(&mut output)
                .map_err(|_| Error::DecompressionFailed);
            pipeline_metrics::record(PipelineStage::Zlib, started.elapsed());
            result?;
            output
        };
        if decompressed.len() > MAX_IMAGE_BYTES {
            return Err(Error::InvalidData);
        }
        recycle_pixel_buffer(data);
        data = decompressed;
    }
    if matches!(
        loading.transmission.format,
        PixelFormat::Png | PixelFormat::EncodedImage
    ) {
        let mut reader = match loading.transmission.format {
            PixelFormat::Png => ImageReader::with_format(Cursor::new(&data), ImageFormat::Png),
            PixelFormat::EncodedImage => ImageReader::new(Cursor::new(&data))
                .with_guessed_format()
                .map_err(|_| Error::InvalidData)?,
            PixelFormat::Rgb | PixelFormat::Rgba => unreachable!(),
        };
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
        limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
        limits.max_alloc = Some(MAX_IMAGE_BYTES as u64);
        reader.limits(limits);
        let started = Instant::now();
        let decoded = reader.decode().map_err(|_| Error::InvalidData);
        pipeline_metrics::record(PipelineStage::ImageDecode, started.elapsed());
        let rgba = decoded?.to_rgba8();
        let (width, height) = rgba.dimensions();
        validate_dimensions(width, height)?;
        let pixels = rgba.into_raw();
        recycle_pixel_buffer(data);
        return Ok(Decoded {
            width,
            height,
            byte_len: pixels.len(),
            pixels: shared_pixels(pixels),
        });
    }
    let width = loading.transmission.width;
    let height = loading.transmission.height;
    validate_dimensions(width, height)?;
    let channels = if loading.transmission.format == PixelFormat::Rgb {
        3
    } else {
        4
    };
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or(Error::DimensionsTooLarge)?;
    if data.len() != expected {
        return Err(Error::InvalidData);
    }
    let pixels = if channels == 4 {
        data
    } else {
        let pixel_count = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or(Error::DimensionsTooLarge)?;
        let rgba_len = pixel_count
            .checked_mul(4)
            .ok_or(Error::DimensionsTooLarge)?;
        data.reserve(rgba_len.saturating_sub(data.len()));
        data.resize(rgba_len, 255);
        // Expanding from the end makes the source and destination ranges safe to overlap and
        // avoids allocating a second full-frame RGBA buffer.
        for pixel in (0..pixel_count).rev() {
            let source = pixel * 3;
            let destination = pixel * 4;
            data.copy_within(source..source + 3, destination);
            data[destination + 3] = 255;
        }
        data
    };
    Ok(Decoded {
        width,
        height,
        byte_len: pixels.len(),
        pixels: shared_pixels(pixels),
    })
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), Error> {
    if width == 0 || height == 0 {
        return Err(Error::DimensionsRequired);
    }
    if width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(Error::DimensionsTooLarge);
    }
    let bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(Error::DimensionsTooLarge)?;
    if bytes as usize > MAX_IMAGE_BYTES {
        return Err(Error::DimensionsTooLarge);
    }
    Ok(())
}

fn load_medium(transmission: Transmission, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
    match transmission.medium {
        Medium::Direct => Ok(payload),
        Medium::File | Medium::TemporaryFile => {
            let path = std::str::from_utf8(&payload).map_err(|_| Error::InvalidData)?;
            let canonical = fs::canonicalize(path).map_err(|_| Error::InvalidData)?;
            validate_file_path(&canonical, transmission.medium)?;
            let result = read_file_range(&canonical, transmission.offset, transmission.size);
            if transmission.medium == Medium::TemporaryFile {
                let _ = fs::remove_file(&canonical);
            }
            result
        }
        Medium::SharedMemory => {
            read_shared_memory(&payload, transmission.offset, transmission.size)
        }
    }
}

fn validate_file_path(path: &Path, medium: Medium) -> Result<(), Error> {
    let path = path.to_string_lossy();
    if path.starts_with("/proc/")
        || path.starts_with("/sys/")
        || path.starts_with("/dev/") && !path.starts_with("/dev/shm/")
    {
        return Err(Error::InvalidData);
    }
    if medium == Medium::TemporaryFile {
        let temp = fs::canonicalize(std::env::temp_dir()).map_err(|_| Error::InvalidData)?;
        let in_temp = Path::new(path.as_ref()).starts_with(&temp)
            || path.starts_with("/tmp/")
            || path.starts_with("/dev/shm/");
        if !in_temp {
            return Err(Error::TemporaryFileNotInTempDir);
        }
        if !path.contains("tty-graphics-protocol") {
            return Err(Error::TemporaryFileNotNamedCorrectly);
        }
    }
    Ok(())
}

fn read_file_range(path: &Path, offset: u32, size: u32) -> Result<Vec<u8>, Error> {
    let metadata = fs::metadata(path).map_err(|_| Error::InvalidData)?;
    if !metadata.is_file() {
        return Err(Error::InvalidData);
    }
    let mut file = File::open(path).map_err(|_| Error::InvalidData)?;
    file.seek(SeekFrom::Start(u64::from(offset)))
        .map_err(|_| Error::InvalidData)?;
    let limit = if size == 0 {
        MAX_IMAGE_BYTES
    } else {
        size as usize
    }
    .min(MAX_IMAGE_BYTES);
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::InvalidData)?;
    if bytes.len() > limit {
        return Err(Error::InvalidData);
    }
    Ok(bytes)
}

#[cfg(unix)]
fn read_shared_memory(payload: &[u8], offset: u32, size: u32) -> Result<Vec<u8>, Error> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd;

    let name = CString::new(payload).map_err(|_| Error::InvalidData)?;
    let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
    if fd < 0 {
        return Err(Error::InvalidData);
    }
    unsafe {
        libc::shm_unlink(name.as_ptr());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|_| Error::InvalidData)?;
    if metadata.len() > MAX_IMAGE_BYTES as u64 {
        return Err(Error::InvalidData);
    }
    let object_len = metadata.len() as usize;
    let start = offset as usize;
    if start > object_len {
        return Err(Error::InvalidData);
    }
    let available = object_len - start;
    let limit = if size == 0 { available } else { size as usize }
        .min(available)
        .min(MAX_IMAGE_BYTES);
    if object_len == 0 {
        return Ok(Vec::new());
    }
    // Darwin's POSIX shared-memory descriptors are mmap-only and return ENXIO from read/write.
    // Mapping also avoids an intermediate kernel buffer before we own the requested range.
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            object_len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            std::os::fd::AsRawFd::as_raw_fd(&file),
            0,
        )
    };
    if mapping == libc::MAP_FAILED {
        return Err(Error::InvalidData);
    }
    let bytes = unsafe {
        let bytes = std::slice::from_raw_parts(mapping.cast::<u8>().add(start), limit).to_vec();
        libc::munmap(mapping, object_len);
        bytes
    };
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_shared_memory(_payload: &[u8], _offset: u32, _size: u32) -> Result<Vec<u8>, Error> {
    Err(Error::UnsupportedMedium)
}

#[derive(Clone, Copy, Debug)]
enum Error {
    InvalidFormat,
    InvalidData,
    UnsupportedFormat,
    UnsupportedMedium,
    DecompressionFailed,
    DimensionsRequired,
    DimensionsTooLarge,
    ImageIdRequired,
    ImageIdOrNumberRequired,
    VirtualPlacementHasParent,
    MutuallyExclusiveIds,
    ImageNotFound,
    OutOfMemory,
    TemporaryFileNotInTempDir,
    TemporaryFileNotNamedCorrectly,
    UnimplementedAction,
}

impl Error {
    fn message(self) -> &'static str {
        match self {
            Self::InvalidFormat | Self::InvalidData => "EINVAL: invalid data",
            Self::UnsupportedFormat => "EINVAL: unsupported format",
            Self::UnsupportedMedium => "EINVAL: unsupported medium",
            Self::DecompressionFailed => "EINVAL: decompression failed",
            Self::DimensionsRequired => "EINVAL: dimensions required",
            Self::DimensionsTooLarge => "EINVAL: dimensions too large",
            Self::ImageIdRequired => "EINVAL: image ID required",
            Self::ImageIdOrNumberRequired => "EINVAL: image ID or number required",
            Self::VirtualPlacementHasParent => "EINVAL: virtual placement cannot refer to a parent",
            Self::MutuallyExclusiveIds => "EINVAL: image ID and number are mutually exclusive",
            Self::ImageNotFound => "ENOENT: image not found",
            Self::OutOfMemory => "ENOMEM: out of memory",
            Self::TemporaryFileNotInTempDir => "EINVAL: temporary file not in temp dir",
            Self::TemporaryFileNotNamedCorrectly => "EINVAL: temporary file not named correctly",
            Self::UnimplementedAction => "ERROR: unimplemented action",
        }
    }

    fn response(
        self,
        image_id: u32,
        image_number: u32,
        placement_id: u32,
        quiet: Quiet,
    ) -> Option<String> {
        if quiet == Quiet::All {
            None
        } else {
            response(image_id, image_number, placement_id, self.message())
        }
    }
}

fn response(image_id: u32, image_number: u32, placement_id: u32, message: &str) -> Option<String> {
    if image_id == 0 && image_number == 0 {
        return None;
    }
    let mut response = String::from("\x1b_G");
    let mut separator = "";
    if image_id != 0 {
        response.push_str(&format!("i={image_id}"));
        separator = ",";
    }
    if image_number != 0 {
        response.push_str(separator);
        response.push_str(&format!("I={image_number}"));
        separator = ",";
    }
    if placement_id != 0 {
        response.push_str(separator);
        response.push_str(&format!("p={placement_id}"));
    }
    response.push(';');
    response.push_str(message);
    response.push_str("\x1b\\");
    Some(response)
}

/// Stateful APC splitter. Ordinary bytes are forwarded in large contiguous slices, while Kitty
/// APC payloads are returned without allowing them to enter the text parser.
#[derive(Debug, Default)]
pub struct ApcSplitter {
    state: ApcState,
    payload: Vec<u8>,
    oversized: bool,
}

pub enum ApcOutput<'a> {
    Text(&'a [u8]),
    Kitty(&'a [u8]),
    TerminalVersionQuery,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ApcState {
    #[default]
    Ground,
    Escape,
    CsiStart,
    CsiGreater,
    CsiGreaterZero,
    ApcStart,
    Apc,
    ApcEscape,
    Ignore,
    IgnoreEscape,
}

impl ApcSplitter {
    pub fn advance(&mut self, bytes: &[u8], mut output: impl FnMut(ApcOutput<'_>)) {
        let mut index = 0;
        while index < bytes.len() {
            match self.state {
                ApcState::Ground => {
                    let remaining = &bytes[index..];
                    if let Some(escape) = memchr(0x1b, remaining) {
                        if escape != 0 {
                            output(ApcOutput::Text(&remaining[..escape]));
                        }
                        self.state = ApcState::Escape;
                        index += escape + 1;
                    } else {
                        output(ApcOutput::Text(remaining));
                        return;
                    }
                }
                ApcState::Escape => {
                    match bytes[index] {
                        b'_' => {
                            self.state = ApcState::ApcStart;
                            index += 1;
                        }
                        b'[' => {
                            self.state = ApcState::CsiStart;
                            index += 1;
                        }
                        _ => {
                            output(ApcOutput::Text(&[0x1b]));
                            self.state = ApcState::Ground;
                            // Reprocess this byte in Ground; it can itself begin another escape.
                        }
                    }
                }
                ApcState::CsiStart => {
                    if bytes[index] == b'>' {
                        self.state = ApcState::CsiGreater;
                        index += 1;
                    } else {
                        output(ApcOutput::Text(b"\x1b["));
                        self.state = ApcState::Ground;
                    }
                }
                ApcState::CsiGreater => {
                    if bytes[index] == b'0' {
                        self.state = ApcState::CsiGreaterZero;
                        index += 1;
                    } else {
                        output(ApcOutput::Text(b"\x1b[>"));
                        self.state = ApcState::Ground;
                    }
                }
                ApcState::CsiGreaterZero => {
                    if bytes[index] == b'q' {
                        output(ApcOutput::TerminalVersionQuery);
                        self.state = ApcState::Ground;
                        index += 1;
                    } else {
                        output(ApcOutput::Text(b"\x1b[>0"));
                        self.state = ApcState::Ground;
                    }
                }
                ApcState::ApcStart => {
                    match bytes[index] {
                        b'G' => {
                            self.payload.clear();
                            self.oversized = false;
                            self.state = ApcState::Apc;
                        }
                        0x1b => self.state = ApcState::IgnoreEscape,
                        _ => self.state = ApcState::Ignore,
                    }
                    index += 1;
                }
                ApcState::Apc => {
                    let remaining = &bytes[index..];
                    if let Some(escape) = memchr(0x1b, remaining) {
                        self.extend(&remaining[..escape]);
                        self.state = ApcState::ApcEscape;
                        index += escape + 1;
                    } else {
                        self.extend(remaining);
                        return;
                    }
                }
                ApcState::ApcEscape => {
                    match bytes[index] {
                        b'\\' => {
                            if !self.oversized {
                                output(ApcOutput::Kitty(&self.payload));
                            }
                            self.payload.clear();
                            self.state = ApcState::Ground;
                        }
                        0x1b => self.push(0x1b),
                        byte => {
                            self.push(0x1b);
                            self.push(byte);
                            self.state = ApcState::Apc;
                        }
                    }
                    index += 1;
                }
                ApcState::Ignore => {
                    if let Some(escape) = memchr(0x1b, &bytes[index..]) {
                        self.state = ApcState::IgnoreEscape;
                        index += escape + 1;
                    } else {
                        return;
                    }
                }
                ApcState::IgnoreEscape => {
                    match bytes[index] {
                        b'\\' => self.state = ApcState::Ground,
                        0x1b => {}
                        _ => self.state = ApcState::Ignore,
                    }
                    index += 1;
                }
            }
        }
    }

    fn push(&mut self, byte: u8) {
        if self.payload.len() < MAX_COMMAND_BYTES {
            self.payload.push(byte);
        } else {
            self.oversized = true;
        }
    }

    fn extend(&mut self, bytes: &[u8]) {
        let available = MAX_COMMAND_BYTES.saturating_sub(self.payload.len());
        let accepted = available.min(bytes.len());
        self.payload.extend_from_slice(&bytes[..accepted]);
        if accepted != bytes.len() {
            self.oversized = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use flate2::Compression as FlateCompression;
    use flate2::write::ZlibEncoder;
    use image::codecs::png::PngEncoder;
    use image::codecs::tiff::TiffEncoder;
    use image::{DynamicImage, ExtendedColorType, ImageEncoder, RgbaImage};

    fn rgba_payload() -> String {
        Base64.encode([255, 0, 0, 255, 0, 255, 0, 255])
    }

    fn execute(graphics: &mut Graphics, payload: &[u8], line: i32, column: u32) -> Execution {
        graphics.execute(
            false,
            payload,
            CommandContext {
                line,
                column,
                columns: 80,
                rows: 24,
                cell_width: 8,
                cell_height: 18,
            },
        )
    }

    fn transmit(graphics: &mut Graphics, control: &str, bytes: &[u8]) -> Execution {
        let command = format!("{control};{}", Base64.encode(bytes));
        let mut execution = execute(graphics, command.as_bytes(), 3, 4);
        merge_execution(&mut execution, graphics.flush_pending());
        execution
    }

    #[test]
    fn splitter_preserves_text_and_joins_chunked_apc() {
        let mut splitter = ApcSplitter::default();
        let mut text_bytes = Vec::new();
        let mut commands = Vec::new();
        splitter.advance(b"hello\x1b_Gi=1,m=1;AAAA\x1b", |output| match output {
            ApcOutput::Text(bytes) => text_bytes.extend_from_slice(bytes),
            ApcOutput::Kitty(payload) => commands.push(payload.to_vec()),
            ApcOutput::TerminalVersionQuery => {}
        });
        splitter.advance(b"\\world", |output| match output {
            ApcOutput::Text(bytes) => text_bytes.extend_from_slice(bytes),
            ApcOutput::Kitty(payload) => commands.push(payload.to_vec()),
            ApcOutput::TerminalVersionQuery => {}
        });
        assert_eq!(text_bytes, b"helloworld");
        assert_eq!(commands, [b"i=1,m=1;AAAA".to_vec()]);
    }

    #[test]
    fn splitter_terminates_non_kitty_apcs_and_handles_repeated_escape() {
        let mut splitter = ApcSplitter::default();
        let mut text = Vec::new();
        let mut commands = Vec::new();
        splitter.advance(
            b"a\x1b_x\x1b\\b\x1b_Gi=1;AA\x1b\x1b\\",
            |output| match output {
                ApcOutput::Text(bytes) => text.extend_from_slice(bytes),
                ApcOutput::Kitty(payload) => commands.push(payload.to_vec()),
                ApcOutput::TerminalVersionQuery => {}
            },
        );
        assert_eq!(text, b"ab");
        assert_eq!(commands, [b"i=1;AA\x1b".to_vec()]);
    }

    #[test]
    fn splitter_intercepts_chunked_xtversion_without_touching_neighboring_csi() {
        let mut splitter = ApcSplitter::default();
        let mut text = Vec::new();
        let mut queries = 0;
        for chunk in [
            b"a\x1b[31m\x1b[>".as_slice(),
            b"0".as_slice(),
            b"qb".as_slice(),
        ] {
            splitter.advance(chunk, |output| match output {
                ApcOutput::Text(bytes) => text.extend_from_slice(bytes),
                ApcOutput::Kitty(_) => panic!("unexpected Kitty APC"),
                ApcOutput::TerminalVersionQuery => queries += 1,
            });
        }
        assert_eq!(text, b"a\x1b[31mb");
        assert_eq!(queries, 1);
    }

    #[test]
    fn direct_rgba_transmit_display_and_delete() {
        let mut graphics = Graphics::default();
        let command = format!("a=T,f=32,s=1,v=2,i=7,c=1,r=2;{}", rgba_payload());
        let execution = execute(&mut graphics, command.as_bytes(), 3, 4);
        assert!(execution.changed);
        assert_eq!(
            execution.cursor_movement,
            Some(CursorMovement {
                start_column: 4,
                columns: 1,
                rows: 2
            })
        );
        let snapshot = graphics.snapshot(false, 0);
        assert_eq!(snapshot.images.len(), 1);
        assert_eq!(snapshot.placements.len(), 1);
        let key = snapshot.images[0].key;
        assert_eq!(
            &*graphics.image(false, key).unwrap(),
            &[255, 0, 0, 255, 0, 255, 0, 255]
        );

        execute(&mut graphics, b"a=d,d=I,i=7", 0, 0);
        assert!(graphics.snapshot(false, 0).images.is_empty());
    }

    #[test]
    fn first_generation_animation_composes_selects_and_advances_frames() {
        let mut graphics = Graphics::default();
        transmit(
            &mut graphics,
            "a=T,f=32,s=2,v=1,i=7,c=2,r=1,C=1",
            &[255, 0, 0, 255, 255, 0, 0, 255],
        );
        transmit(
            &mut graphics,
            "a=f,f=32,i=7,x=1,y=0,s=1,v=1,c=1,z=10,X=1",
            &[0, 0, 255, 255],
        );
        assert_eq!(graphics.primary.images[&7].frames.len(), 2);

        execute(&mut graphics, b"a=a,i=7,r=1,z=10", 0, 0);
        let selected = execute(&mut graphics, b"a=a,i=7,c=2", 0, 0);
        assert!(selected.changed);
        let selected_key = graphics.snapshot(false, 0).images[0].key;
        assert_eq!(
            &*graphics.image(false, selected_key).unwrap(),
            &[255, 0, 0, 255, 0, 0, 255, 255]
        );

        execute(&mut graphics, b"a=a,i=7,c=1,s=3,v=2", 0, 0);
        let deadline = graphics.next_animation_deadline().unwrap();
        assert!(graphics.advance_animations(deadline));
        let animated_key = graphics.snapshot(false, 0).images[0].key;
        assert_ne!(animated_key, selected_key);
        assert_eq!(
            &*graphics.image(false, animated_key).unwrap(),
            &[255, 0, 0, 255, 0, 0, 255, 255]
        );
    }

    #[test]
    fn animation_frame_composition_uses_straight_alpha_and_clips_nothing() {
        let blended = compose_frame(
            &[0, 0, 255, 255],
            1,
            1,
            &[255, 0, 0, 128],
            1,
            1,
            0,
            0,
            false,
        )
        .unwrap();
        assert_eq!(blended, [128, 0, 127, 255]);

        let replaced =
            compose_frame(&[0, 0, 255, 255], 1, 1, &[255, 0, 0, 128], 1, 1, 0, 0, true).unwrap();
        assert_eq!(replaced, [255, 0, 0, 128]);
    }

    #[test]
    fn chunked_transmission_keeps_metadata_from_first_chunk() {
        let mut graphics = Graphics::default();
        let encoded = rgba_payload();
        let split = 4;
        let first = format!("a=T,f=32,s=1,v=2,i=9,m=1;{}", &encoded[..split]);
        let second = format!("m=0;{}", &encoded[split..]);
        assert!(!execute(&mut graphics, first.as_bytes(), 0, 0).changed);
        assert!(execute(&mut graphics, second.as_bytes(), 0, 0).changed);
        assert_eq!(graphics.snapshot(false, 0).placements.len(), 1);
    }

    #[test]
    fn chunked_transmission_inherits_quiet_mode() {
        let mut graphics = Graphics::default();
        let encoded = rgba_payload();
        let first = format!("a=t,f=32,s=1,v=2,i=9,m=1,q=1;{}", &encoded[..4]);
        let second = format!("m=0;{}", &encoded[4..]);
        assert!(
            execute(&mut graphics, first.as_bytes(), 0, 0)
                .response
                .is_none()
        );
        assert!(
            execute(&mut graphics, second.as_bytes(), 0, 0)
                .response
                .is_none()
        );
        let error = execute(&mut graphics, b"a=p,i=404,q=1", 0, 0);
        assert!(error.response.unwrap().contains("ENOENT"));
        assert!(
            execute(&mut graphics, b"a=p,i=404,q=2", 0, 0)
                .response
                .is_none()
        );
    }

    #[test]
    fn rgb_zlib_and_png_are_decoded_to_rgba() {
        let mut graphics = Graphics::default();
        transmit(&mut graphics, "a=t,f=24,s=1,v=1,i=1", &[1, 2, 3]);
        let first = graphics.snapshot(false, 0).images[0].key;
        assert_eq!(&*graphics.image(false, first).unwrap(), &[1, 2, 3, 255]);

        let mut encoder = ZlibEncoder::new(Vec::new(), FlateCompression::fast());
        encoder.write_all(&[4, 5, 6, 7]).unwrap();
        let compressed = encoder.finish().unwrap();
        transmit(&mut graphics, "a=t,f=32,s=1,v=1,i=2,o=z", &compressed);
        let second = graphics
            .snapshot(false, 0)
            .images
            .into_iter()
            .find(|image| image.key.id == 2)
            .unwrap()
            .key;
        assert_eq!(&*graphics.image(false, second).unwrap(), &[4, 5, 6, 7]);

        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&[8, 9, 10, 11], 1, 1, ExtendedColorType::Rgba8)
            .unwrap();
        transmit(&mut graphics, "a=t,f=100,i=3", &png);
        let third = graphics
            .snapshot(false, 0)
            .images
            .into_iter()
            .find(|image| image.key.id == 3)
            .unwrap()
            .key;
        assert_eq!(&*graphics.image(false, third).unwrap(), &[8, 9, 10, 11]);
    }

    #[test]
    fn iterm2_encoded_tiff_is_decoded_on_the_background_image_path() {
        let mut cursor = Cursor::new(Vec::new());
        TiffEncoder::new(&mut cursor)
            .write_image(&[12, 34, 56, 255], 1, 1, ExtendedColorType::Rgba8)
            .unwrap();

        let mut graphics = Graphics::default();
        let encoded = Base64.encode(cursor.into_inner());
        graphics.execute_iterm2_image(
            false,
            encoded.as_bytes(),
            Iterm2ImageLayout {
                preserve_aspect_ratio: true,
                ..Iterm2ImageLayout::default()
            },
            CommandContext {
                line: 0,
                column: 0,
                columns: 80,
                rows: 24,
                cell_width: 8,
                cell_height: 18,
            },
        );
        assert!(graphics.has_pending());
        graphics.flush_pending();
        let image = graphics
            .snapshot(false, 0)
            .images
            .into_iter()
            .next()
            .unwrap()
            .key;
        assert_eq!(&*graphics.image(false, image).unwrap(), &[12, 34, 56, 255]);
    }

    #[test]
    fn iterm2_common_encoded_raster_formats_are_decoded() {
        let source = DynamicImage::ImageRgba8(
            RgbaImage::from_raw(2, 1, vec![12, 34, 56, 255, 78, 90, 123, 255]).unwrap(),
        );
        let formats = [
            ImageFormat::Bmp,
            ImageFormat::Gif,
            ImageFormat::Jpeg,
            ImageFormat::Png,
            ImageFormat::Tiff,
            ImageFormat::WebP,
        ];

        for format in formats {
            let mut encoded = Cursor::new(Vec::new());
            source.write_to(&mut encoded, format).unwrap();

            let encoded = Base64.encode(encoded.into_inner());
            let mut graphics = Graphics::default();
            graphics.execute_iterm2_image(
                false,
                encoded.as_bytes(),
                Iterm2ImageLayout {
                    preserve_aspect_ratio: true,
                    ..Iterm2ImageLayout::default()
                },
                CommandContext {
                    line: 0,
                    column: 0,
                    columns: 80,
                    rows: 24,
                    cell_width: 8,
                    cell_height: 18,
                },
            );
            assert!(graphics.has_pending(), "format {format:?}");
            graphics.flush_pending();
            let snapshot = graphics.snapshot(false, 0);
            let image = &snapshot.images[0];
            assert_eq!((image.width, image.height), (2, 1), "format {format:?}");
            assert_eq!(
                graphics.image(false, image.key).unwrap().len(),
                2 * 4,
                "format {format:?}"
            );
        }
    }

    #[test]
    fn background_decodes_commit_in_submission_order_at_a_barrier() {
        let mut graphics = Graphics::default();
        for id in 1..=3 {
            let mut encoder = ZlibEncoder::new(Vec::new(), FlateCompression::fast());
            encoder.write_all(&[id as u8, 2, 3, 255]).unwrap();
            let compressed = encoder.finish().unwrap();
            let command = format!("a=t,f=32,s=1,v=1,i={id},o=z;{}", Base64.encode(compressed));
            assert!(!execute(&mut graphics, command.as_bytes(), 0, 0).changed);
        }
        assert_eq!(graphics.pending_decodes.len(), 3);
        assert!(graphics.primary.images.is_empty());

        // Display is a dependency barrier: all earlier transmissions must exist first and their
        // responses must retain wire order even if workers completed out of order.
        let execution = execute(&mut graphics, b"a=p,i=3,C=1", 0, 0);
        assert!(execution.changed);
        let response = execution.response.unwrap();
        assert!(response.find("i=1;OK").unwrap() < response.find("i=2;OK").unwrap());
        assert!(response.find("i=2;OK").unwrap() < response.find("i=3;OK").unwrap());
        assert_eq!(
            graphics
                .primary
                .images
                .values()
                .map(|image| image.order)
                .collect::<HashSet<_>>()
                .len(),
            3
        );
        assert_eq!(graphics.snapshot(false, 0).placements[0].image.id, 3);
    }

    #[test]
    fn inline_frames_do_not_force_an_earlier_background_decode_to_finish() {
        let mut graphics = Graphics::default();
        let mut encoder = ZlibEncoder::new(Vec::new(), FlateCompression::fast());
        encoder.write_all(&[1, 2, 3, 255]).unwrap();
        let compressed = encoder.finish().unwrap();
        let compressed = format!("a=t,f=32,s=1,v=1,i=1,o=z;{}", Base64.encode(compressed));
        assert!(!execute(&mut graphics, compressed.as_bytes(), 0, 0).changed);

        let inline = format!("a=t,f=32,s=1,v=1,i=2;{}", Base64.encode([4, 5, 6, 255]));
        assert!(!execute(&mut graphics, inline.as_bytes(), 0, 0).changed);
        assert_eq!(graphics.pending_decodes.len(), 2);
        assert!(graphics.primary.images.is_empty());

        let execution = execute(&mut graphics, b"a=p,i=2,C=1", 0, 0);
        let response = execution.response.unwrap();
        assert!(response.find("i=1;OK").unwrap() < response.find("i=2;OK").unwrap());
        assert_eq!(graphics.snapshot(false, 0).placements[0].image.id, 2);
    }

    #[test]
    fn query_validates_without_storing() {
        let mut graphics = Graphics::default();
        let execution = transmit(&mut graphics, "a=q,f=32,s=1,v=1,i=77", &[1, 2, 3, 4]);
        assert!(execution.response.unwrap().contains("i=77;OK"));
        assert!(graphics.snapshot(false, 0).images.is_empty());
    }

    #[test]
    fn image_numbers_keep_multiple_transmissions_and_display_the_newest() {
        let mut graphics = Graphics::default();
        transmit(&mut graphics, "a=t,f=32,s=1,v=1,I=5", &[1, 2, 3, 4]);
        transmit(&mut graphics, "a=t,f=32,s=1,v=1,I=5", &[5, 6, 7, 8]);
        let snapshot = graphics.snapshot(false, 0);
        assert_eq!(snapshot.images.len(), 2);
        let newest = snapshot
            .images
            .iter()
            .max_by_key(|image| image.key.generation)
            .unwrap()
            .key;
        execute(&mut graphics, b"a=p,I=5,C=1", 0, 0);
        assert_eq!(graphics.snapshot(false, 0).placements[0].image, newest);
    }

    #[test]
    fn placement_sizing_preserves_aspect_ratio_when_one_axis_is_given() {
        let mut graphics = Graphics::default();
        transmit(&mut graphics, "a=t,f=32,s=2,v=1,i=1", &[255; 8]);
        execute(&mut graphics, b"a=p,i=1,c=2,C=1", 0, 0);
        let placement = &graphics.snapshot(false, 0).placements[0];
        assert_eq!(
            (placement.destination_width, placement.destination_height),
            (16, 8)
        );
        assert_eq!((placement.columns, placement.rows), (2, 1));
    }

    #[test]
    fn iterm2_units_rescale_according_to_their_original_coordinate_space() {
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&[255; 8], 2, 1, ExtendedColorType::Rgba8)
            .unwrap();
        let encoded = Base64.encode(png);
        let context = CommandContext {
            line: 0,
            column: 0,
            columns: 10,
            rows: 10,
            cell_width: 8,
            cell_height: 20,
        };
        let cases = [
            (Iterm2Dimension::Cells(2), (16, 8), (20, 10)),
            (Iterm2Dimension::Pixels(16), (16, 8), (16, 8)),
            (Iterm2Dimension::Percent(50), (40, 20), (50, 25)),
            (Iterm2Dimension::Percent(150), (80, 40), (100, 50)),
        ];

        for (width, before, after) in cases {
            let mut graphics = Graphics::default();
            graphics.execute_iterm2_image(
                false,
                encoded.as_bytes(),
                Iterm2ImageLayout {
                    width,
                    height: Iterm2Dimension::Auto,
                    preserve_aspect_ratio: true,
                },
                context,
            );
            assert!(graphics.has_pending(), "{width:?}");
            graphics.flush_pending();
            let placement = &graphics.snapshot(false, 0).placements[0];
            assert_eq!(
                (placement.destination_width, placement.destination_height),
                before,
                "{width:?}"
            );

            graphics.rescale(10, 20);
            let placement = &graphics.snapshot(false, 0).placements[0];
            assert_eq!(
                (placement.destination_width, placement.destination_height),
                after,
                "{width:?}"
            );
        }
    }

    #[test]
    fn explicit_cell_boxes_exclude_the_first_cell_pixel_offset() {
        let mut graphics = Graphics::default();
        transmit(&mut graphics, "a=t,f=32,s=2,v=2,i=1", &[255; 16]);
        execute(&mut graphics, b"a=p,i=1,c=2,r=1,X=3,Y=4,C=1", 0, 0);
        let placement = &graphics.snapshot(false, 0).placements[0];
        assert_eq!((placement.x_offset, placement.y_offset), (3, 4));
        assert_eq!(
            (placement.destination_width, placement.destination_height),
            (13, 14)
        );
        assert_eq!((placement.columns, placement.rows), (2, 1));

        execute(&mut graphics, b"a=p,i=1,p=9,c=2,r=1,X=99,Y=99,C=1", 0, 0);
        let placement = graphics
            .snapshot(false, 0)
            .placements
            .into_iter()
            .find(|placement| placement.placement_id == 9)
            .unwrap();
        assert_eq!((placement.x_offset, placement.y_offset), (7, 17));
        assert_eq!(
            (placement.destination_width, placement.destination_height),
            (9, 1)
        );
    }

    #[test]
    fn uppercase_delete_keeps_images_used_by_another_placement() {
        let mut graphics = Graphics::default();
        transmit(&mut graphics, "a=t,f=32,s=1,v=1,i=7", &[1, 2, 3, 4]);
        execute(&mut graphics, b"a=p,i=7,p=1,C=1", 0, 0);
        execute(&mut graphics, b"a=p,i=7,p=2,C=1", 2, 3);
        execute(&mut graphics, b"a=d,d=I,i=7,p=1", 0, 0);
        let snapshot = graphics.snapshot(false, 0);
        assert_eq!(snapshot.images.len(), 1);
        assert_eq!(snapshot.placements.len(), 1);
        assert_eq!(snapshot.placements[0].placement_id, 2);
        execute(&mut graphics, b"a=d,d=I,i=7,p=2", 0, 0);
        assert!(graphics.snapshot(false, 0).images.is_empty());
    }

    #[test]
    fn delete_coordinates_are_one_based_and_ranges_select_image_ids() {
        let mut graphics = Graphics::default();
        for id in [10, 20] {
            transmit(
                &mut graphics,
                &format!("a=t,f=32,s=1,v=1,i={id}"),
                &[1, 2, 3, 4],
            );
            execute(&mut graphics, format!("a=p,i={id},C=1").as_bytes(), 2, id);
        }
        execute(&mut graphics, b"a=d,d=p,x=11,y=3", 0, 0);
        assert_eq!(graphics.snapshot(false, 0).placements[0].image.id, 20);
        execute(&mut graphics, b"a=p,i=10,C=1", 4, 1);
        execute(&mut graphics, b"a=d,d=r,x=10,y=10", 0, 0);
        assert!(
            graphics
                .snapshot(false, 0)
                .placements
                .iter()
                .all(|placement| placement.image.id != 10)
        );
    }

    #[test]
    fn virtual_unicode_placeholders_merge_only_adjacent_compatible_cells() {
        let mut graphics = Graphics::default();
        transmit(&mut graphics, "a=t,f=32,s=2,v=2,i=7", &[255; 16]);
        execute(&mut graphics, b"a=p,i=7,p=3,U=1,c=2,r=2", 0, 0);
        let mut snapshot = graphics.snapshot(false, 0);
        let cells = [
            VirtualCell {
                line: 1,
                column: 4,
                image_id_low: 7,
                placement_id: 3,
                diacritics: vec![
                    char::from_u32(DIACRITICS[0]).unwrap(),
                    char::from_u32(DIACRITICS[0]).unwrap(),
                ],
            },
            VirtualCell {
                line: 1,
                column: 5,
                image_id_low: 7,
                placement_id: 3,
                diacritics: Vec::new(),
            },
            VirtualCell {
                line: 1,
                column: 7,
                image_id_low: 7,
                placement_id: 3,
                diacritics: vec![
                    char::from_u32(DIACRITICS[1]).unwrap(),
                    char::from_u32(DIACRITICS[0]).unwrap(),
                ],
            },
        ];
        graphics.resolve_virtual_placeholders(false, &mut snapshot, &cells, 8, 18);
        assert_eq!(snapshot.placements.len(), 2);
        assert_eq!(
            (
                snapshot.placements[0].column,
                snapshot.placements[0].columns
            ),
            (4, 2)
        );
        assert_eq!(
            (
                snapshot.placements[1].column,
                snapshot.placements[1].columns
            ),
            (7, 1)
        );
        assert!(
            snapshot
                .placements
                .iter()
                .all(|placement| placement.z == -1)
        );
    }

    #[test]
    fn storage_eviction_invalidates_old_generations() {
        let mut graphics = Graphics::default();
        graphics.set_storage_limit(4);
        transmit(&mut graphics, "a=t,f=32,s=1,v=1,i=1", &[1, 2, 3, 4]);
        let old = graphics.snapshot(false, 0).images[0].key;
        transmit(&mut graphics, "a=t,f=32,s=1,v=1,i=2", &[5, 6, 7, 8]);
        assert!(graphics.image(false, old).is_none());
        assert_eq!(graphics.snapshot(false, 0).images[0].key.id, 2);
    }

    #[test]
    fn temporary_file_medium_reads_a_range_and_unlinks_the_file() {
        let mut graphics = Graphics::default();
        let path = std::env::temp_dir().join(format!(
            "tty-graphics-protocol-eggie-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, [99, 1, 2, 3, 4]).unwrap();
        let control = "a=t,t=t,f=32,s=1,v=1,i=12,O=1,S=4";
        transmit(&mut graphics, control, path.to_string_lossy().as_bytes());
        assert!(!path.exists());
        let key = graphics.snapshot(false, 0).images[0].key;
        assert_eq!(&*graphics.image(false, key).unwrap(), &[1, 2, 3, 4]);
    }

    #[cfg(unix)]
    #[test]
    fn shared_memory_medium_reads_and_unlinks_the_object() {
        use std::ffi::CString;
        use std::os::fd::FromRawFd;

        let mut graphics = Graphics::default();
        // Darwin limits POSIX shared-memory names to PSHMNAMLEN (31 bytes).
        let name = CString::new(format!(
            "/eg{:x}{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                & 0x0fff_ffff
        ))
        .unwrap();
        let fd = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        assert!(fd >= 0);
        assert_eq!(unsafe { libc::ftruncate(fd, 4) }, 0);
        let file = unsafe { File::from_raw_fd(fd) };
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                std::os::fd::AsRawFd::as_raw_fd(&file),
                0,
            )
        };
        assert_ne!(mapping, libc::MAP_FAILED);
        unsafe {
            std::ptr::copy_nonoverlapping([1_u8, 2, 3, 4].as_ptr(), mapping.cast::<u8>(), 4);
            assert_eq!(libc::msync(mapping, 4, libc::MS_SYNC), 0);
            libc::munmap(mapping, 4);
        }
        transmit(
            &mut graphics,
            "a=t,t=s,f=32,s=1,v=1,i=13,S=4",
            name.as_bytes(),
        );
        let reopened = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
        assert_eq!(
            reopened, -1,
            "shared memory name must be unlinked after opening"
        );
        let key = graphics.snapshot(false, 0).images[0].key;
        assert_eq!(&*graphics.image(false, key).unwrap(), &[1, 2, 3, 4]);
    }

    #[test]
    fn scroll_moves_placements_and_preserves_primary_history() {
        let mut graphics = Graphics::default();
        let command = format!("a=T,f=32,s=1,v=2,i=7,c=1,r=2,C=1;{}", rgba_payload());
        execute(&mut graphics, command.as_bytes(), 3, 4);
        graphics.scroll(
            false,
            ScrollParams {
                region_start: 0,
                region_end: 24,
                delta: -2,
                preserve_history: true,
                has_margins: false,
                cell_height: 20,
            },
        );
        assert_eq!(graphics.snapshot(false, 0).placements[0].line, 1);
        assert_eq!(graphics.snapshot(false, 2).placements[0].line, 3);
    }

    #[test]
    fn full_screen_history_scroll_keeps_moving_offscreen_placements() {
        let mut graphics = Graphics::default();
        let command = format!("a=T,f=32,s=1,v=2,i=7,c=1,r=2,C=1;{}", rgba_payload());
        execute(&mut graphics, command.as_bytes(), 1, 12);

        for _ in 0..8 {
            graphics.scroll(
                false,
                ScrollParams {
                    region_start: 0,
                    region_end: 5,
                    delta: -1,
                    preserve_history: true,
                    has_margins: false,
                    cell_height: 20,
                },
            );
        }

        let stored = graphics.snapshot(false, 0);
        assert_eq!(stored.placements[0].line, -7);
        assert!(
            stored.placements[0]
                .line
                .saturating_add(stored.placements[0].rows as i32)
                <= 0,
            "the historical image must not remain partially pinned to the viewport"
        );
        assert_eq!(graphics.snapshot(false, 8).placements[0].line, 1);
        assert_eq!(graphics.snapshot(false, 0).placements[0].line, -7);
    }

    #[test]
    fn margin_scroll_moves_only_contained_placements_and_clips_crossings() {
        let mut graphics = Graphics::default();
        let command = format!("a=T,f=32,s=1,v=2,i=7,c=1,r=2,C=1;{}", rgba_payload());
        execute(&mut graphics, command.as_bytes(), 0, 2);
        execute(&mut graphics, command.as_bytes(), 1, 4);

        graphics.scroll(
            false,
            ScrollParams {
                region_start: 1,
                region_end: 4,
                delta: -1,
                preserve_history: false,
                has_margins: true,
                cell_height: 18,
            },
        );
        let snapshot = graphics.snapshot(false, 0);
        let crossing = snapshot
            .placements
            .iter()
            .find(|placement| placement.column == 2)
            .unwrap();
        let contained = snapshot
            .placements
            .iter()
            .find(|placement| placement.column == 4)
            .unwrap();
        assert_eq!(crossing.line, 0, "an image crossing the margin stays put");
        assert_eq!(contained.line, 1);
        assert_eq!(contained.rows, 1);
        assert_eq!(contained.destination_height, 18);
    }

    #[test]
    fn reflow_anchors_update_physical_placements_and_keep_virtual_placements() {
        let mut graphics = Graphics::default();
        let command = format!("a=T,f=32,s=1,v=2,i=7,c=1,r=2,C=1;{}", rgba_payload());
        execute(&mut graphics, command.as_bytes(), 8, 12);
        execute(
            &mut graphics,
            format!("a=T,f=32,s=1,v=2,i=8,c=1,r=2,U=1;{}", rgba_payload()).as_bytes(),
            4,
            3,
        );

        assert_eq!(graphics.reflow_anchors(false), vec![Some((8, 12)), None]);
        graphics.apply_reflow_anchors(false, vec![Some((11, 4)), None]);
        let snapshot = graphics.snapshot(false, 0);
        assert_eq!(
            (snapshot.placements[0].line, snapshot.placements[0].column),
            (11, 4)
        );
        assert!(snapshot.placements[1].virtual_placement);
    }

    #[test]
    fn cell_rescale_updates_explicit_boxes_but_preserves_natural_image_size() {
        let mut graphics = Graphics::default();
        transmit(&mut graphics, "a=t,f=32,s=1,v=2,i=7", &[255; 8]);
        execute(&mut graphics, b"a=p,i=7,p=1,c=2,r=2,X=1,Y=2,C=1", 3, 4);
        execute(&mut graphics, b"a=p,i=7,p=2,C=1", 6, 4);

        graphics.rescale(17, 37);
        let snapshot = graphics.snapshot(false, 0);
        let explicit = snapshot
            .placements
            .iter()
            .find(|placement| placement.placement_id == 1)
            .unwrap();
        let natural = snapshot
            .placements
            .iter()
            .find(|placement| placement.placement_id == 2)
            .unwrap();
        assert_eq!(
            (explicit.destination_width, explicit.destination_height),
            (33, 72)
        );
        assert_eq!(
            (natural.destination_width, natural.destination_height),
            (1, 2)
        );
    }

    #[test]
    fn render_snapshot_keeps_only_viewport_intersections_and_referenced_images() {
        let mut graphics = Graphics::default();
        for (id, line) in [(1, -3), (2, -1), (3, 23), (4, 24)] {
            let command = format!("a=T,f=32,s=1,v=2,i={id},c=1,r=2,C=1;{}", rgba_payload());
            execute(&mut graphics, command.as_bytes(), line, 0);
        }

        let snapshot = graphics.snapshot_visible(false, 0, 24, 80);

        assert_eq!(
            snapshot
                .placements
                .iter()
                .map(|placement| placement.image.id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(
            snapshot
                .images
                .iter()
                .map(|image| image.key.id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn equal_z_placements_follow_internal_image_and_ref_creation_order() {
        let mut graphics = Graphics::default();
        transmit(&mut graphics, "a=t,f=32,s=1,v=1,i=100", &[1, 2, 3, 255]);
        transmit(&mut graphics, "a=t,f=32,s=1,v=1,i=1", &[4, 5, 6, 255]);

        execute(&mut graphics, b"a=p,i=100,p=9,z=0,C=1", 0, 0);
        execute(&mut graphics, b"a=p,i=100,p=2,z=0,C=1", 0, 1);
        execute(&mut graphics, b"a=p,i=1,p=7,z=0,C=1", 0, 2);

        assert_eq!(
            graphics
                .snapshot(false, 0)
                .placements
                .iter()
                .map(|placement| (placement.image.id, placement.placement_id))
                .collect::<Vec<_>>(),
            vec![(100, 9), (100, 2), (1, 7)]
        );
    }

    #[test]
    fn post_020_frame_composition_remains_explicitly_unsupported() {
        let mut graphics = Graphics::default();
        let response = execute(&mut graphics, b"a=c,i=1", 0, 0).response.unwrap();
        assert!(response.contains("ERROR: unimplemented action"));
    }
}
