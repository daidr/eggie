//! Window controllers project the shared presentation model; moving a tab never touches its PTY.
use super::*;
use eggie_domain::{ItemLocation, PresentationTarget, WindowFrame, WindowPresentation};

mod lifecycle;
mod navigation;
mod switchers;
mod transfer;
