//! Terminal grid rendering, font rasterization, IME contracts and latency diagnostics.
pub mod input_latency;
#[cfg(target_os = "macos")]
mod metal_terminal;
pub mod terminal_renderer;
mod terminal_sprites;
