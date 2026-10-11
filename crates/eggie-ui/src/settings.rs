pub use eggie_config::font::*;
use gpui::Context;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use eggie_themes::{DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME};
pub use eggie_themes::{TerminalTheme, UiColors, theme_catalog};

mod appearance;
mod model;
mod store;
pub(crate) use appearance::*;
pub(crate) use model::*;
pub(crate) use store::*;
