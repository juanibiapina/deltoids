//! Review logic shared by `deltoids tui` and the browser reviewer.
//!
//! Each function here has one implementation for both UIs. The TUI links
//! the crate; the reviewer loads it as a small WebAssembly module
//! (`build-wasm.sh`) through the C-ABI in `wasm.rs`. The crate has no
//! dependencies, so the module stays small enough to load before the
//! reviewer's first render.

pub mod overview;
pub mod tree;

#[cfg(target_arch = "wasm32")]
mod wasm;
