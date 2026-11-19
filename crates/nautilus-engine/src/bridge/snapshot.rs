//! The strip as the shell last described it, owned, so two can be compared.

use std::collections::HashSet;

use osmena_core::{Folder, FolderId, LoadState, TabId, WindowId};
use osmena_engine::WindowStrip;

/// Every window's strip, in window order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub windows: Vec<SnapWindow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SnapWindow {
    pub id: WindowId,
    pub focused: bool,
    /// Strip order.
    pub tabs: Vec<SnapTab>,
    pub folders: Vec<Folder>,
    /// The view of the panel the window shows, which is in no strip.
    pub panel_view: Option<TabId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SnapTab {
    pub id: TabId,
    pub url: String,
    pub title: String,
    pub active: bool,
    pub folder: Option<FolderId>,
    pub load: LoadState,
    pub deferred: bool,
}

impl Snapshot {
    /// An owned copy of what the shell lent.
    pub(crate) fn copy_of(windows: &[WindowStrip<'_>]) -> Snapshot {
        Snapshot {
            windows: windows
                .iter()
                .map(|window| SnapWindow {
                    id: window.window,
                    focused: window.focused,
                    tabs: window
                        .tabs
                        .iter()
                        .map(|tab| SnapTab {
                            id: tab.id,
                            url: tab.url.as_str().to_string(),
                            title: tab.title.to_string(),
                            active: tab.active,
                            folder: tab.folder,
                            load: tab.load,
                            deferred: tab.deferred,
                        })
                        .collect(),
                    folders: window.folders.to_vec(),
                    panel_view: window.shown_panel.map(|panel| panel.view),
                })
                .collect(),
        }
    }

    pub(crate) fn window(&self, id: WindowId) -> Option<&SnapWindow> {
        self.windows.iter().find(|window| window.id == id)
    }

    /// The window holding `tab` in its strip, and the tab's index there.
    pub(crate) fn locate(&self, tab: TabId) -> Option<(&SnapWindow, usize)> {
        self.windows.iter().find_map(|window| {
            let index = window.tabs.iter().position(|t| t.id == tab)?;
            Some((window, index))
        })
    }

    /// The window a view belongs to: as a tab of its strip, or as the
    /// panel it shows.
    pub(crate) fn window_of_view(&self, view: TabId) -> Option<&SnapWindow> {
        self.locate(view).map(|(window, _)| window).or_else(|| {
            self.windows
                .iter()
                .find(|window| window.panel_view == Some(view))
        })
    }

    pub(crate) fn tab_ids(&self) -> HashSet<TabId> {
        self.windows
            .iter()
            .flat_map(|window| window.tabs.iter().map(|tab| tab.id))
            .collect()
    }
}

impl SnapWindow {
    pub(crate) fn active(&self) -> Option<&SnapTab> {
        self.tabs.iter().find(|tab| tab.active)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use osmena_core::{Folder, FolderId, LoadState, SidePanel, TabId, WindowId};
    use osmena_engine::{StripTab, WindowStrip};
    use url::Url;

    use super::*;

    /// A tab for tests: `"a*"` is active, `"a~"` deferred; the URL is
    /// `https://<name>.example/` and the title the name in capitals.
    pub(crate) fn tab(id: u64, spec: &str) -> SnapTab {
        let name = spec.trim_end_matches(['*', '~']);
        SnapTab {
            id: TabId(id),
            url: format!("https://{name}.example/"),
            title: name.to_uppercase(),
            active: spec.ends_with('*'),
            folder: None,
            load: LoadState::Complete,
            deferred: spec.ends_with('~'),
        }
    }

    pub(crate) fn window(id: u64, focused: bool, tabs: Vec<SnapTab>) -> SnapWindow {
        SnapWindow {
            id: WindowId(id),
            focused,
            tabs,
            folders: Vec::new(),
            panel_view: None,
        }
    }

    pub(crate) fn snapshot(windows: Vec<SnapWindow>) -> Snapshot {
        Snapshot { windows }
    }

    #[test]
    fn a_copy_holds_everything_the_strip_said() {
        let first = Url::parse("https://a.example/").unwrap();
        let second = Url::parse("https://b.example/").unwrap();
        let tabs = [
            StripTab {
                id: TabId(1),
                url: &first,
                title: "A",
                active: true,
                folder: Some(FolderId(7)),
                load: LoadState::Loading,
                deferred: false,
            },
            StripTab {
                id: TabId(2),
                url: &second,
                title: "B",
                active: false,
                folder: None,
                load: LoadState::Idle,
                deferred: true,
            },
        ];
        let folders = [Folder {
            id: FolderId(7),
            name: "Work".to_string(),
            expanded: true,
        }];
        let panel = SidePanel {
            extension: "abcdefghijklmnopabcdefghijklmnop".to_string(),
            page: "panel.html".to_string(),
            view: TabId(9),
        };
        let copy = Snapshot::copy_of(&[WindowStrip {
            window: WindowId(1),
            focused: true,
            tabs: &tabs,
            folders: &folders,
            shown_panel: Some(&panel),
        }]);
        assert_eq!(
            copy,
            Snapshot {
                windows: vec![SnapWindow {
                    id: WindowId(1),
                    focused: true,
                    tabs: vec![
                        SnapTab {
                            id: TabId(1),
                            url: "https://a.example/".to_string(),
                            title: "A".to_string(),
                            active: true,
                            folder: Some(FolderId(7)),
                            load: LoadState::Loading,
                            deferred: false,
                        },
                        SnapTab {
                            id: TabId(2),
                            url: "https://b.example/".to_string(),
                            title: "B".to_string(),
                            active: false,
                            folder: None,
                            load: LoadState::Idle,
                            deferred: true,
                        },
                    ],
                    folders: folders.to_vec(),
                    panel_view: Some(TabId(9)),
                }]
            }
        );
    }

    #[test]
    fn a_view_is_found_in_a_strip_or_as_a_shown_panel() {
        let mut second = window(2, false, vec![tab(3, "c*")]);
        second.panel_view = Some(TabId(9));
        let s = snapshot(vec![
            window(1, true, vec![tab(1, "a*"), tab(2, "b")]),
            second,
        ]);
        assert_eq!(
            s.locate(TabId(2)).map(|(w, i)| (w.id, i)),
            Some((WindowId(1), 1))
        );
        assert_eq!(s.locate(TabId(9)), None);
        assert_eq!(s.window_of_view(TabId(3)).map(|w| w.id), Some(WindowId(2)));
        assert_eq!(s.window_of_view(TabId(9)).map(|w| w.id), Some(WindowId(2)));
        assert_eq!(s.window_of_view(TabId(50)), None);
        assert_eq!(s.window(WindowId(2)).map(|w| w.tabs.len()), Some(1));
        assert_eq!(s.tab_ids().len(), 3);
    }
}
