//! The ids extensions see: Chromium's for live tabs, synthetic ones for the rest, osmena's for windows and groups.

use std::collections::{HashMap, HashSet};

use osmena_core::{FolderId, TabId, WindowId};

pub(crate) const TAB_ID_NONE: i64 = -1;
pub(crate) const WINDOW_ID_NONE: i64 = -1;
pub(crate) const WINDOW_ID_CURRENT: i64 = -2;
pub(crate) const TAB_GROUP_ID_NONE: i64 = -1;

/// Where synthetic tab ids start. Chromium's ids are positive 32-bit
/// session ids from a random start, so no range is free of them; a
/// synthetic id is only ever one no live tab holds (see `sync`).
pub(crate) const SYNTHETIC_BASE: i64 = 2_000_000_000;
const SYNTHETIC_END: i64 = i32::MAX as i64;

/// The window id extensions see: the model's own, which counts from 1.
pub(crate) fn window_id(window: WindowId) -> i64 {
    window.0 as i64
}

pub(crate) fn window_from(id: i64) -> Option<WindowId> {
    (id >= 1).then_some(WindowId(id as u64))
}

/// The group id extensions see for a tab's folder.
pub(crate) fn group_id(folder: Option<FolderId>) -> i64 {
    folder.map_or(TAB_GROUP_ID_NONE, |folder| folder.0 as i64)
}

/// A tab extensions knew as `removed` is `added` from now on
/// (`chrome.tabs.onReplaced`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Replaced {
    pub tab: TabId,
    pub added: i64,
    pub removed: i64,
}

/// Every tab's id as extensions see it, and every id back to its tab.
#[derive(Debug, Default)]
pub(crate) struct IdSpace {
    /// The id each tab has now.
    current: HashMap<TabId, i64>,
    /// The synthetic id a tab was given, kept so it gets the same one
    /// again.
    synthetic: HashMap<TabId, i64>,
    /// Every id a tab has or had, so an extension holding an old one is
    /// still understood.
    owner: HashMap<i64, TabId>,
    next_synthetic: i64,
}

impl IdSpace {
    /// The id extensions see for `tab`.
    pub(crate) fn ext(&self, tab: TabId) -> Option<i64> {
        self.current.get(&tab).copied()
    }

    /// The tab an id names, current or former.
    pub(crate) fn tab(&self, id: i64) -> Option<TabId> {
        self.owner.get(&id).copied()
    }

    /// Whether `tab`'s id is Chromium's, so Chromium can be asked about it.
    pub(crate) fn is_live(&self, tab: TabId) -> bool {
        self.ext(tab)
            .is_some_and(|id| self.synthetic.get(&tab) != Some(&id))
    }

    /// Bring the ids in line with `tabs`: each tab with the Chromium id it
    /// should be known by, or `None` for a synthetic one. Returns the tabs
    /// whose id changed. Tabs not named keep what they have until `prune`.
    pub(crate) fn sync(&mut self, tabs: &[(TabId, Option<i64>)]) -> Vec<Replaced> {
        let mut replaced = Vec::new();
        // The id a tab lost to a live tab, to report when it gets another.
        let mut displaced: HashMap<TabId, i64> = HashMap::new();
        for &(tab, chromium) in tabs {
            let Some(live) = chromium else { continue };
            if self.current.get(&tab) == Some(&live) {
                continue;
            }
            if let Some(&other) = self.owner.get(&live)
                && other != tab
            {
                if self.synthetic.get(&other) == Some(&live) {
                    self.synthetic.remove(&other);
                }
                if self.current.get(&other) == Some(&live) {
                    self.current.remove(&other);
                    displaced.insert(other, live);
                }
            }
            self.owner.insert(live, tab);
            if let Some(removed) = self.current.insert(tab, live) {
                replaced.push(Replaced {
                    tab,
                    added: live,
                    removed,
                });
            }
        }
        for &(tab, chromium) in tabs {
            if chromium.is_some() {
                continue;
            }
            let id = self.synthetic_for(tab);
            let before = self.current.insert(tab, id).or(displaced.remove(&tab));
            if let Some(removed) = before
                && removed != id
            {
                replaced.push(Replaced {
                    tab,
                    added: id,
                    removed,
                });
            }
        }
        replaced
    }

    fn synthetic_for(&mut self, tab: TabId) -> i64 {
        if let Some(&id) = self.synthetic.get(&tab) {
            return id;
        }
        let span = SYNTHETIC_END - SYNTHETIC_BASE + 1;
        let id = loop {
            let candidate = SYNTHETIC_BASE + self.next_synthetic % span;
            self.next_synthetic += 1;
            if !self.owner.contains_key(&candidate) {
                break candidate;
            }
        };
        self.synthetic.insert(tab, id);
        self.owner.insert(id, tab);
        id
    }

