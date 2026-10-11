//! Immutable rows let snapshots share unchanged cells and consume mapped storage without decoding.
use crate::{TerminalCell, TerminalColor};
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeSeq};
use std::{fmt, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TerminalCellView<'a> {
    #[serde(rename = "l")]
    pub line: u16,
    #[serde(rename = "c")]
    pub column: u16,
    #[serde(rename = "ch")]
    pub character: char,
    #[serde(rename = "z", skip_serializing_if = "<[char]>::is_empty")]
    pub zerowidth: &'a [char],
    #[serde(rename = "fg")]
    pub foreground: TerminalColor,
    #[serde(rename = "bg")]
    pub background: TerminalColor,
    #[serde(rename = "u", skip_serializing_if = "Option::is_none")]
    pub underline_color: Option<TerminalColor>,
    #[serde(rename = "h", skip_serializing_if = "Option::is_none")]
    pub hyperlink: Option<&'a str>,
    #[serde(rename = "f")]
    pub flags: u16,
}

impl TerminalCellView<'_> {
    pub fn to_owned(self) -> TerminalCell {
        TerminalCell {
            line: self.line,
            column: self.column,
            character: self.character,
            zerowidth: self.zerowidth.to_vec(),
            foreground: self.foreground,
            background: self.background,
            underline_color: self.underline_color,
            hyperlink: self.hyperlink.map(str::to_owned),
            flags: self.flags,
        }
    }
}

impl TerminalCell {
    pub fn view(&self) -> TerminalCellView<'_> {
        TerminalCellView {
            line: self.line,
            column: self.column,
            character: self.character,
            zerowidth: &self.zerowidth,
            foreground: self.foreground,
            background: self.background,
            underline_color: self.underline_color,
            hyperlink: self.hyperlink.as_deref(),
            flags: self.flags,
        }
    }
}

/// Storage owns its immutable backing; IPC implements this contract for leased shared-memory rows.
pub trait TerminalRowStorage: fmt::Debug + Send + Sync {
    fn len(&self) -> usize;
    fn cell(&self, index: usize) -> TerminalCellView<'_>;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug)]
pub struct TerminalRow(pub Arc<dyn TerminalRowStorage>);

#[derive(Debug)]
struct OwnedRow(Vec<TerminalCell>);
impl TerminalRowStorage for OwnedRow {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn cell(&self, index: usize) -> TerminalCellView<'_> {
        self.0[index].view()
    }
}

impl TerminalRow {
    pub fn owned(cells: Vec<TerminalCell>) -> Self {
        Self(Arc::new(OwnedRow(cells)))
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn cell(&self, index: usize) -> TerminalCellView<'_> {
        self.0.cell(index)
    }
    pub fn iter(&self) -> impl ExactSizeIterator<Item = TerminalCellView<'_>> {
        (0..self.len()).map(|index| self.cell(index))
    }
    pub fn same_storage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TerminalCells {
    rows: Vec<TerminalRow>,
    len: usize,
}

impl TerminalCells {
    pub fn from_rows(rows: Vec<TerminalRow>) -> Self {
        let len = rows.iter().map(TerminalRow::len).sum();
        Self { rows, len }
    }
    pub fn rows(&self) -> &[TerminalRow] {
        &self.rows
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn iter(&self) -> impl Iterator<Item = TerminalCellView<'_>> {
        self.rows.iter().flat_map(TerminalRow::iter)
    }
    pub fn get(&self, mut index: usize) -> Option<TerminalCellView<'_>> {
        for row in &self.rows {
            if index < row.len() {
                return Some(row.cell(index));
            }
            index -= row.len();
        }
        None
    }
    pub fn to_vec(&self) -> Vec<TerminalCell> {
        self.iter().map(TerminalCellView::to_owned).collect()
    }
}

impl From<Vec<TerminalCell>> for TerminalCells {
    fn from(mut cells: Vec<TerminalCell>) -> Self {
        cells.sort_by_key(|cell| (cell.line, cell.column));
        let mut rows = Vec::new();
        let mut cells = cells.into_iter().peekable();
        while let Some(first) = cells.peek() {
            let line = first.line as usize;
            while rows.len() < line {
                rows.push(TerminalRow::owned(Vec::new()));
            }
            let mut row = Vec::new();
            while cells.peek().is_some_and(|cell| cell.line as usize == line) {
                row.push(cells.next().unwrap());
            }
            rows.push(TerminalRow::owned(row));
        }
        Self::from_rows(rows)
    }
}

impl PartialEq for TerminalCells {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}
impl Eq for TerminalCells {}
impl PartialEq<Vec<TerminalCell>> for TerminalCells {
    fn eq(&self, other: &Vec<TerminalCell>) -> bool {
        self.iter().eq(other.iter().map(TerminalCell::view))
    }
}
impl Serialize for TerminalCells {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.len))?;
        for cell in self.iter() {
            seq.serialize_element(&cell)?;
        }
        seq.end()
    }
}
impl<'de> Deserialize<'de> for TerminalCells {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<TerminalCell>::deserialize(deserializer).map(Into::into)
    }
}
