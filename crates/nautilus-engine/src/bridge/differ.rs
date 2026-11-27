//! Two snapshots to the chrome.tabs and chrome.windows events between
//! them. Pure. `diff` says what happened in terms of the model; `render`
//! turns one happening into an event name and its arguments for one
//! extension (which may or may not be allowed to see addresses).
//!
//! The order is fixed and documented in the bridge protocol: replaced
//! ids, new windows, removed tabs, detached, created, attached, moved,
//! activated (with highlighted), updated, removed windows, focus.

use std::collections::{HashMap, HashSet};

use osmena_core::{TabId, WindowId};
use serde_json::{Map, Value, json};

use super::ids::{self, IdSpace, WINDOW_ID_NONE, group_id, window_id};
use super::shape::{View, status};
use super::snapshot::{SnapTab, Snapshot};

/// What changed about a tab that `tabs.onUpdated` reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Changed {
    pub url: bool,
    pub title: bool,
    pub status: bool,
    pub group: bool,
    pub discarded: bool,
}

impl Changed {
    fn any(self) -> bool {
        self.url || self.title || self.status || self.group || self.discarded
    }
}

/// One thing that happened to the strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StripEvent {
    Replaced {
        added: i64,
        removed: i64,
    },
    WindowCreated(WindowId),
    /// `id`: the tab's id as extensions knew it; the tab is gone.
    TabRemoved {
        id: i64,
        window: WindowId,
        window_closing: bool,
    },
    TabDetached {
        tab: TabId,
        old_window: WindowId,
        old_position: usize,
    },
    TabCreated(TabId),
    TabAttached {
        tab: TabId,
        new_window: WindowId,
        new_position: usize,
    },
    TabMoved {
        tab: TabId,
        window: WindowId,
        from: usize,
        to: usize,
    },
    TabActivated {
        tab: TabId,
        window: WindowId,
    },
    TabHighlighted {
        tab: TabId,
        window: WindowId,
    },
    TabUpdated {
        tab: TabId,
        changed: Changed,
    },
    WindowRemoved(WindowId),
    FocusChanged(Option<WindowId>),
}

/// Where each tab is: its window and index.
fn places(snapshot: &Snapshot) -> HashMap<TabId, (WindowId, usize)> {
    snapshot
        .windows
        .iter()
        .flat_map(|window| {
            window
                .tabs
                .iter()
                .enumerate()
                .map(move |(index, tab)| (tab.id, (window.id, index)))
        })
        .collect()
}