    /// Forget every tab not in `keep`, with all its ids.
    pub(crate) fn prune(&mut self, keep: &HashSet<TabId>) {
        self.current.retain(|tab, _| keep.contains(tab));
        self.synthetic.retain(|tab, _| keep.contains(tab));
        self.owner.retain(|_, tab| keep.contains(tab));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use osmena_core::{FolderId, TabId, WindowId};

    use super::*;

    #[test]
    fn a_live_tab_is_known_by_chromiums_id() {
        let mut ids = IdSpace::default();
        let replaced = ids.sync(&[(TabId(1), Some(900_927_761))]);
        assert_eq!(replaced, vec![]);
        assert_eq!(ids.ext(TabId(1)), Some(900_927_761));
        assert_eq!(ids.tab(900_927_761), Some(TabId(1)));
        assert!(ids.is_live(TabId(1)));
    }

    #[test]
    fn a_deferred_tab_gets_a_synthetic_id() {
        let mut ids = IdSpace::default();
        ids.sync(&[(TabId(1), Some(41)), (TabId(2), None), (TabId(3), None)]);
        assert_eq!(ids.ext(TabId(2)), Some(SYNTHETIC_BASE));
        assert_eq!(ids.ext(TabId(3)), Some(SYNTHETIC_BASE + 1));
        assert!(!ids.is_live(TabId(2)));
        // The same id on every later sync.
        assert_eq!(
            ids.sync(&[(TabId(1), Some(41)), (TabId(2), None), (TabId(3), None)]),
            vec![]
        );
        assert_eq!(ids.ext(TabId(2)), Some(SYNTHETIC_BASE));
    }

    #[test]
    fn showing_a_deferred_tab_replaces_its_id_and_the_old_one_still_resolves() {
        let mut ids = IdSpace::default();
        ids.sync(&[(TabId(2), None)]);
        let replaced = ids.sync(&[(TabId(2), Some(77))]);
        assert_eq!(
            replaced,
            vec![Replaced {
                tab: TabId(2),
                added: 77,
                removed: SYNTHETIC_BASE
            }]
        );
        assert_eq!(ids.ext(TabId(2)), Some(77));
        assert!(ids.is_live(TabId(2)));
        assert_eq!(ids.tab(SYNTHETIC_BASE), Some(TabId(2)));
        assert_eq!(ids.tab(77), Some(TabId(2)));
    }

    #[test]
    fn a_tab_that_loses_its_browser_goes_back_to_its_synthetic_id() {
        let mut ids = IdSpace::default();
        ids.sync(&[(TabId(2), None)]);
        ids.sync(&[(TabId(2), Some(77))]);
        let replaced = ids.sync(&[(TabId(2), None)]);
        assert_eq!(
            replaced,
            vec![Replaced {
                tab: TabId(2),
                added: SYNTHETIC_BASE,
                removed: 77
            }]
        );
        assert_eq!(ids.tab(77), Some(TabId(2)));
    }

    #[test]
    fn a_chromium_id_always_wins_over_a_synthetic_one() {
        let mut ids = IdSpace::default();
        ids.sync(&[(TabId(1), None)]);
        // Chromium hands out the very number tab 1 was given.
        let replaced = ids.sync(&[(TabId(1), None), (TabId(2), Some(SYNTHETIC_BASE))]);
        assert_eq!(ids.ext(TabId(2)), Some(SYNTHETIC_BASE));
        assert_eq!(ids.tab(SYNTHETIC_BASE), Some(TabId(2)));
        assert_eq!(ids.ext(TabId(1)), Some(SYNTHETIC_BASE + 1));
        assert_eq!(
            replaced,
            vec![Replaced {
                tab: TabId(1),
                added: SYNTHETIC_BASE + 1,
                removed: SYNTHETIC_BASE
            }]
        );
        // And a new synthetic id never lands on a live one.
        ids.sync(&[
            (TabId(1), None),
            (TabId(2), Some(SYNTHETIC_BASE)),
            (TabId(3), Some(SYNTHETIC_BASE + 2)),
            (TabId(4), None),
        ]);
        assert_eq!(ids.ext(TabId(4)), Some(SYNTHETIC_BASE + 3));
    }

    #[test]
    fn pruning_forgets_closed_tabs_and_their_old_ids() {
        let mut ids = IdSpace::default();
        ids.sync(&[(TabId(1), Some(5)), (TabId(2), None)]);
        ids.sync(&[(TabId(1), Some(5)), (TabId(2), Some(6))]);
        ids.prune(&HashSet::from([TabId(1)]));
        assert_eq!(ids.ext(TabId(2)), None);
        assert_eq!(ids.tab(6), None);
        assert_eq!(ids.tab(SYNTHETIC_BASE), None);
        assert_eq!(ids.tab(5), Some(TabId(1)));
    }

    #[test]
    fn window_and_group_ids_are_the_models_and_never_a_reserved_value() {
        assert_eq!(window_id(WindowId(1)), 1);
        assert_eq!(window_from(3), Some(WindowId(3)));
        for reserved in [0, WINDOW_ID_NONE, WINDOW_ID_CURRENT] {
            assert_eq!(window_from(reserved), None);
        }
        assert_eq!(group_id(Some(FolderId(4))), 4);
        assert_eq!(group_id(None), TAB_GROUP_ID_NONE);
    }
}
