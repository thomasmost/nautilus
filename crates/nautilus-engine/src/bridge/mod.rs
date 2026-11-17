//! The bridge between Chromium's extension system and Nautilus's tabs.
//!
//! Nautilus's tabs are windowless browsers in no Chromium window, so the
//! extension APIs that walk windows cannot see them. A script loaded
//! ahead of an extension's service worker (`js/bridge.js`) answers
//! those APIs from the engine instead. The engine reaches it through a
//! relay page, one of the extension's own pages, in a hidden browser:
//! `docs/bridge-protocol.md` has the messages.
//!
//! `rewrite` and `table` are pure. `relay` and `native` hold the CEF
//! side.

pub(crate) mod rewrite;
pub(crate) mod table;