/// What happened between `prev` and `next`. `ids` must already know
/// `next`'s tabs and still know `prev`'s (prune after this); `replaced`
/// is what syncing the ids to `next` returned.
pub(crate) fn diff(
    prev: &Snapshot,
    next: &Snapshot,
    ids: &IdSpace,
    replaced: &[ids::Replaced],
) -> Vec<StripEvent> {
    let before = places(prev);
    let after = places(next);
    let mut events = Vec::new();

    // A tab extensions already knew has another id now.
    for change in replaced {
        if before.contains_key(&change.tab) && after.contains_key(&change.tab) {
            events.push(StripEvent::Replaced {
                added: change.added,
                removed: change.removed,
            });
        }
    }
    for window in &next.windows {
        if prev.window(window.id).is_none() {
            events.push(StripEvent::WindowCreated(window.id));
        }
    }
    for window in &prev.windows {
        for tab in &window.tabs {
            if !after.contains_key(&tab.id)
                && let Some(id) = ids.ext(tab.id)
            {
                events.push(StripEvent::TabRemoved {
                    id,
                    window: window.id,
                    window_closing: next.window(window.id).is_none(),
                });
            }
        }
    }
    for window in &prev.windows {
        for (index, tab) in window.tabs.iter().enumerate() {
            if after.get(&tab.id).is_some_and(|(now, _)| *now != window.id) {
                events.push(StripEvent::TabDetached {
                    tab: tab.id,
                    old_window: window.id,
                    old_position: index,
                });
            }
        }
    }
    for window in &next.windows {
        for tab in &window.tabs {
            if !before.contains_key(&tab.id) {
                events.push(StripEvent::TabCreated(tab.id));
            }
        }
    }
    for window in &next.windows {
        for (index, tab) in window.tabs.iter().enumerate() {
            if before
                .get(&tab.id)
                .is_some_and(|(was, _)| *was != window.id)
            {
                events.push(StripEvent::TabAttached {
                    tab: tab.id,
                    new_window: window.id,
                    new_position: index,
                });
            }
        }
    }
    for window in &next.windows {
        let Some(old) = prev.window(window.id) else {
            continue;
        };
        // The tabs that were in this window and still are, in their old
        // order, set into the new order's slots: what the strip would be
        // had nothing moved. Arrivals already sit where they arrived.
        let target: Vec<TabId> = window.tabs.iter().map(|tab| tab.id).collect();
        let stayed: Vec<TabId> = old
            .tabs
            .iter()
            .map(|tab| tab.id)
            .filter(|tab| after.get(tab).is_some_and(|(now, _)| *now == window.id))
            .collect();
        let staying: HashSet<TabId> = stayed.iter().copied().collect();
        let mut stayed = stayed.into_iter();
        let unmoved: Vec<TabId> = target
            .iter()
            .map(|tab| {
                if staying.contains(tab) {
                    stayed.next().expect("as many as there are slots")
                } else {
                    *tab
                }
            })
            .collect();
        events.extend(moves(window.id, unmoved, &target));
    }
    for window in &next.windows {
        let active = window.active().map(|tab| tab.id);
        let was = prev
            .window(window.id)
            .and_then(|old| old.active())
            .map(|tab| tab.id);
        if let Some(tab) = active
            && active != was
        {
            events.push(StripEvent::TabActivated {
                tab,
                window: window.id,
            });
            events.push(StripEvent::TabHighlighted {
                tab,
                window: window.id,
            });
        }
    }
    let old_tabs: HashMap<TabId, &SnapTab> = prev
        .windows
        .iter()
        .flat_map(|window| window.tabs.iter().map(|tab| (tab.id, tab)))
        .collect();
    for window in &next.windows {
        for tab in &window.tabs {
            let Some(old) = old_tabs.get(&tab.id) else {
                continue;
            };
            let changed = Changed {
                url: old.url != tab.url,
                title: old.title != tab.title,
                status: status(old) != status(tab),
                group: old.folder != tab.folder,
                discarded: old.deferred != tab.deferred,
            };
            if changed.any() {
                events.push(StripEvent::TabUpdated {
                    tab: tab.id,
                    changed,
                });
            }
        }
    }
    for window in &prev.windows {
        if next.window(window.id).is_none() {
            events.push(StripEvent::WindowRemoved(window.id));
        }
    }
    let focused = |snapshot: &Snapshot| {
        snapshot
            .windows
            .iter()
            .find(|window| window.focused)
            .map(|window| window.id)
    };
    if focused(prev) != focused(next) {
        events.push(StripEvent::FocusChanged(focused(next)));
    }
    events
}

/// The moves that turn `order` into `target` (the same tabs). One move
/// when taking a single tab out of both leaves them equal, which is what
/// a drag does; otherwise each tab that is out of place is lifted into
/// its place, left to right, the indexes being those at that moment.
fn moves(window: WindowId, mut order: Vec<TabId>, target: &[TabId]) -> Vec<StripEvent> {
    if order == target {
        return Vec::new();
    }
    let without = |list: &[TabId], tab: TabId| -> Vec<TabId> {
        list.iter().copied().filter(|t| *t != tab).collect()
    };
    for (from, &tab) in order.iter().enumerate() {
        let to = target
            .iter()
            .position(|t| *t == tab)
            .expect("the same tabs in both orders");
        if from != to && without(&order, tab) == without(target, tab) {
            return vec![StripEvent::TabMoved {
                tab,
                window,
                from,
                to,
            }];
        }
    }
    let mut events = Vec::new();
    for to in 0..target.len() {
        if order[to] == target[to] {
            continue;
        }
        let from = order
            .iter()
            .position(|t| *t == target[to])
            .expect("the same tabs in both orders");
        let tab = order.remove(from);
        order.insert(to, tab);
        events.push(StripEvent::TabMoved {
            tab,
            window,
            from,
            to,
        });
    }
    events
}

