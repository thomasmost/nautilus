//! The JSON objects extensions receive for tabs and windows.

use osmena_core::{LoadState, WindowId};
use serde_json::{Value, json};

use super::ids::{IdSpace, TAB_ID_NONE, group_id, window_id};
use super::pattern::Grants;
use super::snapshot::{SnapTab, SnapWindow, Snapshot};
use super::transport::Asker;

/// `chrome.tabs.TabStatus` for a tab.
pub(crate) fn status(tab: &SnapTab) -> &'static str {
    if tab.deferred {
        "unloaded"
    } else if tab.load == LoadState::Loading {
        "loading"
    } else {
        "complete"
    }
}

/// Everything an answer is made from: the published strip, the ids, what
/// the asking extension may see, and the window focused last.
pub(crate) struct View<'a> {
    pub snapshot: &'a Snapshot,
    pub ids: &'a IdSpace,
    pub grants: &'a Grants,
    pub last_focused: Option<WindowId>,
}

impl<'a> View<'a> {
    /// The window "current" means for a service worker and
    /// `lastFocusedWindow` means for everyone: the one focused last, else
    /// one that is focused now, else the first.
    pub(crate) fn last_focused_window(&self) -> Option<&'a SnapWindow> {
        self.last_focused
            .and_then(|id| self.snapshot.window(id))
            .or_else(|| self.snapshot.windows.iter().find(|window| window.focused))
            .or(self.snapshot.windows.first())
    }

    /// The window "current" means for `asker`: the one holding the page
    /// that asked, and the last focused one when that is not (yet) known.
    pub(crate) fn current_window(&self, asker: &Asker) -> Option<&'a SnapWindow> {
        let hosting = match asker {
            Asker::Worker => None,
            Asker::View(view) => self.snapshot.window_of_view(*view),
            Asker::Popup { over } => self.snapshot.window_of_view(*over),
        };
        hosting.or_else(|| self.last_focused_window())
    }

    /// The fields Chromium cannot know about a tab, laid over its own
    /// object for forwarded calls and `sender.tab`.
    pub(crate) fn strip_json(&self, window: &SnapWindow, index: usize) -> Value {
        let tab = &window.tabs[index];
        json!({
            "index": index,
            "windowId": window_id(window.id),
            "groupId": group_id(tab.folder),
            "active": tab.active,
            "highlighted": tab.active,
            "selected": tab.active,
            "pinned": false,
        })
    }

    /// A `chrome.tabs.Tab` from the snapshot alone.
    pub(crate) fn tab_json(&self, window: &SnapWindow, index: usize) -> Value {
        let tab = &window.tabs[index];
        let mut object = self.strip_json(window, index);
        let rest = json!({
            "id": self.ids.ext(tab.id).unwrap_or(TAB_ID_NONE),
            "incognito": false,
            "audible": false,
            "mutedInfo": { "muted": false },
            "discarded": tab.deferred,
            "autoDiscardable": true,
            "frozen": false,
            "lastAccessed": 0,
            "splitViewId": -1,
            "status": status(tab),
        });
        if let (Some(object), Value::Object(rest)) = (object.as_object_mut(), rest) {
            object.extend(rest);
        }
        if self.grants.sees(&tab.url) {
            object["url"] = json!(tab.url);
            object["title"] = json!(tab.title);
        }
        object
    }

    /// A `chrome.windows.Window`, with its tabs when `populate`.
    pub(crate) fn window_json(&self, window: &SnapWindow, populate: bool) -> Value {
        let mut object = json!({
            "id": window_id(window.id),
            "focused": window.focused,
            "incognito": false,
            "alwaysOnTop": false,
            "type": "normal",
            "state": "normal",
        });
        if populate {
            object["tabs"] = (0..window.tabs.len())
                .map(|index| self.tab_json(window, index))
                .collect();
        }
        object
    }
}
