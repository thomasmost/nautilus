//! Chromium glue. This is the only crate in the workspace that imports the
//! `cef` crate (the Chromium Embedded Framework). Everything that crosses
//! into the model or the UI is expressed in Nautilus's own types: core ids,
//! [`EngineEvent`]s, [`EngineInput`], and [`Frame`]s of pixels. CEF's
//! browsers, handlers, and ids never leave this crate.
//!
//! - [`run_subprocess`] must be the first thing `main` does: Chromium runs
//!   its renderer, GPU, and utility processes by starting the executable
//!   again, and those runs never return to the browser's `main`.
//! - [`Engine`] initialises CEF once per process and owns the event queue.
//! - [`TabHost`] owns the tabs of one window. Each tab is a windowless
//!   (off-screen) browser that paints into a [`Frame`] the window uploads
//!   as a texture under the toolbar.
//!
//! [`EngineEvent`]: nautilus_core::EngineEvent

mod delegate;
mod engine;
mod frame;
mod host;
mod input;
mod keycodes;

pub use dpi::PhysicalSize;
pub use keyboard_types;
pub use raw_window_handle::RawWindowHandle;

pub use crate::engine::{Engine, EngineError, Waker, run_helper, run_subprocess};
pub use crate::frame::Frame;
pub use crate::host::TabHost;
pub use crate::input::{ButtonAction, EngineInput, KeyInput, MouseButton};
