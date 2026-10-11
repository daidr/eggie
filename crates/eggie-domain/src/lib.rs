mod cell_flags;
pub use cell_flags::TerminalCellFlags;
mod project;
pub use project::*;
mod profile;
pub use profile::*;
mod layout;
pub use layout::*;
mod presentation;
pub use presentation::*;
mod appearance;
pub use appearance::{DEFAULT_PALETTE, TerminalAppearance};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use uuid::Uuid;

pub type ProjectId = Uuid;
pub type GroupId = Uuid;
pub type ItemId = Uuid;
pub type SessionId = Uuid;
/// Identity of a [`Profile`]. Terminals record the profile they were launched from, and projects
/// name a default profile for new terminals.
pub type ProfileId = Uuid;
/// Stable identity of a presentation window, independent of a process or native window handle.
pub type WindowId = Uuid;
/// Identity of a split node in a [`LayoutNode`] tree. Distinct from [`GroupId`] so a split is never
/// confused with the groups it contains.
pub type SplitId = Uuid;

#[cfg(test)]
mod tests;
