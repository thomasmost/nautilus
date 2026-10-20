//! Chromium glue. This is the only crate in the workspace that imports the
//! `cef` crate (the Chromium Embedded Framework). It implements the
//! `osmena_engine` traits; everything that crosses into the shell is
//! expressed in osmena's types: core ids, `EngineEvent`s, `EngineInput`,
//! and frames of pixels. CEF's browsers, handlers, and ids never leave
//! this crate.
//!
//! - [`run_subprocess`] must be the first thing `main` does: Chromium runs
//!   its renderer, GPU, and utility processes by starting the executable
//!   again, and those runs never return to the browser's `main`.
//! - [`Engine`] initialises CEF once per process and owns the event queue.
//! - [`TabHost`] owns the tabs of one window. Each tab is a windowless
//!   (off-screen) browser that paints into a [`Frame`] the shell uploads
//!   as a texture under the toolbar (`PagePaint::Frame`).

// The CRX parser serves the store installs on Windows and macOS; it is
// kept (and tested) everywhere, so its lints are quiet elsewhere.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
mod crx;
mod delegate;
mod devtools;
mod engine;
mod extensions;
mod frame;
mod host;
mod input;
mod keycodes;
#[cfg(target_os = "macos")]
mod mac_app;
mod popup;

pub use crate::engine::{Engine, INFO, run_helper, run_subprocess};
pub use crate::extensions::web_store_item_id;
pub use crate::frame::Frame;
pub use crate::host::TabHost;
