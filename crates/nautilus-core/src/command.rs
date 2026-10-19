//! Named user actions and the pure `dispatch` that applies them.

use url::Url;

use crate::event::PendingTab;
use crate::model::{Browser, LoadState, Tab, TabId, Window, WindowId};

/// Every user action in Nautilus is one of these. The chrome, the keymap,
/// and (in Phase 2) remappable hotkeys all produce `Command`s; nothing else
/// mutates the model from the UI side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    NewTab {
        window: WindowId,
        url: Option<Url>,
    },
    CloseTab(TabId),
    /// Make `tab` active and the only selected tab (a plain click).
    ActivateTab(TabId),
    /// Add `tab` to its window's selection or remove it (Ctrl-click). The
    /// active tab is always selected, so toggling it does nothing.
    ToggleTabSelection(TabId),
    /// Select every tab between the active tab and `tab`, inclusive, and
    /// nothing else (Shift-click). The active tab stays active.
    SelectTabRange(TabId),
    /// Close each of these tabs (the context menu's "Close tabs").
    CloseTabs(Vec<TabId>),
    /// Move these tabs to a new window, in strip order. A Chromium browser
    /// cannot leave the native window it was created for, so each
    /// tab is recreated at its URL in the new window and closed here: the
    /// page reloads and its history and form state are lost.
    MoveTabsToNewWindow(Vec<TabId>),
    /// Move these tabs to the end of `window`'s strip, the same way.
    MoveTabsToWindow {
        tabs: Vec<TabId>,
        window: WindowId,
    },
    /// Open these tabs' URLs in a `nautilus` process for `profile` (another
    /// profile is another process, as a Chromium cache directory belongs
    /// to one browser process at a time) and close
    /// them here.
    OpenTabsInProfile {
        tabs: Vec<TabId>,
        profile: String,
    },
    NextTab(WindowId),
    PreviousTab(WindowId),
    Navigate {
        tab: TabId,
        url: Url,
    },
    Back(TabId),
    Forward(TabId),
    Reload(TabId),
    Stop(TabId),
    FocusAddressBar(WindowId),
    NewWindow,
    CloseWindow(WindowId),
    OpenSettings,
    /// Clear the engine error shown in the status area.
    DismissEngineError,
    Quit,
}

/// What the application must do after [`dispatch`] (or [`crate::apply`])
/// has updated the model. Most variants are forwarded to the engine's tab
/// host; `OpenWindow`, `CloseWindow`, `FocusAddressBar`, `OpenSettings`, and
/// `Quit` are handled by the app itself. Returning these instead of calling
/// the engine directly keeps `dispatch` pure and testable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineAction {
    /// Create and show a new engine tab for `tab` in `window`, loading `url`.
    CreateTab {
        window: WindowId,
        tab: TabId,
        url: Url,
    },
    /// Adopt a tab the engine already created (a `window.open` request) as
    /// `tab` in `window`, and show it.
    AdoptTab {
        window: WindowId,
        tab: TabId,
        pending: PendingTab,
    },
    Navigate {
        tab: TabId,
        url: Url,
    },
    Back(TabId),
    Forward(TabId),
    Reload(TabId),
    Stop(TabId),
    Show(TabId),
    Hide(TabId),
    CloseTab(TabId),
    /// Open a native window for `window` whose first tab is `tab` at `url`.
    OpenWindow {
        window: WindowId,
        tab: TabId,
        url: Url,
    },
    CloseWindow(WindowId),
    FocusAddressBar(WindowId),
    OpenSettings,
    /// Start a `nautilus --profile <profile> <urls>` process.
    OpenInProfile {
        profile: String,
        urls: Vec<Url>,
    },
    Quit,
}