/// The event `event` is to the extension `view` is for: its name (as
/// listeners are registered under) and arguments, or `None` when there is
/// nothing to tell this extension.
pub(crate) fn render(event: &StripEvent, view: &View) -> Option<(&'static str, Vec<Value>)> {
    let ext = |tab: TabId| view.ids.ext(tab);
    Some(match event {
        StripEvent::Replaced { added, removed } => {
            ("tabs.onReplaced", vec![json!(added), json!(removed)])
        }
        StripEvent::WindowCreated(window) => (
            "windows.onCreated",
            vec![view.window_json(view.snapshot.window(*window)?, false)],
        ),
        StripEvent::TabRemoved {
            id,
            window,
            window_closing,
        } => (
            "tabs.onRemoved",
            vec![
                json!(id),
                json!({ "windowId": window_id(*window), "isWindowClosing": window_closing }),
            ],
        ),
        StripEvent::TabDetached {
            tab,
            old_window,
            old_position,
        } => (
            "tabs.onDetached",
            vec![
                json!(ext(*tab)?),
                json!({ "oldWindowId": window_id(*old_window), "oldPosition": old_position }),
            ],
        ),
        StripEvent::TabCreated(tab) => {
            let (window, index) = view.snapshot.locate(*tab)?;
            ("tabs.onCreated", vec![view.tab_json(window, index)])
        }
        StripEvent::TabAttached {
            tab,
            new_window,
            new_position,
        } => (
            "tabs.onAttached",
            vec![
                json!(ext(*tab)?),
                json!({ "newWindowId": window_id(*new_window), "newPosition": new_position }),
            ],
        ),
        StripEvent::TabMoved {
            tab,
            window,
            from,
            to,
        } => (
            "tabs.onMoved",
            vec![
                json!(ext(*tab)?),
                json!({ "windowId": window_id(*window), "fromIndex": from, "toIndex": to }),
            ],
        ),
        StripEvent::TabActivated { tab, window } => (
            "tabs.onActivated",
            vec![json!({ "tabId": ext(*tab)?, "windowId": window_id(*window) })],
        ),
        StripEvent::TabHighlighted { tab, window } => (
            "tabs.onHighlighted",
            vec![json!({ "windowId": window_id(*window), "tabIds": [ext(*tab)?] })],
        ),
        StripEvent::TabUpdated { tab, changed } => {
            let (window, index) = view.snapshot.locate(*tab)?;
            let now = &window.tabs[index];
            let seen = view.grants.sees(&now.url);
            let mut info = Map::new();
            if changed.status {
                info.insert("status".to_string(), json!(status(now)));
            }
            if changed.url && seen {
                info.insert("url".to_string(), json!(now.url));
            }
            if changed.title && seen {
                info.insert("title".to_string(), json!(now.title));
            }
            if changed.group {
                info.insert("groupId".to_string(), json!(group_id(now.folder)));
            }
            if changed.discarded {
                info.insert("discarded".to_string(), json!(now.deferred));
            }
            if info.is_empty() {
                return None;
            }
            (
                "tabs.onUpdated",
                vec![
                    json!(ext(*tab)?),
                    Value::Object(info),
                    view.tab_json(window, index),
                ],
            )
        }
        StripEvent::WindowRemoved(window) => ("windows.onRemoved", vec![json!(window_id(*window))]),
        StripEvent::FocusChanged(window) => (
            "windows.onFocusChanged",
            vec![json!(window.map_or(WINDOW_ID_NONE, window_id))],
        ),
    })
}

#[cfg(test)]
mod tests {
    use osmena_core::{FolderId, LoadState, TabId, WindowId};
    use serde_json::json;

    use super::StripEvent::*;
    use super::*;
    use crate::bridge::ids::{IdSpace, SYNTHETIC_BASE};
    use crate::bridge::pattern::Grants;
    use crate::bridge::shape::View;
    use crate::bridge::snapshot::Snapshot;
    use crate::bridge::snapshot::tests::{snapshot, tab, window};

    fn pairs(snapshot: &Snapshot) -> Vec<(TabId, Option<i64>)> {
        snapshot
            .windows
            .iter()
            .flat_map(|w| &w.tabs)
            .map(|t| (t.id, (!t.deferred).then_some(100 + t.id.0 as i64)))
            .collect()
    }

