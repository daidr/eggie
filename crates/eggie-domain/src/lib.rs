mod cell_flags;
pub use cell_flags::TerminalCellFlags;
mod project;
pub use project::*;
mod profile;
pub use profile::*;
mod layout;
pub use layout::*;
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
/// Identity of a GUI window. A session in the daemon is tagged with the id of the window that owns
/// it so that each window sees only its own sessions; `None` ownership marks a detached session.
pub type WindowId = Uuid;
/// Identity of a split node in a [`LayoutNode`] tree. Distinct from [`GroupId`] so a split is never
/// confused with the groups it contains.
pub type SplitId = Uuid;

#[cfg(test)]
mod tests;
