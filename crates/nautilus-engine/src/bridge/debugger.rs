//! Which extension is debugging which tab, and so which tabs must keep
//! painting. The bridge reports each `chrome.debugger` attach and detach
//! its worker makes (`js/bridge.js`); the engine keeps the set here and
//! un-hides a debugged tab, hiding it again when its last debugger goes.
//!
//! The commands and events of `chrome.debugger` extend this module, and build
//! on this set rather than keeping a second one beside it. Pure: no CEF
//! types, so the tests run anywhere.

use std::collections::{BTreeMap, BTreeSet};

use osmena_core::TabId;

/// What a change to the set did to one tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// The tab had no debugger and now has one: it must be shown.
    Debugged,
    /// The tab's last debugger went: it may be hidden again.
    Undebugged,
    /// The tab's state did not change.
    Unchanged,
}

/// The extensions debugging each tab, by extension id.
#[derive(Default, Debug)]
pub(crate) struct Debuggers {
    by_tab: BTreeMap<TabId, BTreeSet<String>>,
}

impl Debuggers {
    /// `extension` attached a debugger to `tab`.
    pub(crate) fn attach(&mut self, extension: &str, tab: TabId) -> Change {
        let debuggers = self.by_tab.entry(tab).or_default();
        let was_debugged = !debuggers.is_empty();
        debuggers.insert(extension.to_string());
        if was_debugged {
            Change::Unchanged
        } else {
            Change::Debugged
        }
    }

    /// `extension`'s debugger on `tab` went.
    pub(crate) fn detach(&mut self, extension: &str, tab: TabId) -> Change {
        let Some(debuggers) = self.by_tab.get_mut(&tab) else {
            return Change::Unchanged;
        };
        if !debuggers.remove(extension) {
            return Change::Unchanged;
        }
        if debuggers.is_empty() {
            self.by_tab.remove(&tab);
            Change::Undebugged
        } else {
            Change::Unchanged
        }
    }

    /// `extension` is gone (its relay closed): its debuggers go, and the
    /// tabs left with none are returned.
    pub(crate) fn forget_extension(&mut self, extension: &str) -> Vec<TabId> {
        let mut freed = Vec::new();
        self.by_tab.retain(|tab, debuggers| {
            if debuggers.remove(extension) && debuggers.is_empty() {
                freed.push(*tab);
                false
            } else {
                true
            }
        });
        freed
    }

    /// `tab` closed: no debugger is left on it.
    pub(crate) fn forget_tab(&mut self, tab: TabId) {
        self.by_tab.remove(&tab);
    }

    /// The tabs `extension` debugs, in tab order.
    pub(crate) fn tabs_of(&self, extension: &str) -> Vec<TabId> {
        self.by_tab
            .iter()
            .filter(|(_, debuggers)| debuggers.contains(extension))
            .map(|(tab, _)| *tab)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn is_debugged(&self, tab: TabId) -> bool {
        self.by_tab.contains_key(&tab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "extension-a";
    const B: &str = "extension-b";

    #[test]
    fn a_tab_is_debugged_from_its_first_debugger_until_its_last() {
        let mut set = Debuggers::default();
        assert_eq!(set.attach(A, TabId(1)), Change::Debugged);
        assert!(set.is_debugged(TabId(1)));
        assert_eq!(set.attach(B, TabId(1)), Change::Unchanged);
        assert_eq!(
            set.detach(A, TabId(1)),
            Change::Unchanged,
            "B still debugs it"
        );
        assert!(set.is_debugged(TabId(1)));
        assert_eq!(set.detach(B, TabId(1)), Change::Undebugged);
        assert!(!set.is_debugged(TabId(1)));
    }

    #[test]
    fn one_extension_attaching_twice_is_one_debugger() {
        let mut set = Debuggers::default();
        assert_eq!(set.attach(A, TabId(1)), Change::Debugged);
        assert_eq!(set.attach(A, TabId(1)), Change::Unchanged);
        assert_eq!(set.detach(A, TabId(1)), Change::Undebugged);
    }

    #[test]
    fn a_detach_that_matches_no_attach_changes_nothing() {
        let mut set = Debuggers::default();
        assert_eq!(set.detach(A, TabId(1)), Change::Unchanged);
        set.attach(A, TabId(1));
        assert_eq!(set.detach(B, TabId(1)), Change::Unchanged);
        assert!(set.is_debugged(TabId(1)));
    }

    #[test]
    fn tabs_are_independent() {
        let mut set = Debuggers::default();
        set.attach(A, TabId(1));
        assert!(!set.is_debugged(TabId(2)));
    }

    #[test]
    fn forgetting_an_extension_returns_only_the_tabs_left_with_no_debugger() {
        let mut set = Debuggers::default();
        set.attach(A, TabId(1));
        set.attach(B, TabId(1));
        set.attach(A, TabId(2));
        assert_eq!(set.forget_extension(A), vec![TabId(2)]);
        assert!(set.is_debugged(TabId(1)), "B still debugs tab 1");
        assert!(!set.is_debugged(TabId(2)));
        assert_eq!(
            set.forget_extension(A),
            Vec::new(),
            "nothing is left to forget"
        );
    }

    #[test]
    fn an_extension_lists_only_the_tabs_it_debugs_in_tab_order() {
        let mut set = Debuggers::default();
        set.attach(A, TabId(2));
        set.attach(B, TabId(1));
        set.attach(A, TabId(1));
        assert_eq!(set.tabs_of(A), vec![TabId(1), TabId(2)]);
        assert_eq!(set.tabs_of(B), vec![TabId(1)]);
        assert!(set.tabs_of("nobody").is_empty());
    }

    #[test]
    fn a_closed_tab_has_no_debugger_and_a_new_attach_starts_again() {
        let mut set = Debuggers::default();
        set.attach(A, TabId(1));
        set.forget_tab(TabId(1));
        assert!(!set.is_debugged(TabId(1)));
        assert_eq!(set.attach(A, TabId(1)), Change::Debugged);
    }
}