/// Apply a command to the model and return what the app must do next.
pub fn dispatch(browser: &mut Browser, cmd: &Command) -> Vec<EngineAction> {
    match cmd {
        Command::NewTab { window, url } => new_tab(browser, *window, url.clone()),
        Command::CloseTab(tab) => close_tab(browser, *tab),
        Command::ActivateTab(tab) => activate_tab(browser, *tab),
        Command::ToggleTabSelection(tab) => {
            toggle_tab_selection(browser, *tab);
            Vec::new()
        }
        Command::SelectTabRange(tab) => {
            select_tab_range(browser, *tab);
            Vec::new()
        }
        Command::CloseTabs(tabs) => close_tabs(browser, tabs),
        Command::MoveTabsToNewWindow(tabs) => move_tabs_to_new_window(browser, tabs),
        Command::MoveTabsToWindow { tabs, window } => move_tabs_to_window(browser, tabs, *window),
        Command::OpenTabsInProfile { tabs, profile } => {
            open_tabs_in_profile(browser, tabs, profile)
        }
        Command::NextTab(window) => step_tab(browser, *window, 1),
        Command::PreviousTab(window) => step_tab(browser, *window, -1),
        Command::Navigate { tab, url } => match browser.tab_mut(*tab) {
            Some(t) => {
                t.url = url.clone();
                t.load = LoadState::Loading;
                t.crashed = None;
                vec![EngineAction::Navigate {
                    tab: *tab,
                    url: url.clone(),
                }]
            }
            None => Vec::new(),
        },
        Command::Back(tab) => history_step(browser, *tab, EngineAction::Back(*tab)),
        Command::Forward(tab) => history_step(browser, *tab, EngineAction::Forward(*tab)),
        Command::Reload(tab) => match browser.tab_mut(*tab) {
            Some(t) => {
                t.load = LoadState::Loading;
                t.crashed = None;
                vec![EngineAction::Reload(*tab)]
            }
            None => Vec::new(),
        },
        Command::Stop(tab) => pass_through(browser, *tab, EngineAction::Stop(*tab)),
        Command::FocusAddressBar(window) => {
            if browser.window(*window).is_some() {
                vec![EngineAction::FocusAddressBar(*window)]
            } else {
                Vec::new()
            }
        }
        Command::NewWindow => {
            let url = browser.new_tab_url();
            open_window(browser, url)
        }
        Command::CloseWindow(window) => close_window(browser, *window),
        Command::OpenSettings => vec![EngineAction::OpenSettings],
        Command::DismissEngineError => {
            browser.engine_error = None;
            Vec::new()
        }
        Command::Quit => vec![EngineAction::Quit],
    }
}

/// Add a window to the model whose first tab shows `url`. Used by
/// `Command::NewWindow` and by `main` for the initial window, which opens
/// the homepage or a URL given on the command line.
pub fn open_window(browser: &mut Browser, url: Url) -> Vec<EngineAction> {
    let window = browser.allocate_window_id();
    let tab = browser.allocate_tab_id();
    browser
        .windows
        .push(Window::with_tab(window, Tab::new(tab, url.clone())));
    vec![EngineAction::OpenWindow { window, tab, url }]
}

fn pass_through(browser: &Browser, tab: TabId, action: EngineAction) -> Vec<EngineAction> {
    if browser.tab(tab).is_some() {
        vec![action]
    } else {
        Vec::new()
    }
}

/// Back and Forward ask for a new load of the tab's content, so like
/// `Reload` they clear a crashed tab's error panel.
fn history_step(browser: &mut Browser, tab: TabId, action: EngineAction) -> Vec<EngineAction> {
    match browser.tab_mut(tab) {
        Some(t) => {
            t.crashed = None;
            vec![action]
        }
        None => Vec::new(),
    }
}

fn new_tab(browser: &mut Browser, window: WindowId, url: Option<Url>) -> Vec<EngineAction> {
    if browser.window(window).is_none() {
        return Vec::new();
    }
    let focus_address = url.is_none();
    let url = url.unwrap_or_else(|| browser.new_tab_url());
    let tab = browser.allocate_tab_id();
    let w = browser
        .window_mut(window)
        .expect("window existence checked above");
    let previous = w.active;
    w.tabs.push(Tab::new(tab, url.clone()));
    w.active = tab;
    w.select_only(tab);
    let mut actions = vec![
        EngineAction::Hide(previous),
        EngineAction::CreateTab { window, tab, url },
    ];
    // A tab the user opened empty is there to type an address into; a tab
    // opened on a URL (a popup, a link into a new tab) keeps the focus on
    // the page.
    if focus_address {
        actions.push(EngineAction::FocusAddressBar(window));
    }
    actions
}

/// Remove `tab`. If it was active, the tab to its right becomes active, or
/// the one to its left if it was last. Closing the last tab closes the
/// window, and closing the last window quits.
pub(crate) fn close_tab(browser: &mut Browser, tab: TabId) -> Vec<EngineAction> {
    let Some(window_id) = browser.window_of_tab(tab) else {
        return Vec::new();
    };
    let mut actions = vec![EngineAction::CloseTab(tab)];
    let new_active = {
        let w = browser
            .window_mut(window_id)
            .expect("window_of_tab returned an existing window");
        let index = w.index_of(tab).expect("tab is in this window");
        w.tabs.remove(index);
        w.selected.retain(|t| *t != tab);
        if w.tabs.is_empty() {
            None
        } else if w.active == tab {
            let next = index.min(w.tabs.len() - 1);
            w.active = w.tabs[next].id;
            w.select_only(w.active);
            Some(w.active)
        } else {
            return actions;
        }
    };
    match new_active {
        Some(next) => actions.push(EngineAction::Show(next)),
        None => {
            browser.windows.retain(|w| w.id != window_id);
            actions.push(EngineAction::CloseWindow(window_id));
            if browser.windows.is_empty() {
                actions.push(EngineAction::Quit);
            }
        }
    }
    actions
}

