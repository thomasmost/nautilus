//! Notifications from the engine and the pure `apply` that folds them into
//! the model.

use url::Url;

use crate::command::{EngineAction, close_tab};
use crate::model::{Browser, Favicon, LoadState, Tab, TabId};

/// Handle to an engine tab that page content created (`window.open`) and
/// that the engine is holding until the model adopts it with
/// [`EngineAction::AdoptTab`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PendingTab(pub u64);

/// The pointer shape a page asked for while the pointer is over it, in
/// CSS `cursor` terms (the engine maps its own cursor types to these; the
/// window maps them to the platform's cursors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Default,
    Pointer,
    Text,
    Crosshair,
    Wait,
    Progress,
    Help,
    Move,
    Grab,
    Grabbing,
    NotAllowed,
    ColResize,
    RowResize,
    NsResize,
    EwResize,
    NeswResize,
    NwseResize,
    ZoomIn,
    ZoomOut,
    /// `cursor: none`.
    Hidden,
}

/// Something the engine reported. Produced by the engine crate from Chromium
/// (CEF) handler callbacks, keyed by core ids.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    UrlChanged {
        tab: TabId,
        url: Url,
    },
    TitleChanged {
        tab: TabId,
        title: String,
    },
    LoadStateChanged {
        tab: TabId,
        state: LoadState,
    },
    HistoryChanged {
        tab: TabId,
        can_go_back: bool,
        can_go_forward: bool,
    },
    StatusChanged {
        tab: TabId,
        text: String,
    },
    FaviconChanged {
        tab: TabId,
        favicon: Favicon,
    },
    /// Page content in `opener` asked for a new tab. The engine already
    /// created it and holds it as `pending`.
    NewTabRequested {
        opener: TabId,
        pending: PendingTab,
    },
    /// Page content closed the tab (`window.close()`).
    TabClosed(TabId),
    TabCrashed {
        tab: TabId,
        reason: String,
    },
    /// The engine painted a new frame for this tab; the window showing it
    /// should redraw.
    FrameReady(TabId),
    /// The page under the pointer changed the pointer shape. Not part of
    /// the model: the window applies it directly.
    CursorChanged {
        tab: TabId,
        cursor: Cursor,
    },
    /// An engine-level error not tied to a tab (for example, losing the
    /// connection to the engine backend).
    EngineError(String),
}

/// Fold an engine event into the model. Returns follow-up actions; only
/// `NewTabRequested` and `TabClosed` produce any.
pub fn apply(browser: &mut Browser, event: &EngineEvent) -> Vec<EngineAction> {
    match event {
        EngineEvent::UrlChanged { tab, url } => {
            with_tab(browser, *tab, |t| t.url = url.clone());
            Vec::new()
        }
        EngineEvent::TitleChanged { tab, title } => {
            with_tab(browser, *tab, |t| t.title = title.clone());
            Vec::new()
        }
        EngineEvent::LoadStateChanged { tab, state } => {
            with_tab(browser, *tab, |t| t.load = *state);
            Vec::new()
        }
        EngineEvent::HistoryChanged {
            tab,
            can_go_back,
            can_go_forward,
        } => {
            with_tab(browser, *tab, |t| {
                t.can_go_back = *can_go_back;
                t.can_go_forward = *can_go_forward;
            });
            Vec::new()
        }
        EngineEvent::StatusChanged { tab, text } => {
            with_tab(browser, *tab, |t| t.status = text.clone());
            Vec::new()
        }
        EngineEvent::FaviconChanged { tab, favicon } => {
            with_tab(browser, *tab, |t| t.favicon = Some(favicon.clone()));
            Vec::new()
        }
        EngineEvent::NewTabRequested { opener, pending } => {
            let Some(window) = browser.window_of_tab(*opener) else {
                return Vec::new();
            };
            let tab = browser.allocate_tab_id();
            let w = browser
                .window_mut(window)
                .expect("window_of_tab returned an existing window");
            let previous = w.active;
            w.tabs.push(Tab::new(
                tab,
                Url::parse("about:blank").expect("about:blank parses"),
            ));
            w.active = tab;
            vec![
                EngineAction::Hide(previous),
                EngineAction::AdoptTab {
                    window,
                    tab,
                    pending: *pending,
                },
            ]
        }
        EngineEvent::TabClosed(tab) => close_tab(browser, *tab),
        EngineEvent::TabCrashed { tab, reason } => {
            with_tab(browser, *tab, |t| {
                t.crashed = Some(reason.clone());
                t.load = LoadState::Idle;
            });
            Vec::new()
        }
        EngineEvent::FrameReady(_) => Vec::new(),
        EngineEvent::CursorChanged { .. } => Vec::new(),
        EngineEvent::EngineError(message) => {
            browser.engine_error = Some(message.clone());
            Vec::new()
        }
    }
}

