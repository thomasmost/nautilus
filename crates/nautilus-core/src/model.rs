//! Windows, tabs, and the identifiers that cross crate boundaries.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::profile::Profile;
use crate::settings::Settings;

/// Identifies a browser window. Allocated by [`Browser`]; engine ids never
/// leave the engine crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WindowId(pub u64);

/// Identifies a tab. Allocated by [`Browser`]; engine ids never leave the
/// engine crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TabId(pub u64);

/// Where a tab is in its page load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadState {
    #[default]
    Idle,
    Loading,
    Complete,
}

/// A decoded favicon, straight RGBA8, row-major, top-left origin. The pixels
/// sit behind an `Arc` so a renderer can tell a new icon from the one it
/// already uploaded by pointer, without hashing the pixels every frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Favicon {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

/// One page shown as a tab.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub id: TabId,
    pub url: Url,
    pub title: String,
    pub favicon: Option<Favicon>,
    pub load: LoadState,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub status: String,
    /// `Some(reason)` after the engine reported this tab's content crashed.
    /// The tab stays in the model so its URL is not lost; the UI shows an
    /// error panel with a reload button instead of page content.
    pub crashed: Option<String>,
}

impl Tab {
    pub fn new(id: TabId, url: Url) -> Tab {
        Tab {
            id,
            url,
            title: String::new(),
            favicon: None,
            load: LoadState::Idle,
            can_go_back: false,
            can_go_forward: false,
            status: String::new(),
            crashed: None,
        }
    }

    /// The text to show in the tab strip: the page title, or the URL until
    /// a title arrives.
    pub fn display_title(&self) -> &str {
        if self.title.is_empty() {
            self.url.as_str()
        } else {
            &self.title
        }
    }
}

/// A browser window. Invariants: a window always has at least one tab,
/// `active` names one of them, and `selected` holds `active` plus any other
/// tabs of this window the user added to the selection (Ctrl-click,
/// Shift-click). Closing the last tab closes the window.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub id: WindowId,
    pub tabs: Vec<Tab>,
    pub active: TabId,
    pub selected: Vec<TabId>,
}

impl Window {
    /// A window with one tab, which is active and selected.
    pub fn with_tab(id: WindowId, tab: Tab) -> Window {
        let active = tab.id;
        Window {
            id,
            tabs: vec![tab],
            active,
            selected: vec![active],
        }
    }

    pub fn is_selected(&self, tab: TabId) -> bool {
        self.selected.contains(&tab)
    }

    /// The selected tabs in strip order. Always includes the active tab.
    pub fn selection(&self) -> Vec<TabId> {
        self.tabs
            .iter()
            .map(|t| t.id)
            .filter(|id| self.selected.contains(id))
            .collect()
    }

    /// Make `tab` (which must be one of this window's tabs) the only
    /// selected tab. Does not change the active tab.
    pub(crate) fn select_only(&mut self, tab: TabId) {
        self.selected.clear();
        self.selected.push(tab);
    }
    pub fn index_of(&self, tab: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == tab)
    }

    pub fn tab(&self, tab: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == tab)
    }

    pub fn tab_mut(&mut self, tab: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == tab)
    }

    /// Index of the active tab. Panics only if the window invariant is broken.
    pub fn active_index(&self) -> usize {
        self.index_of(self.active)
            .expect("window invariant: active tab is one of the window's tabs")
    }

    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_index()]
    }
}

/// The whole browser state for one process (one profile).
#[derive(Debug)]
pub struct Browser {
    pub profile: Profile,
    pub settings: Settings,
    pub windows: Vec<Window>,
    /// Last engine-level error reported by the engine, shown in the status area.
    pub engine_error: Option<String>,
    next_window_id: u64,
    next_tab_id: u64,
}

impl Browser {
    pub fn new(profile: Profile, settings: Settings) -> Browser {
        Browser {
            profile,
            settings,
            windows: Vec::new(),
            engine_error: None,
            next_window_id: 1,
            next_tab_id: 1,
        }
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    pub fn window_of_tab(&self, tab: TabId) -> Option<WindowId> {
        self.windows
            .iter()
            .find(|w| w.index_of(tab).is_some())
            .map(|w| w.id)
    }

    pub fn tab(&self, tab: TabId) -> Option<&Tab> {
        self.windows.iter().find_map(|w| w.tab(tab))
    }

    pub fn tab_mut(&mut self, tab: TabId) -> Option<&mut Tab> {
        self.windows.iter_mut().find_map(|w| w.tab_mut(tab))
    }

    /// The URL new tabs open with, from settings, falling back to
    /// `about:blank` if the setting does not parse.
    pub fn new_tab_url(&self) -> Url {
        Url::parse(&self.settings.new_tab_url)
            .unwrap_or_else(|_| Url::parse("about:blank").expect("about:blank parses"))
    }

    /// The homepage from settings, falling back to the new-tab URL.
    pub fn homepage(&self) -> Url {
        Url::parse(&self.settings.homepage).unwrap_or_else(|_| self.new_tab_url())
    }

    pub(crate) fn allocate_window_id(&mut self) -> WindowId {
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;
        id
    }

    pub(crate) fn allocate_tab_id(&mut self) -> TabId {
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;
        id
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;

    use super::*;

    /// A browser with no windows, a throwaway profile, and default settings.
    pub(crate) fn browser() -> Browser {
        let profile = Profile {
            name: "test".to_string(),
            dir: PathBuf::from("/nonexistent/nautilus-test-profile"),
        };
        Browser::new(profile, Settings::default())
    }

    pub(crate) fn url(s: &str) -> Url {
        Url::parse(s).expect("test URL parses")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{browser, url};
    use super::*;

    #[test]
    fn display_title_falls_back_to_url() {
        let mut tab = Tab::new(TabId(1), url("https://example.com/"));
        assert_eq!(tab.display_title(), "https://example.com/");
        tab.title = "Example".to_string();
        assert_eq!(tab.display_title(), "Example");
    }

    #[test]
    fn new_tab_url_falls_back_to_about_blank_when_setting_is_invalid() {
        let mut b = browser();
        b.settings.new_tab_url = "not a url".to_string();
        assert_eq!(b.new_tab_url().as_str(), "about:blank");
    }

    #[test]
    fn homepage_uses_setting_when_valid() {
        let mut b = browser();
        b.settings.homepage = "https://home.example/".to_string();
        assert_eq!(b.homepage().as_str(), "https://home.example/");
    }

    #[test]
    fn homepage_falls_back_to_new_tab_url_when_invalid() {
        let mut b = browser();
        b.settings.homepage = "not a url".to_string();
        b.settings.new_tab_url = "https://n.example/".to_string();
        assert_eq!(b.homepage().as_str(), "https://n.example/");
    }

    #[test]
    fn ids_are_allocated_sequentially() {
        let mut b = browser();
        assert_eq!(b.allocate_tab_id(), TabId(1));
        assert_eq!(b.allocate_tab_id(), TabId(2));
        assert_eq!(b.allocate_window_id(), WindowId(1));
        assert_eq!(b.allocate_window_id(), WindowId(2));
    }
}