fn activate_tab(browser: &mut Browser, tab: TabId) -> Vec<EngineAction> {
    let Some(window_id) = browser.window_of_tab(tab) else {
        return Vec::new();
    };
    let w = browser
        .window_mut(window_id)
        .expect("window_of_tab returned an existing window");
    if w.active == tab {
        w.select_only(tab);
        return Vec::new();
    }
    let previous = w.active;
    w.active = tab;
    w.select_only(tab);
    vec![EngineAction::Hide(previous), EngineAction::Show(tab)]
}

fn toggle_tab_selection(browser: &mut Browser, tab: TabId) {
    let Some(window_id) = browser.window_of_tab(tab) else {
        return;
    };
    let w = browser
        .window_mut(window_id)
        .expect("window_of_tab returned an existing window");
    if w.active == tab {
        return;
    }
    if w.is_selected(tab) {
        w.selected.retain(|t| *t != tab);
    } else {
        w.selected.push(tab);
    }
}

fn select_tab_range(browser: &mut Browser, tab: TabId) {
    let Some(window_id) = browser.window_of_tab(tab) else {
        return;
    };
    let w = browser
        .window_mut(window_id)
        .expect("window_of_tab returned an existing window");
    let (from, to) = {
        let a = w.active_index();
        let b = w.index_of(tab).expect("tab is in this window");
        (a.min(b), a.max(b))
    };
    w.selected = w.tabs[from..=to].iter().map(|t| t.id).collect();
}

fn close_tabs(browser: &mut Browser, tabs: &[TabId]) -> Vec<EngineAction> {
    let mut actions = Vec::new();
    for tab in tabs {
        actions.extend(close_tab(browser, *tab));
    }
    actions
}

/// The tabs of `tabs` that exist, in strip order (window by window), with
/// their URLs, and whether each is the active tab of its window.
fn moving_tabs(browser: &Browser, tabs: &[TabId]) -> Vec<(TabId, Url, bool)> {
    browser
        .windows
        .iter()
        .flat_map(|w| {
            w.tabs
                .iter()
                .filter(|t| tabs.contains(&t.id))
                .map(move |t| (t.id, t.url.clone(), w.active == t.id))
        })
        .collect()
}

fn move_tabs_to_new_window(browser: &mut Browser, tabs: &[TabId]) -> Vec<EngineAction> {
    let moving = moving_tabs(browser, tabs);
    let Some((_, first_url, _)) = moving.first() else {
        return Vec::new();
    };
    let mut actions = open_window(browser, first_url.clone());
    let window = browser
        .windows
        .last()
        .expect("open_window pushed a window")
        .id;
    actions.extend(place_moved_tabs(browser, window, &moving, 1));
    actions
}

fn move_tabs_to_window(
    browser: &mut Browser,
    tabs: &[TabId],
    window: WindowId,
) -> Vec<EngineAction> {
    if browser.window(window).is_none() {
        return Vec::new();
    }
    let moving: Vec<_> = moving_tabs(browser, tabs)
        .into_iter()
        .filter(|(tab, _, _)| browser.window_of_tab(*tab) != Some(window))
        .collect();
    if moving.is_empty() {
        return Vec::new();
    }
    place_moved_tabs(browser, window, &moving, 0)
}

/// Recreate `moving` (from index `already` on; the earlier ones are
/// already in `window`, at its end) as new tabs at the end of `window`,
/// activate the one standing in for the tab that was active in its old
/// window (or the first), then close the originals.
fn place_moved_tabs(
    browser: &mut Browser,
    window: WindowId,
    moving: &[(TabId, Url, bool)],
    already: usize,
) -> Vec<EngineAction> {
    let mut actions = Vec::new();
    let first_new = browser
        .window(window)
        .map(|w| w.tabs.len() - already)
        .unwrap_or(0);
    for (_, url, _) in &moving[already..] {
        actions.extend(new_tab(browser, window, Some(url.clone())));
    }
    let to_activate = {
        let w = browser.window(window).expect("window existence checked");
        let offset = moving
            .iter()
            .position(|(_, _, active)| *active)
            .unwrap_or(0);
        w.tabs[first_new + offset].id
    };
    actions.extend(activate_tab(browser, to_activate));
    let old: Vec<TabId> = moving.iter().map(|(tab, _, _)| *tab).collect();
    actions.extend(close_tabs(browser, &old));
    actions
}