fn with_tab(browser: &mut Browser, tab: TabId, f: impl FnOnce(&mut Tab)) {
    if let Some(t) = browser.tab_mut(tab) {
        f(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::command::{Command, dispatch, open_window};
    use crate::model::WindowId;
    use crate::model::test_support::{browser, url};

    fn one_window() -> (Browser, WindowId, TabId) {
        let mut b = browser();
        let actions = open_window(&mut b, url("https://a.example/"));
        let EngineAction::OpenWindow { window, tab, .. } = actions[0].clone() else {
            panic!("open_window returns OpenWindow");
        };
        (b, window, tab)
    }

    #[test]
    fn url_changed_updates_tab_url() {
        let (mut b, _, tab) = one_window();
        let actions = apply(
            &mut b,
            &EngineEvent::UrlChanged {
                tab,
                url: url("https://b.example/"),
            },
        );
        assert!(actions.is_empty());
        assert_eq!(b.tab(tab).unwrap().url.as_str(), "https://b.example/");
    }

    #[test]
    fn title_changed_updates_title() {
        let (mut b, _, tab) = one_window();
        apply(
            &mut b,
            &EngineEvent::TitleChanged {
                tab,
                title: "Hello".to_string(),
            },
        );
        assert_eq!(b.tab(tab).unwrap().title, "Hello");
    }

    #[test]
    fn load_state_changed_updates_load() {
        let (mut b, _, tab) = one_window();
        apply(
            &mut b,
            &EngineEvent::LoadStateChanged {
                tab,
                state: LoadState::Complete,
            },
        );
        assert_eq!(b.tab(tab).unwrap().load, LoadState::Complete);
    }

    #[test]
    fn history_changed_updates_navigation_flags() {
        let (mut b, _, tab) = one_window();
        apply(
            &mut b,
            &EngineEvent::HistoryChanged {
                tab,
                can_go_back: true,
                can_go_forward: false,
            },
        );
        let t = b.tab(tab).unwrap();
        assert!(t.can_go_back);
        assert!(!t.can_go_forward);
    }

    #[test]
    fn status_changed_updates_status() {
        let (mut b, _, tab) = one_window();
        apply(
            &mut b,
            &EngineEvent::StatusChanged {
                tab,
                text: "https://link.example/".to_string(),
            },
        );
        assert_eq!(b.tab(tab).unwrap().status, "https://link.example/");
    }

    #[test]
    fn favicon_changed_stores_favicon() {
        let (mut b, _, tab) = one_window();
        let favicon = Favicon {
            width: 1,
            height: 1,
            rgba: Arc::from(vec![255, 0, 0, 255]),
        };
        apply(
            &mut b,
            &EngineEvent::FaviconChanged {
                tab,
                favicon: favicon.clone(),
            },
        );
        assert_eq!(b.tab(tab).unwrap().favicon, Some(favicon));
    }

    #[test]
    fn new_tab_requested_adds_active_tab_and_adopts_pending() {
        let (mut b, window, opener) = one_window();
        let actions = apply(
            &mut b,
            &EngineEvent::NewTabRequested {
                opener,
                pending: PendingTab(7),
            },
        );
        let w = b.window(window).unwrap();
        assert_eq!(w.tabs.len(), 2);
        let tab = w.tabs[1].id;
        assert_eq!(w.active, tab);
        assert_eq!(
            actions,
            vec![
                EngineAction::Hide(opener),
                EngineAction::AdoptTab {
                    window,
                    tab,
                    pending: PendingTab(7),
                },
            ]
        );
    }

    #[test]
    fn new_tab_requested_for_unknown_opener_does_nothing() {
        let (mut b, _, _) = one_window();
        let actions = apply(
            &mut b,
            &EngineEvent::NewTabRequested {
                opener: TabId(99),
                pending: PendingTab(1),
            },
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn tab_closed_behaves_like_close_tab_command() {
        let (mut b, window, t1) = one_window();
        dispatch(&mut b, &Command::NewTab { window, url: None });
        let t2 = b.window(window).unwrap().active;
        let actions = apply(&mut b, &EngineEvent::TabClosed(t2));
        assert_eq!(
            actions,
            vec![EngineAction::CloseTab(t2), EngineAction::Show(t1)]
        );
        assert_eq!(b.window(window).unwrap().tabs.len(), 1);
    }

    #[test]
    fn tab_crashed_keeps_tab_and_records_reason() {
        let (mut b, _, tab) = one_window();
        apply(
            &mut b,
            &EngineEvent::LoadStateChanged {
                tab,
                state: LoadState::Loading,
            },
        );
        apply(
            &mut b,
            &EngineEvent::TabCrashed {
                tab,
                reason: "script panicked".to_string(),
            },
        );
        let t = b.tab(tab).unwrap();
        assert_eq!(t.crashed.as_deref(), Some("script panicked"));
        assert_eq!(t.load, LoadState::Idle);
        assert_eq!(t.url.as_str(), "https://a.example/");
    }

    #[test]
    fn frame_ready_changes_nothing() {
        let (mut b, _, tab) = one_window();
        let before = b.windows.clone();
        assert!(apply(&mut b, &EngineEvent::FrameReady(tab)).is_empty());
        assert_eq!(b.windows, before);
    }

    #[test]
    fn engine_error_is_recorded() {
        let (mut b, _, _) = one_window();
        apply(
            &mut b,
            &EngineEvent::EngineError("lost backend".to_string()),
        );
        assert_eq!(b.engine_error.as_deref(), Some("lost backend"));
    }

    #[test]
    fn tab_closed_echo_after_close_tab_command_is_a_no_op() {
        let (mut b, window, t1) = one_window();
        dispatch(&mut b, &Command::NewTab { window, url: None });
        let t2 = b.window(window).unwrap().active;
        let closed = dispatch(&mut b, &Command::CloseTab(t2));
        assert_eq!(
            closed,
            vec![EngineAction::CloseTab(t2), EngineAction::Show(t1)]
        );
        let before = b.windows.clone();
        assert!(apply(&mut b, &EngineEvent::TabClosed(t2)).is_empty());
        assert_eq!(b.windows, before);
    }

    #[test]
    fn tab_closed_echo_after_closing_last_tab_is_a_no_op() {
        let (mut b, window, t1) = one_window();
        let closed = dispatch(&mut b, &Command::CloseTab(t1));
        assert_eq!(
            closed,
            vec![
                EngineAction::CloseTab(t1),
                EngineAction::CloseWindow(window),
                EngineAction::Quit,
            ]
        );
        let before = b.windows.clone();
        assert!(apply(&mut b, &EngineEvent::TabClosed(t1)).is_empty());
        assert_eq!(b.windows, before);
        assert!(b.windows.is_empty());
    }

    #[test]
    fn tab_closed_for_unknown_tab_is_a_no_op() {
        let (mut b, _, _) = one_window();
        let before = b.windows.clone();
        assert!(apply(&mut b, &EngineEvent::TabClosed(TabId(99))).is_empty());
        assert_eq!(b.windows, before);
    }

    #[test]
    fn events_for_unknown_tabs_are_ignored() {
        let (mut b, _, _) = one_window();
        let before = b.windows.clone();
        apply(
            &mut b,
            &EngineEvent::TitleChanged {
                tab: TabId(99),
                title: "x".to_string(),
            },
        );
        assert_eq!(b.windows, before);
    }
}
