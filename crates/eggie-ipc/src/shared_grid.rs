//! A connection owns a bounded pool; each immutable row lease releases its slot on last drop.
use anyhow::{Context, Result, ensure};
use eggie_protocol::{
    TerminalCellView, TerminalCells, TerminalColor, TerminalRow, TerminalRowStorage,
    TerminalSnapshot,
};
use memmap2::{MmapMut, MmapOptions};
use serde::{Deserialize, Serialize};
use std::{
    ffi::CString,
    fs::File,
    os::fd::FromRawFd,
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
};

const SLOT_HEADER: usize = 64;
const RECORD: usize = 48;
const SLOTS_PER_ROW: usize = 3;
const MAX_POOL: usize = 64 * 1024 * 1024;
const FREE: u32 = 0;
const WRITING: u32 = 2;
const TRANSFERRED: u32 = 1;
const READING: u32 = 3;
static SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct Region {
    map: MmapMut,
    name: CString,
    capacity: usize,
    stride: usize,
    rows: usize,
}

impl Region {
    fn slot_offset(&self, slot: usize) -> usize {
        slot * self.stride
    }
    fn lease(&self, slot: usize) -> &AtomicU32 {
        // Every header is cache-line aligned and contains an initialized atomic. The mapping
        // outlives all leases; only the producer writes an exclusively reserved payload.
        unsafe {
            &*self
                .map
                .as_ptr()
                .add(self.slot_offset(slot))
                .cast::<AtomicU32>()
        }
    }
    fn payload(&self, slot: usize, len: usize) -> &[u8] {
        let offset = self.slot_offset(slot) + SLOT_HEADER;
        &self.map[offset..offset + len]
    }
    fn generation(&self, slot: usize) -> u64 {
        let offset = self.slot_offset(slot) + 8;
        u64::from_le_bytes(self.map[offset..offset + 8].try_into().unwrap())
    }
    fn create(rows: usize, capacity: usize) -> Result<Arc<Self>> {
        let stride = (SLOT_HEADER + capacity).next_multiple_of(64);
        let len = rows
            .checked_mul(SLOTS_PER_ROW)
            .and_then(|n| n.checked_mul(stride))
            .context("grid pool size overflow")?;
        ensure!(len > 0 && len <= MAX_POOL, "grid pool exceeds memory bound");
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let name = CString::new(format!("/eggie-grid-{}-{serial}", std::process::id()))?;
        let fd = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let result = (|| {
            file.set_len(len as u64)?;
            let map = unsafe { MmapOptions::new().len(len).map_mut(&file) }?;
            for slot in 0..rows * SLOTS_PER_ROW {
                unsafe {
                    map.as_ptr()
                        .add(slot * stride)
                        .cast_mut()
                        .cast::<AtomicU32>()
                        .write(AtomicU32::new(FREE));
                }
            }
            Ok(Arc::new(Self {
                map,
                name: name.clone(),
                capacity,
                stride,
                rows,
            }))
        })();
        if result.is_err() {
            unsafe {
                libc::shm_unlink(name.as_ptr());
            }
        }
        result
    }
    fn open(update: &GridUpdate) -> Result<Arc<Self>> {
        let name = CString::new(update.name.as_bytes())?;
        ensure!(
            update.name.starts_with("/eggie-grid-"),
            "invalid grid pool name"
        );
        let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDWR, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("opening grid pool");
        }
        let file = unsafe { File::from_raw_fd(fd) };
        // Both processes retain mappings, so unlinking the name cannot invalidate leased rows.
        unsafe {
            libc::shm_unlink(name.as_ptr());
        }
        let capacity = update.capacity as usize;
        ensure!(
            capacity >= 8 && capacity <= MAX_POOL,
            "invalid row capacity"
        );
        let stride = (SLOT_HEADER + capacity).next_multiple_of(64);
        let rows = update.snapshot.size.rows as usize;
        let len = rows
            .checked_mul(SLOTS_PER_ROW)
            .and_then(|n| n.checked_mul(stride))
            .context("grid pool size overflow")?;
        // Darwin rounds POSIX shm objects to VM pages even when ftruncate requests fewer bytes.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        ensure!(page > 0, "invalid VM page size");
        ensure!(
            len > 0
                && len <= MAX_POOL
                && file.metadata()?.len() >= len as u64
                && file.metadata()?.len() <= len.next_multiple_of(page as usize) as u64,
            "invalid grid pool size"
        );
        let map = unsafe { MmapOptions::new().len(len).map_mut(&file) }?;
        Ok(Arc::new(Self {
            map,
            name,
            capacity,
            stride,
            rows,
        }))
    }
}
impl Drop for Region {
    fn drop(&mut self) {
        unsafe {
            libc::shm_unlink(self.name.as_ptr());
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct RowReference {
    line: u16,
    slot: u32,
    generation: u64,
    len: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GridUpdate {
    name: String,
    capacity: u32,
    base_revision: u64,
    snapshot: TerminalSnapshot,
    changed: Vec<RowReference>,
}

#[derive(Default)]
pub struct GridWriter {
    region: Option<Arc<Region>>,
    previous: Option<Arc<TerminalSnapshot>>,
    generation: u64,
    scratch: Vec<u8>,
}

impl GridWriter {
    /// Returns false when all slots are leased or a row exceeds the bounded pool. Socket fallback
    /// then releases the old mapped frame without ever blocking the PTY publisher.
    pub fn send(
        &mut self,
        output: &mut impl std::io::Write,
        buffer: &mut Vec<u8>,
        snapshot: Arc<TerminalSnapshot>,
        after_revision: u64,
    ) -> Result<bool> {
        let started = crate::client::render_metrics_enabled().then(std::time::Instant::now);
        if !cfg!(target_endian = "little") || snapshot.size.rows == 0 {
            return Ok(false);
        }
        let rows = snapshot.size.rows as usize;
        let compatible = self.previous.as_ref().is_some_and(|p| {
            p.session_id == snapshot.session_id
                && p.revision == after_revision
                && p.size == snapshot.size
        });
        let mut changed = Vec::new();
        let mut required = 8;
        for line in 0..rows {
            let row = snapshot.cells.rows().get(line);
            let same = compatible
                && self
                    .previous
                    .as_ref()
                    .and_then(|p| p.cells.rows().get(line))
                    .zip(row)
                    .is_some_and(|(a, b)| a.same_storage(b) || a.iter().eq(b.iter()));
            if !same {
                required = required.max(row.map_or(8, packed_row_len));
                changed.push(line);
            }
        }
        let capacity = required
            .max(256)
            .checked_next_power_of_two()
            .context("grid row size overflow")?;
        let recreate = self
            .region
            .as_ref()
            .is_none_or(|r| r.rows != rows || r.capacity < required)
            || self
                .previous
                .as_ref()
                .is_some_and(|p| p.size != snapshot.size);
        if recreate {
            self.region = match Region::create(rows, capacity) {
                Ok(region) => Some(region),
                Err(_) => return Ok(false),
            };
            changed = (0..rows).collect();
        }
        let region = self.region.as_ref().unwrap();
        let mut reserved: Vec<RowReference> = Vec::new();
        for line in changed {
            let Some(slot) = (line * SLOTS_PER_ROW..(line + 1) * SLOTS_PER_ROW).find(|&slot| {
                region
                    .lease(slot)
                    .compare_exchange(FREE, WRITING, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            }) else {
                for reference in &reserved {
                    region
                        .lease(reference.slot as usize)
                        .store(FREE, Ordering::Release);
                }
                self.previous = None;
                return Ok(false);
            };
            self.scratch.clear();
            pack_row(snapshot.cells.rows().get(line), &mut self.scratch);
            self.generation = self.generation.wrapping_add(1);
            let offset = region.slot_offset(slot);
            // Reservation gives this producer exclusive access, including against other processes.
            unsafe {
                let pointer = region.map.as_ptr().add(offset).cast_mut();
                std::ptr::copy_nonoverlapping(
                    self.generation.to_le_bytes().as_ptr(),
                    pointer.add(8),
                    8,
                );
                std::ptr::copy_nonoverlapping(
                    self.scratch.as_ptr(),
                    pointer.add(SLOT_HEADER),
                    self.scratch.len(),
                );
            }
            region.lease(slot).store(TRANSFERRED, Ordering::Release);
            reserved.push(RowReference {
                line: line as u16,
                slot: slot as u32,
                generation: self.generation,
                len: self.scratch.len() as u32,
            });
        }
        let mut metadata = (*snapshot).clone();
        metadata.cells = TerminalCells::default();
        let update = GridUpdate {
            name: region.name.to_str()?.to_owned(),
            capacity: region.capacity as u32,
            base_revision: if compatible && !recreate {
                after_revision
            } else {
                0
            },
            snapshot: metadata,
            changed: reserved,
        };
        let result = crate::wire::write_grid_update(output, buffer, &update);
        if result.is_err() {
            // A failed notification closes the connection; no slot in this region is reused.
            self.region = None;
            self.previous = None;
            return result.map(|_| false);
        }
        if let Some(started) = started.filter(|_| snapshot.revision.is_multiple_of(30)) {
            eprintln!(
                "[eggie-grid] publish revision={} rows={} shared_bytes={} socket_bytes={} pool_bytes={} pack_and_notify_ms={:.3}",
                snapshot.revision,
                update.changed.len(),
                update.changed.iter().map(|r| r.len as usize).sum::<usize>(),
                buffer.len() + 4,
                region.map.len(),
                started.elapsed().as_secs_f64() * 1000.
            );
        }
        self.previous = Some(snapshot);
        Ok(true)
    }
}

#[derive(Debug)]
struct MappedRow {
    region: Arc<Region>,
    reference: RowReference,
    count: usize,
}
impl Drop for MappedRow {
    fn drop(&mut self) {
        self.region
            .lease(self.reference.slot as usize)
            .store(FREE, Ordering::Release);
    }
}
impl TerminalRowStorage for MappedRow {
    fn len(&self) -> usize {
        self.count
    }
    fn cell(&self, index: usize) -> TerminalCellView<'_> {
        assert!(index < self.count);
        let data = self
            .region
            .payload(self.reference.slot as usize, self.reference.len as usize);
        let offset = 8 + index * RECORD;
        let zero = word(data, offset + 32) as usize;
        let count = word(data, offset + 36) as usize;
        let link = word(data, offset + 40) as usize;
        let link_len = word(data, offset + 44) as usize;
        // Validated scalar values, bounds and alignment at lease acquisition. The row stays
        // immutable until this storage's last Arc is dropped, so these references remain valid.
        let zerowidth =
            unsafe { std::slice::from_raw_parts(data.as_ptr().add(zero).cast::<char>(), count) };
        let hyperlink = (link != 0)
            .then(|| unsafe { std::str::from_utf8_unchecked(&data[link..link + link_len]) });
        TerminalCellView {
            line: self.reference.line,
            column: u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()),
            flags: u16::from_le_bytes(data[offset + 2..offset + 4].try_into().unwrap()),
            character: char::from_u32(word(data, offset + 4)).unwrap(),
            zerowidth,
            foreground: color(data, offset + 8).unwrap(),
            background: color(data, offset + 16).unwrap(),
            underline_color: color(data, offset + 24),
            hyperlink,
        }
    }
}

#[derive(Default)]
pub struct GridReader {
    region: Option<Arc<Region>>,
    previous: Option<Arc<TerminalSnapshot>>,
}
impl GridReader {
    pub(crate) fn receive(&mut self, update: GridUpdate) -> Result<Arc<TerminalSnapshot>> {
        ensure!(
            cfg!(target_endian = "little"),
            "shared grid requires little-endian scalars"
        );
        let new_region = self
            .region
            .as_ref()
            .is_none_or(|r| r.name.to_bytes() != update.name.as_bytes());
        if new_region {
            self.region = Some(Region::open(&update)?);
        }
        let region = self.region.as_ref().unwrap().clone();
        ensure!(
            region.capacity == update.capacity as usize
                && region.rows == update.snapshot.size.rows as usize,
            "inconsistent shared grid shape"
        );
        ensure!(
            update.snapshot.cells.is_empty() && update.changed.len() <= region.rows,
            "invalid shared grid metadata"
        );
        let mut leases = Vec::with_capacity(update.changed.len());
        for reference in &update.changed {
            ensure!(
                (reference.line as usize) < region.rows
                    && reference.slot as usize / SLOTS_PER_ROW == reference.line as usize,
                "invalid shared row slot"
            );
            ensure!(
                reference.len as usize <= region.capacity && reference.len >= 8,
                "invalid shared row length"
            );
            ensure!(
                region
                    .lease(reference.slot as usize)
                    .compare_exchange(TRANSFERRED, READING, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok(),
                "stale shared row lease"
            );
            leases.push(MappedRow {
                region: region.clone(),
                reference: *reference,
                count: 0,
            });
            ensure!(
                region.generation(reference.slot as usize) == reference.generation,
                "stale shared row generation"
            );
        }
        let mut rows = if update.base_revision != 0 {
            let base = self.previous.as_ref().context("missing shared grid base")?;
            ensure!(
                base.session_id == update.snapshot.session_id
                    && base.revision == update.base_revision
                    && base.size == update.snapshot.size
                    && !new_region,
                "invalid shared grid base"
            );
            base.cells.rows().to_vec()
        } else {
            (0..region.rows)
                .map(|_| TerminalRow::owned(Vec::new()))
                .collect()
        };
        // Socket snapshots omit trailing empty rows; they still form a valid render base.
        rows.resize_with(region.rows, || TerminalRow::owned(Vec::new()));
        let mut seen = vec![false; region.rows];
        for mut lease in leases {
            let line = lease.reference.line as usize;
            ensure!(!seen[line], "duplicate shared row");
            seen[line] = true;
            lease.count = validate_row(
                region.payload(lease.reference.slot as usize, lease.reference.len as usize),
                update.snapshot.size.columns,
            )?;
            rows[line] = TerminalRow(Arc::new(lease));
        }
        ensure!(
            update.base_revision != 0 || seen.iter().all(|&v| v),
            "incomplete shared grid"
        );
        let mut snapshot = update.snapshot;
        snapshot.cells = TerminalCells::from_rows(rows);
        let snapshot = Arc::new(snapshot);
        self.previous = Some(snapshot.clone());
        Ok(snapshot)
    }
    pub fn accept_owned(&mut self, snapshot: Arc<TerminalSnapshot>) {
        self.previous = Some(snapshot);
    }
}

fn word(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}
fn put(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn color(data: &[u8], offset: usize) -> Option<TerminalColor> {
    let value = word(data, offset);
    match word(data, offset + 4) {
        0 => Some(TerminalColor::Rgb(value)),
        1 => Some(TerminalColor::Indexed(value as u8)),
        2 => Some(TerminalColor::Named(value as u16)),
        _ => None,
    }
}
fn put_color(data: &mut [u8], offset: usize, color: Option<TerminalColor>) {
    let (value, tag) = match color {
        Some(TerminalColor::Rgb(v)) => (v, 0),
        Some(TerminalColor::Indexed(v)) => (v as u32, 1),
        Some(TerminalColor::Named(v)) => (v as u32, 2),
        None => (0, 3),
    };
    put(data, offset, value);
    put(data, offset + 4, tag);
}
fn packed_row_len(row: &TerminalRow) -> usize {
    let mut len = 8 + RECORD * row.len();
    for cell in row.iter() {
        len =
            len.next_multiple_of(4) + cell.zerowidth.len() * 4 + cell.hyperlink.map_or(0, str::len);
    }
    len
}
fn pack_row(row: Option<&TerminalRow>, data: &mut Vec<u8>) {
    let count = row.map_or(0, TerminalRow::len);
    data.resize(8 + RECORD * count, 0);
    put(data, 0, count as u32);
    if let Some(row) = row {
        for (index, cell) in row.iter().enumerate() {
            let offset = 8 + index * RECORD;
            data[offset..offset + 2].copy_from_slice(&cell.column.to_le_bytes());
            data[offset + 2..offset + 4].copy_from_slice(&cell.flags.to_le_bytes());
            put(data, offset + 4, cell.character as u32);
            put_color(data, offset + 8, Some(cell.foreground));
            put_color(data, offset + 16, Some(cell.background));
            put_color(data, offset + 24, cell.underline_color);
            data.resize(data.len().next_multiple_of(4), 0);
            let zero = data.len();
            for ch in cell.zerowidth {
                data.extend_from_slice(&(*ch as u32).to_le_bytes());
            }
            put(data, offset + 32, zero as u32);
            put(data, offset + 36, cell.zerowidth.len() as u32);
            if let Some(link) = cell.hyperlink {
                let start = data.len();
                data.extend_from_slice(link.as_bytes());
                put(data, offset + 40, start as u32);
                put(data, offset + 44, link.len() as u32);
            }
        }
    }
    let len = data.len();
    put(data, 4, len as u32);
}
fn validate_row(data: &[u8], columns: u16) -> Result<usize> {
    ensure!(
        data.len() >= 8 && word(data, 4) as usize == data.len(),
        "invalid row header"
    );
    let count = word(data, 0) as usize;
    let records_end = count
        .checked_mul(RECORD)
        .and_then(|n| n.checked_add(8))
        .context("row record overflow")?;
    ensure!(
        count <= columns as usize && records_end <= data.len(),
        "invalid cell count"
    );
    let mut previous = None;
    for index in 0..count {
        let offset = 8 + index * RECORD;
        let column = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
        ensure!(
            column < columns && previous.is_none_or(|p| p < column),
            "invalid cell order"
        );
        previous = Some(column);
        ensure!(
            char::from_u32(word(data, offset + 4)).is_some(),
            "invalid cell scalar"
        );
        for color_offset in [8, 16, 24] {
            let tag = word(data, offset + color_offset + 4);
            let value = word(data, offset + color_offset);
            ensure!(
                tag <= if color_offset == 24 { 3 } else { 2 },
                "invalid color tag"
            );
            ensure!(tag != 1 || value <= u8::MAX as u32, "invalid indexed color");
            ensure!(tag != 2 || value <= u16::MAX as u32, "invalid named color");
        }
        let zero = word(data, offset + 32) as usize;
        let count = word(data, offset + 36) as usize;
        let end = count
            .checked_mul(4)
            .and_then(|n| zero.checked_add(n))
            .context("combining scalar overflow")?;
        ensure!(
            zero >= records_end && zero.is_multiple_of(4) && end <= data.len(),
            "invalid combining range"
        );
        for offset in (zero..end).step_by(4) {
            ensure!(
                char::from_u32(word(data, offset)).is_some(),
                "invalid combining scalar"
            );
        }
        let link = word(data, offset + 40) as usize;
        let len = word(data, offset + 44) as usize;
        if link != 0 {
            ensure!(
                link >= records_end && link.checked_add(len).is_some_and(|n| n <= data.len()),
                "invalid hyperlink range"
            );
            std::str::from_utf8(&data[link..link + len]).context("invalid hyperlink UTF-8")?;
        } else {
            ensure!(len == 0, "invalid absent hyperlink");
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests;