    /// The events from `prev` to `next`, live tabs being 100 + their id
    /// to Chromium, and the ids as they stand afterwards.
    fn run(prev: &Snapshot, next: &Snapshot) -> (Vec<StripEvent>, IdSpace) {
        let mut ids = IdSpace::default();
        ids.sync(&pairs(prev));
        let replaced = ids.sync(&pairs(next));
        (diff(prev, next, &ids, &replaced), ids)
    }

    const W1: WindowId = WindowId(1);
    const W2: WindowId = WindowId(2);

    #[test]
    fn each_change_yields_its_events_in_order() {
        let one = |tabs| snapshot(vec![window(1, true, tabs)]);
        let navigated = {
            let mut t = tab(2, "b");
            t.url = "https://b.example/next".to_string();
            t.title = "Next".to_string();
            t.load = LoadState::Loading;
            t
        };
        let foldered = {
            let mut t = tab(2, "b");
            t.folder = Some(FolderId(7));
            t
        };
        let all = Changed {
            url: true,
            title: true,
            status: true,
            group: false,
            discarded: false,
        };
        let rows: Vec<(&str, Snapshot, Snapshot, Vec<StripEvent>)> = vec![
            (
                "nothing changed",
                one(vec![tab(1, "a*"), tab(2, "b")]),
                one(vec![tab(1, "a*"), tab(2, "b")]),
                vec![],
            ),
            (
                "a tab opened behind",
                one(vec![tab(1, "a*"), tab(2, "b")]),
                one(vec![tab(1, "a*"), tab(2, "b"), tab(3, "c")]),
                vec![TabCreated(TabId(3))],
            ),
            (
                "a tab opened in front",
                one(vec![tab(1, "a*"), tab(2, "b")]),
                one(vec![tab(1, "a"), tab(2, "b"), tab(3, "c*")]),
                vec![
                    TabCreated(TabId(3)),
                    TabActivated {
                        tab: TabId(3),
                        window: W1,
                    },
                    TabHighlighted {
                        tab: TabId(3),
                        window: W1,
                    },
                ],
            ),
            (
                "a tab opened beside its opener moves nothing",
                one(vec![tab(1, "a*"), tab(2, "b")]),
                one(vec![tab(1, "a*"), tab(3, "c"), tab(2, "b")]),
                vec![TabCreated(TabId(3))],
            ),
            (
                "a tab closed",
                one(vec![tab(1, "a*"), tab(2, "b"), tab(3, "c")]),
                one(vec![tab(1, "a*"), tab(3, "c")]),
                vec![TabRemoved {
                    id: 102,
                    window: W1,
                    window_closing: false,
                }],
            ),
            (
                "the active tab closed",
                one(vec![tab(1, "a*"), tab(2, "b")]),
                one(vec![tab(2, "b*")]),
                vec![
                    TabRemoved {
                        id: 101,
                        window: W1,
                        window_closing: false,
                    },
                    TabActivated {
                        tab: TabId(2),
                        window: W1,
                    },
                    TabHighlighted {
                        tab: TabId(2),
                        window: W1,
                    },
                ],
            ),
            (
                "one tab dragged to the end is one move",
                one(vec![tab(1, "a*"), tab(2, "b"), tab(3, "c")]),
                one(vec![tab(2, "b"), tab(3, "c"), tab(1, "a*")]),
                vec![TabMoved {
                    tab: TabId(1),
                    window: W1,
                    from: 0,
                    to: 2,
                }],
            ),
            (
                "a reversal is a move per lifted tab, left to right",
                one(vec![tab(1, "a*"), tab(2, "b"), tab(3, "c"), tab(4, "d")]),
                one(vec![tab(4, "d"), tab(3, "c"), tab(2, "b"), tab(1, "a*")]),
                vec![
                    TabMoved {
                        tab: TabId(4),
                        window: W1,
                        from: 3,
                        to: 0,
                    },
                    TabMoved {
                        tab: TabId(3),
                        window: W1,
                        from: 3,
                        to: 1,
                    },
                    TabMoved {
                        tab: TabId(2),
                        window: W1,
                        from: 3,
                        to: 2,
                    },
                ],
            ),
            (
                "a navigation",
                one(vec![tab(1, "a*"), tab(2, "b")]),
                one(vec![tab(1, "a*"), navigated]),
                vec![TabUpdated {
                    tab: TabId(2),
                    changed: all,
                }],
            ),
            (
                "a tab put in a folder",
                one(vec![tab(1, "a*"), tab(2, "b")]),
                one(vec![tab(1, "a*"), foldered]),
                vec![TabUpdated {
                    tab: TabId(2),
                    changed: Changed {
                        group: true,
                        ..Changed::default()
                    },
                }],
            ),
            (
                "a deferred tab shown",
                one(vec![tab(1, "a*"), tab(2, "b~")]),
                one(vec![tab(1, "a"), tab(2, "b*")]),
                vec![
                    Replaced {
                        added: 102,
                        removed: SYNTHETIC_BASE,
                    },
                    TabActivated {
                        tab: TabId(2),
                        window: W1,
                    },
                    TabHighlighted {
                        tab: TabId(2),
                        window: W1,
                    },
                    TabUpdated {
                        tab: TabId(2),
                        changed: Changed {
                            status: true,
                            discarded: true,
                            ..Changed::default()
                        },
                    },
                ],
            ),
            (
                "a window opened and focused",
                snapshot(vec![window(1, true, vec![tab(1, "a*")])]),
                snapshot(vec![
                    window(1, false, vec![tab(1, "a*")]),
                    window(2, true, vec![tab(2, "b*")]),
                ]),
                vec![
                    WindowCreated(W2),
                    TabCreated(TabId(2)),
                    TabActivated {
                        tab: TabId(2),
                        window: W2,
                    },
                    TabHighlighted {
                        tab: TabId(2),
                        window: W2,
                    },
                    FocusChanged(Some(W2)),
                ],
            ),
            (
                "a window closed",
                snapshot(vec![
                    window(1, true, vec![tab(1, "a*")]),
                    window(2, false, vec![tab(2, "b*"), tab(3, "c")]),
                ]),
                snapshot(vec![window(1, true, vec![tab(1, "a*")])]),
                vec![
                    TabRemoved {
                        id: 102,
                        window: W2,
                        window_closing: true,
                    },
                    TabRemoved {
                        id: 103,
                        window: W2,
                        window_closing: true,
                    },
                    WindowRemoved(W2),
                ],
            ),
            (
                "the browser lost focus",
                snapshot(vec![window(1, true, vec![tab(1, "a*")])]),
                snapshot(vec![window(1, false, vec![tab(1, "a*")])]),
                vec![FocusChanged(None)],
            ),
            (
                "a tab changed window",
                snapshot(vec![
                    window(1, true, vec![tab(1, "a*"), tab(2, "b")]),
                    window(2, false, vec![tab(3, "c*")]),
                ]),
                snapshot(vec![
                    window(1, true, vec![tab(1, "a*")]),
                    window(2, false, vec![tab(3, "c*"), tab(2, "b")]),
                ]),
                vec![
                    TabDetached {
                        tab: TabId(2),
                        old_window: W1,
                        old_position: 1,
                    },
                    TabAttached {
                        tab: TabId(2),
                        new_window: W2,
                        new_position: 1,
                    },
                ],
            ),
            (
                "closed, opened, moved and activated at once",
                one(vec![tab(1, "a*"), tab(2, "b"), tab(3, "c")]),
                one(vec![tab(3, "c*"), tab(1, "a"), tab(4, "d")]),
                vec![
                    TabRemoved {
                        id: 102,
                        window: W1,
                        window_closing: false,
                    },
                    TabCreated(TabId(4)),
                    TabMoved {
                        tab: TabId(1),
                        window: W1,
                        from: 0,
                        to: 1,
                    },
                    TabActivated {
                        tab: TabId(3),
                        window: W1,
                    },
                    TabHighlighted {
                        tab: TabId(3),
                        window: W1,
                    },
                ],
            ),
        ];
        for (name, prev, next, expected) in rows {
            assert_eq!(run(&prev, &next).0, expected, "{name}");
        }
    }

