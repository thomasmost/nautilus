//! Engine-free model of the browser.
//!
//! This crate knows nothing about Chromium, egui, or winit. It holds the window
//! and tab model, the named commands the UI emits, the events the engine
//! reports, the keymap, settings, and profile paths. The binary crate turns
//! commands into engine calls; the engine crate turns Chromium callbacks into
//! [`EngineEvent`]s. Both go through the pure functions [`dispatch`] and
//! [`apply`], which is what keeps this crate unit-testable without a browser
//! engine.

pub mod command;
pub mod event;
pub mod keymap;
pub mod model;
pub mod profile;
pub mod settings;

pub use command::{Command, EngineAction, dispatch};
pub use event::{Cursor, EngineEvent, PendingTab, apply};
pub use keymap::{CommandKind, KeyChord, Keymap, Platform};
pub use model::{Browser, Favicon, LoadState, Tab, TabId, Window, WindowId};
pub use profile::{Profile, ProfileError};
pub use settings::{ChromeColor, Settings, SettingsError};