fn open_tabs_in_profile(browser: &mut Browser, tabs: &[TabId], profile: &str) -> Vec<EngineAction> {
    if profile == browser.profile.name {
        return Vec::new();
    }
    let moving = moving_tabs(browser, tabs);
    if moving.is_empty() {
        return Vec::new();
    }
    let urls = moving.iter().map(|(_, url, _)| url.clone()).collect();
    let old: Vec<TabId> = moving.iter().map(|(tab, _, _)| *tab).collect();
    let mut actions = vec![EngineAction::OpenInProfile {
        profile: profile.to_string(),
        urls,
    }];
    actions.extend(close_tabs(browser, &old));
    actions
}

fn step_tab(browser: &mut Browser, window: WindowId, delta: isize) -> Vec<EngineAction> {
    let Some(w) = browser.window(window) else {
        return Vec::new();
    };
    let len = w.tabs.len() as isize;
    let next = (w.active_index() as isize + delta).rem_euclid(len) as usize;
    let target = w.tabs[next].id;
    activate_tab(browser, target)
}

fn close_window(browser: &mut Browser, window: WindowId) -> Vec<EngineAction> {
    let Some(position) = browser.windows.iter().position(|w| w.id == window) else {
        return Vec::new();
    };
    let removed = browser.windows.remove(position);
    let mut actions: Vec<EngineAction> = removed
        .tabs
        .iter()
        .map(|t| EngineAction::CloseTab(t.id))
        .collect();
    actions.push(EngineAction::CloseWindow(window));
    if browser.windows.is_empty() {
        actions.push(EngineAction::Quit);
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_support::{browser, url};

    /// A browser with one window holding one tab. Returns the ids.
    fn one_window() -> (Browser, WindowId, TabId) {
        let mut b = browser();
        let actions = open_window(&mut b, url("https://a.example/"));
        let EngineAction::OpenWindow { window, tab, .. } = actions[0].clone() else {
            panic!("open_window returns OpenWindow");
        };
        (b, window, tab)
    }

    #[test]
    fn open_window_creates_window_with_one_active_tab() {
        let (b, window, tab) = one_window();
        let w = b.window(window).unwrap();
        assert_eq!(w.tabs.len(), 1);
        assert_eq!(w.active, tab);
        assert_eq!(w.active_tab().url.as_str(), "https://a.example/");
    }

    #[test]
    fn new_tab_uses_new_tab_url_and_becomes_active() {
        let (mut b, window, first) = one_window();
        b.settings.new_tab_url = "https://new.example/".to_string();
        let actions = dispatch(&mut b, &Command::NewTab { window, url: None });
        let w = b.window(window).unwrap();
        assert_eq!(w.tabs.len(), 2);
        let second = w.tabs[1].id;
        assert_eq!(w.active, second);
        assert_eq!(
            actions,
            vec![
                EngineAction::Hide(first),
                EngineAction::CreateTab {
                    window,
                    tab: second,
                    url: url("https://new.example/"),
                },
                EngineAction::FocusAddressBar(window),
            ]
        );
    }

    #[test]
    fn new_tab_with_explicit_url_uses_it_and_leaves_focus_on_the_page() {
        let (mut b, window, first) = one_window();
        let actions = dispatch(
            &mut b,
            &Command::NewTab {
                window,
                url: Some(url("https://b.example/")),
            },
        );
        let w = b.window(window).unwrap();
        assert_eq!(w.active_tab().url.as_str(), "https://b.example/");
        assert_eq!(
            actions,
            vec![
                EngineAction::Hide(first),
                EngineAction::CreateTab {
                    window,
                    tab: w.active,
                    url: url("https://b.example/"),
                },
            ]
        );
    }

    #[test]
    fn new_tab_in_unknown_window_does_nothing() {
        let (mut b, _, _) = one_window();
        let actions = dispatch(
            &mut b,
            &Command::NewTab {
                window: WindowId(99),
                url: None,
            },
        );
        assert!(actions.is_empty());
        assert_eq!(b.windows.len(), 1);
    }

    #[test]
    fn activate_tab_hides_previous_and_shows_target() {
        let (mut b, window, first) = one_window();
        dispatch(&mut b, &Command::NewTab { window, url: None });
        let second = b.window(window).unwrap().active;
        let actions = dispatch(&mut b, &Command::ActivateTab(first));
        assert_eq!(b.window(window).unwrap().active, first);
        assert_eq!(
            actions,
            vec![EngineAction::Hide(second), EngineAction::Show(first)]
        );
        assert!(dispatch(&mut b, &Command::ActivateTab(first)).is_empty());
    }

    #[test]
    fn close_active_tab_activates_tab_to_the_right_then_left() {
        let (mut b, window, t1) = one_window();
        dispatch(&mut b, &Command::NewTab { window, url: None });
        let t2 = b.window(window).unwrap().active;
        dispatch(&mut b, &Command::NewTab { window, url: None });
        let t3 = b.window(window).unwrap().active;

        dispatch(&mut b, &Command::ActivateTab(t2));
        let actions = dispatch(&mut b, &Command::CloseTab(t2));
        assert_eq!(b.window(window).unwrap().active, t3, "right neighbour wins");
        assert_eq!(
            actions,
            vec![EngineAction::CloseTab(t2), EngineAction::Show(t3)]
        );

        let actions = dispatch(&mut b, &Command::CloseTab(t3));
        assert_eq!(
            b.window(window).unwrap().active,
            t1,
            "last tab falls back left"
        );
        assert_eq!(
            actions,
            vec![EngineAction::CloseTab(t3), EngineAction::Show(t1)]
        );
    }

    #[test]
    fn close_inactive_tab_keeps_active_and_emits_no_show() {
        let (mut b, window, t1) = one_window();
        dispatch(&mut b, &Command::NewTab { window, url: None });
        let t2 = b.window(window).unwrap().active;
        let actions = dispatch(&mut b, &Command::CloseTab(t1));
        assert_eq!(b.window(window).unwrap().active, t2);
        assert_eq!(actions, vec![EngineAction::CloseTab(t1)]);
    }

    #[test]
    fn closing_last_tab_closes_window_and_last_window_quits() {
        let (mut b, w1, t1) = one_window();
        let w2_actions = dispatch(&mut b, &Command::NewWindow);
        let EngineAction::OpenWindow {
            window: w2,
            tab: t2,
            ..
        } = w2_actions[0].clone()
        else {
            panic!("NewWindow returns OpenWindow");
        };

        let actions = dispatch(&mut b, &Command::CloseTab(t1));
        assert_eq!(
            actions,
            vec![EngineAction::CloseTab(t1), EngineAction::CloseWindow(w1)]
        );
        assert!(b.window(w1).is_none());

        let actions = dispatch(&mut b, &Command::CloseTab(t2));
        assert_eq!(
            actions,
            vec![
                EngineAction::CloseTab(t2),
                EngineAction::CloseWindow(w2),
                EngineAction::Quit,
            ]
        );
        assert!(b.windows.is_empty());
    }

    #[test]
    fn next_and_previous_tab_wrap_around() {
        let (mut b, window, t1) = one_window();
        dispatch(&mut b, &Command::NewTab { window, url: None });
        let t2 = b.window(window).unwrap().active;
        dispatch(&mut b, &Command::NewTab { window, url: None });
        let t3 = b.window(window).unwrap().active;

        dispatch(&mut b, &Command::NextTab(window));
        assert_eq!(
            b.window(window).unwrap().active,
            t1,
            "wraps from last to first"
        );
        dispatch(&mut b, &Command::PreviousTab(window));
        assert_eq!(
            b.window(window).unwrap().active,
            t3,
            "wraps from first to last"
        );
        dispatch(&mut b, &Command::PreviousTab(window));
        assert_eq!(b.window(window).unwrap().active, t2);
    }

    #[test]
    fn next_tab_with_single_tab_is_a_no_op() {
        let (mut b, window, t1) = one_window();
        assert!(dispatch(&mut b, &Command::NextTab(window)).is_empty());
        assert_eq!(b.window(window).unwrap().active, t1);
    }

    #[test]
    fn navigate_updates_url_marks_loading_and_clears_crash() {
        let (mut b, _, tab) = one_window();
        b.tab_mut(tab).unwrap().crashed = Some("boom".to_string());
        let actions = dispatch(
            &mut b,
            &Command::Navigate {
                tab,
                url: url("https://b.example/"),
            },
        );
        let t = b.tab(tab).unwrap();
        assert_eq!(t.url.as_str(), "https://b.example/");
        assert_eq!(t.load, LoadState::Loading);
        assert_eq!(t.crashed, None);
        assert_eq!(
            actions,
            vec![EngineAction::Navigate {
                tab,
                url: url("https://b.example/"),
            }]
        );
    }

    #[test]
    fn back_forward_reload_stop_pass_through_for_known_tabs_only() {
        let (mut b, _, tab) = one_window();
        assert_eq!(
            dispatch(&mut b, &Command::Back(tab)),
            vec![EngineAction::Back(tab)]
        );
        assert_eq!(
            dispatch(&mut b, &Command::Forward(tab)),
            vec![EngineAction::Forward(tab)]
        );
        assert_eq!(
            dispatch(&mut b, &Command::Reload(tab)),
            vec![EngineAction::Reload(tab)]
        );
        assert_eq!(b.tab(tab).unwrap().load, LoadState::Loading);
        assert_eq!(
            dispatch(&mut b, &Command::Stop(tab)),
            vec![EngineAction::Stop(tab)]
        );
        assert!(dispatch(&mut b, &Command::Back(TabId(99))).is_empty());
        assert!(dispatch(&mut b, &Command::Reload(TabId(99))).is_empty());
    }

    #[test]
    fn back_clears_a_crashed_tab() {
        let (mut b, _, tab) = one_window();
        b.tab_mut(tab).unwrap().crashed = Some("boom".to_string());
        assert_eq!(
            dispatch(&mut b, &Command::Back(tab)),
            vec![EngineAction::Back(tab)]
        );
        assert_eq!(b.tab(tab).unwrap().crashed, None);
    }

    #[test]
    fn forward_clears_a_crashed_tab() {
        let (mut b, _, tab) = one_window();
        b.tab_mut(tab).unwrap().crashed = Some("boom".to_string());
        assert_eq!(
            dispatch(&mut b, &Command::Forward(tab)),
            vec![EngineAction::Forward(tab)]
        );
        assert_eq!(b.tab(tab).unwrap().crashed, None);
    }

    #[test]
    fn new_window_opens_with_new_tab_url() {
        let (mut b, _, _) = one_window();
        b.settings.new_tab_url = "https://new.example/".to_string();
        let actions = dispatch(&mut b, &Command::NewWindow);
        assert_eq!(b.windows.len(), 2);
        let w = &b.windows[1];
        assert_eq!(
            actions,
            vec![EngineAction::OpenWindow {
                window: w.id,
                tab: w.active,
                url: url("https://new.example/"),
            }]
        );
    }

    #[test]
    fn close_window_closes_all_its_tabs_and_quits_when_last() {
        let (mut b, window, t1) = one_window();
        dispatch(&mut b, &Command::NewTab { window, url: None });
        let t2 = b.window(window).unwrap().active;
        let actions = dispatch(&mut b, &Command::CloseWindow(window));
        assert_eq!(
            actions,
            vec![
                EngineAction::CloseTab(t1),
                EngineAction::CloseTab(t2),
                EngineAction::CloseWindow(window),
                EngineAction::Quit,
            ]
        );
        assert!(b.windows.is_empty());
        assert!(dispatch(&mut b, &Command::CloseWindow(window)).is_empty());
    }

    #[test]
    fn focus_settings_and_quit_are_forwarded_to_the_app() {
        let (mut b, window, _) = one_window();
        assert_eq!(
            dispatch(&mut b, &Command::FocusAddressBar(window)),
            vec![EngineAction::FocusAddressBar(window)]
        );
        assert!(dispatch(&mut b, &Command::FocusAddressBar(WindowId(99))).is_empty());
        assert_eq!(
            dispatch(&mut b, &Command::OpenSettings),
            vec![EngineAction::OpenSettings]
        );
        assert_eq!(dispatch(&mut b, &Command::Quit), vec![EngineAction::Quit]);
    }

    #[test]
    fn tab_and_window_commands_for_unknown_ids_are_no_ops() {
        let (mut b, _, _) = one_window();
        let before = b.windows.clone();
        assert!(dispatch(&mut b, &Command::CloseTab(TabId(99))).is_empty());
        assert!(dispatch(&mut b, &Command::ActivateTab(TabId(99))).is_empty());
        assert!(dispatch(&mut b, &Command::NextTab(WindowId(99))).is_empty());
        assert!(dispatch(&mut b, &Command::PreviousTab(WindowId(99))).is_empty());
        assert_eq!(b.windows, before);
    }

    #[test]
    fn dismiss_engine_error_clears_it() {
        let (mut b, _, _) = one_window();
        b.engine_error = Some("lost backend".to_string());
        assert!(dispatch(&mut b, &Command::DismissEngineError).is_empty());
        assert_eq!(b.engine_error, None);
    }

    /// A window with three tabs; returns the browser, the window, and the
    /// tab ids in strip order. The third tab is active.
    fn three_tabs() -> (Browser, WindowId, [TabId; 3]) {
        let (mut b, window, first) = one_window();
        dispatch(
            &mut b,
            &Command::NewTab {
                window,
                url: Some(url("https://b.example/")),
            },
        );
        dispatch(
            &mut b,
            &Command::NewTab {
                window,
                url: Some(url("https://c.example/")),
            },
        );
        let w = b.window(window).unwrap();
        let ids = [first, w.tabs[1].id, w.tabs[2].id];
        assert_eq!(w.active, ids[2]);
        (b, window, ids)
    }

    #[test]
    fn a_new_or_activated_tab_is_the_only_selected_tab() {
        let (mut b, window, [a, bb, c]) = three_tabs();
        assert_eq!(b.window(window).unwrap().selection(), vec![c]);
        dispatch(&mut b, &Command::ToggleTabSelection(a));
        assert_eq!(b.window(window).unwrap().selection(), vec![a, c]);
        dispatch(&mut b, &Command::ActivateTab(bb));
        assert_eq!(b.window(window).unwrap().selection(), vec![bb]);
    }

    #[test]
    fn toggling_adds_and_removes_tabs_but_never_the_active_one() {
        let (mut b, window, [a, bb, c]) = three_tabs();
        assert!(dispatch(&mut b, &Command::ToggleTabSelection(a)).is_empty());
        dispatch(&mut b, &Command::ToggleTabSelection(bb));
        assert_eq!(b.window(window).unwrap().selection(), vec![a, bb, c]);
        dispatch(&mut b, &Command::ToggleTabSelection(a));
        assert_eq!(b.window(window).unwrap().selection(), vec![bb, c]);
        dispatch(&mut b, &Command::ToggleTabSelection(c));
        assert_eq!(b.window(window).unwrap().selection(), vec![bb, c]);
        assert_eq!(b.window(window).unwrap().active, c);
        assert!(dispatch(&mut b, &Command::ToggleTabSelection(TabId(99))).is_empty());
    }

    #[test]
    fn range_selection_runs_from_the_active_tab_in_either_direction() {
        let (mut b, window, [a, bb, c]) = three_tabs();
        dispatch(&mut b, &Command::SelectTabRange(a));
        assert_eq!(b.window(window).unwrap().selection(), vec![a, bb, c]);
        assert_eq!(b.window(window).unwrap().active, c);
        dispatch(&mut b, &Command::ActivateTab(a));
        dispatch(&mut b, &Command::SelectTabRange(bb));
        assert_eq!(b.window(window).unwrap().selection(), vec![a, bb]);
        dispatch(&mut b, &Command::SelectTabRange(a));
        assert_eq!(b.window(window).unwrap().selection(), vec![a]);
    }

    #[test]
    fn closing_a_tab_drops_it_from_the_selection() {
        let (mut b, window, [a, bb, c]) = three_tabs();
        dispatch(&mut b, &Command::SelectTabRange(a));
        dispatch(&mut b, &Command::CloseTab(bb));
        assert_eq!(b.window(window).unwrap().selection(), vec![a, c]);
        // Closing the active tab makes the neighbour active and the only
        // selected tab.
        dispatch(&mut b, &Command::CloseTab(c));
        assert_eq!(b.window(window).unwrap().selection(), vec![a]);
    }

    #[test]
    fn close_tabs_closes_each_and_the_window_when_none_remain() {
        let (mut b, window, [a, bb, c]) = three_tabs();
        let actions = dispatch(&mut b, &Command::CloseTabs(vec![a, c]));
        assert_eq!(b.window(window).unwrap().tabs.len(), 1);
        assert_eq!(b.window(window).unwrap().active, bb);
        assert!(actions.contains(&EngineAction::CloseTab(a)));
        assert!(actions.contains(&EngineAction::CloseTab(c)));
        let actions = dispatch(&mut b, &Command::CloseTabs(vec![bb, TabId(99)]));
        assert!(b.windows.is_empty());
        assert_eq!(
            actions,
            vec![
                EngineAction::CloseTab(bb),
                EngineAction::CloseWindow(window),
                EngineAction::Quit,
            ]
        );
    }

    #[test]
    fn moving_tabs_to_a_new_window_recreates_them_there_and_closes_the_originals() {
        let (mut b, window, [a, bb, c]) = three_tabs();
        let actions = dispatch(&mut b, &Command::MoveTabsToNewWindow(vec![c, a]));
        assert_eq!(b.windows.len(), 2);
        let new = &b.windows[1];
        let urls: Vec<&str> = new.tabs.iter().map(|t| t.url.as_str()).collect();
        // Strip order, not the order given.
        assert_eq!(urls, vec!["https://a.example/", "https://c.example/"]);
        // The tab that was active moves as the active tab.
        assert_eq!(new.active_tab().url.as_str(), "https://c.example/");
        assert_eq!(new.selection(), vec![new.active]);
        let old = b.window(window).unwrap();
        assert_eq!(old.tabs.iter().map(|t| t.id).collect::<Vec<_>>(), vec![bb]);
        assert_eq!(old.active, bb);
        // The new window opens before the old tabs close, so an old window
        // that empties out never looks like the last one.
        let open = actions
            .iter()
            .position(|x| matches!(x, EngineAction::OpenWindow { .. }))
            .unwrap();
        let close = actions
            .iter()
            .position(|x| *x == EngineAction::CloseTab(a))
            .unwrap();
        assert!(open < close);
        assert!(!actions.contains(&EngineAction::Quit));
    }

    #[test]
    fn moving_every_tab_to_a_new_window_closes_the_old_window_without_quitting() {
        let (mut b, window, [a, bb, c]) = three_tabs();
        let actions = dispatch(&mut b, &Command::MoveTabsToNewWindow(vec![a, bb, c]));
        assert_eq!(b.windows.len(), 1);
        assert_ne!(b.windows[0].id, window);
        assert!(actions.contains(&EngineAction::CloseWindow(window)));
        assert!(!actions.contains(&EngineAction::Quit));
    }

    #[test]
    fn moving_tabs_to_another_window_appends_them_and_activates_the_moved_active_tab() {
        let (mut b, window, [a, _bb, c]) = three_tabs();
        let other_actions = dispatch(&mut b, &Command::NewWindow);
        let EngineAction::OpenWindow { window: other, .. } = other_actions[0].clone() else {
            panic!("NewWindow opens a window");
        };
        let actions = dispatch(
            &mut b,
            &Command::MoveTabsToWindow {
                tabs: vec![a, c],
                window: other,
            },
        );
        let target = b.window(other).unwrap();
        let urls: Vec<&str> = target.tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(urls.len(), 3);
        assert_eq!(&urls[1..], &["https://a.example/", "https://c.example/"]);
        assert_eq!(target.active_tab().url.as_str(), "https://c.example/");
        assert_eq!(b.window(window).unwrap().tabs.len(), 1);
        assert!(actions.contains(&EngineAction::CloseTab(a)));
        assert!(actions.contains(&EngineAction::CloseTab(c)));
        // Tabs already in the target, unknown tabs, and unknown windows are ignored.
        let before = b.windows.clone();
        let moved = target.tabs[1].id;
        assert!(
            dispatch(
                &mut b,
                &Command::MoveTabsToWindow {
                    tabs: vec![moved, TabId(99)],
                    window: other,
                },
            )
            .is_empty()
        );
        assert!(
            dispatch(
                &mut b,
                &Command::MoveTabsToWindow {
                    tabs: vec![a],
                    window: WindowId(99),
                },
            )
            .is_empty()
        );
        assert_eq!(b.windows, before);
    }

    #[test]
    fn opening_tabs_in_another_profile_hands_over_the_urls_and_closes_them() {
        let (mut b, window, [a, bb, c]) = three_tabs();
        let actions = dispatch(
            &mut b,
            &Command::OpenTabsInProfile {
                tabs: vec![c, a],
                profile: "work".to_string(),
            },
        );
        assert_eq!(
            actions[0],
            EngineAction::OpenInProfile {
                profile: "work".to_string(),
                urls: vec![url("https://a.example/"), url("https://c.example/")],
            }
        );
        assert!(actions.contains(&EngineAction::CloseTab(a)));
        assert!(actions.contains(&EngineAction::CloseTab(c)));
        assert_eq!(
            b.window(window)
                .unwrap()
                .tabs
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            vec![bb]
        );
        // The current profile is not another process: nothing happens.
        let before = b.windows.clone();
        let own_profile = b.profile.name.clone();
        assert!(
            dispatch(
                &mut b,
                &Command::OpenTabsInProfile {
                    tabs: vec![bb],
                    profile: own_profile,
                },
            )
            .is_empty()
        );
        assert_eq!(b.windows, before);
    }
}