    #[test]
    fn events_render_as_chromes_payloads() {
        let prev = snapshot(vec![
            window(1, true, vec![tab(1, "a*"), tab(2, "b")]),
            window(2, false, vec![tab(5, "e*")]),
        ]);
        let mut moved = tab(2, "b");
        moved.url = "https://b.example/next".to_string();
        moved.folder = Some(FolderId(7));
        let next = snapshot(vec![window(
            1,
            true,
            vec![moved, tab(1, "a*"), tab(3, "c")],
        )]);
        let (events, ids) = run(&prev, &next);
        let all = Grants {
            tabs: true,
            ..Grants::default()
        };
        let view = View {
            snapshot: &next,
            ids: &ids,
            grants: &all,
            last_focused: None,
            panels: &std::collections::HashMap::new(),
        };
        let rendered: Vec<(&str, serde_json::Value)> = events
            .iter()
            .filter_map(|event| render(event, &view))
            .map(|(name, args)| (name, json!(args)))
            .collect();
        let created = view.tab_json(&next.windows[0], 2);
        let updated = view.tab_json(&next.windows[0], 0);
        assert_eq!(
            rendered,
            vec![
                (
                    "tabs.onRemoved",
                    json!([105, { "windowId": 2, "isWindowClosing": true }])
                ),
                ("tabs.onCreated", json!([created])),
                (
                    "tabs.onMoved",
                    json!([101, { "windowId": 1, "fromIndex": 0, "toIndex": 1 }])
                ),
                (
                    "tabs.onUpdated",
                    json!([102, { "url": "https://b.example/next", "groupId": 7 }, updated])
                ),
                ("windows.onRemoved", json!([2])),
            ]
        );
        // What the other events look like.
        for (event, expected) in [
            (
                Replaced {
                    added: 102,
                    removed: SYNTHETIC_BASE,
                },
                ("tabs.onReplaced", json!([102, SYNTHETIC_BASE])),
            ),
            (
                TabActivated {
                    tab: TabId(1),
                    window: W1,
                },
                ("tabs.onActivated", json!([{ "tabId": 101, "windowId": 1 }])),
            ),
            (
                TabHighlighted {
                    tab: TabId(1),
                    window: W1,
                },
                (
                    "tabs.onHighlighted",
                    json!([{ "windowId": 1, "tabIds": [101] }]),
                ),
            ),
            (
                TabDetached {
                    tab: TabId(1),
                    old_window: W2,
                    old_position: 3,
                },
                (
                    "tabs.onDetached",
                    json!([101, { "oldWindowId": 2, "oldPosition": 3 }]),
                ),
            ),
            (
                TabAttached {
                    tab: TabId(1),
                    new_window: W1,
                    new_position: 1,
                },
                (
                    "tabs.onAttached",
                    json!([101, { "newWindowId": 1, "newPosition": 1 }]),
                ),
            ),
            (
                WindowCreated(W1),
                (
                    "windows.onCreated",
                    json!([{ "id": 1, "focused": true, "incognito": false, "alwaysOnTop": false, "type": "normal", "state": "normal" }]),
                ),
            ),
            (
                FocusChanged(Some(W1)),
                ("windows.onFocusChanged", json!([1])),
            ),
            (FocusChanged(None), ("windows.onFocusChanged", json!([-1]))),
        ] {
            let (name, args) = render(&event, &view).unwrap();
            assert_eq!((name, json!(args)), expected);
        }
        // An extension that may not see the page is told what changed
        // without the address, and nothing when only the address did.
        let none = Grants::default();
        let blind = View {
            snapshot: &next,
            ids: &ids,
            grants: &none,
            last_focused: None,
            panels: &std::collections::HashMap::new(),
        };
        let update = TabUpdated {
            tab: TabId(2),
            changed: Changed {
                url: true,
                group: true,
                ..Changed::default()
            },
        };
        let (_, args) = render(&update, &blind).unwrap();
        assert_eq!(args[1], json!({ "groupId": 7 }));
        assert_eq!(args[2].get("url"), None);
        let only_url = TabUpdated {
            tab: TabId(2),
            changed: Changed {
                url: true,
                title: true,
                ..Changed::default()
            },
        };
        assert_eq!(render(&only_url, &blind), None);
        // An event about a tab that is gone again renders nothing.
        assert_eq!(render(&TabCreated(TabId(99)), &view), None);
    }
}
