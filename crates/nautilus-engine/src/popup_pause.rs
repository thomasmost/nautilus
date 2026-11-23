//! Every other tab stops painting while one tab's popup widget is open.
//!
//! Chromium paints a popup widget (a `<select>`'s list) through a video
//! capturer of its own, and with GPU compositing that capturer delivers
//! no frame while two other windowless views are being captured: two
//! windows, each showing a tab, and no list ever appears in either (C15
//! in `docs/upstream-issues.md`). So from the moment a popup widget is
//! announced until it is gone, the tabs that are painting, in every
//! window of every profile, are told they are hidden, which stops their
//! capturers; they keep their last frame on screen and come back when
//! the widget closes, through `show_view`, which is what brings back a
//! view whose surface went while it was hidden (C16).
//!
//! The extension bridge's hidden pages are windowless views too and
//! would count, but they are told they are hidden for as long as they
//! live (`bridge/hidden_page.rs`), so there is nothing of theirs to
//! pause here.
//!
//! The list of views is the process's (the browser process's UI thread),
//! not a profile's: the limit does not know profiles.

use std::cell::RefCell;
use std::os::raw::c_int;
use std::rc::{Rc, Weak};

use cef::{Browser, ImplBrowser, ImplBrowserHost};

use crate::delegate::{ViewState, show_view};

struct Tracked {
    id: c_int,
    view: Weak<ViewState>,
    browser: Browser,
}

#[derive(Default)]
struct State {
    views: Vec<Tracked>,
    /// The browser whose popup widget the others are paused for.
    owner: Option<c_int>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// What a view is doing when a popup widget opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Standing {
    id: c_int,
    /// Told it is hidden already, for a reason of its own.
    hidden: bool,
    /// Paused for an earlier popup widget.
    paused: bool,
}

/// The views to pause when `owner`'s popup widget opens, and whether
/// `owner` itself must be let go of an earlier pause: every other view
/// that is painting. A view already paused stays so.
fn plan_open(owner: c_int, views: &[Standing]) -> (Vec<c_int>, bool) {
    let pause = views
        .iter()
        .filter(|v| v.id != owner && !v.hidden && !v.paused)
        .map(|v| v.id)
        .collect();
    let release_owner = views.iter().any(|v| v.id == owner && v.paused);
    (pause, release_owner)
}

/// A tab's view has a browser: it is one of the views that paint.
pub(crate) fn track(view: &Rc<ViewState>, browser: &Browser) {
    let id = browser.identifier();
    let displaced: Vec<Tracked> = STATE.with_borrow_mut(|state| {
        let (gone, kept) = std::mem::take(&mut state.views)
            .into_iter()
            .partition(|t| t.id == id || t.view.strong_count() == 0);
        state.views = kept;
        state.views.push(Tracked {
            id,
            view: Rc::downgrade(view),
            browser: browser.clone(),
        });
        gone
    });
    // Browsers are let go of outside the borrow.
    drop(displaced);
}

/// The browser `id` is closing: its view paints no more, and a pause
/// held for its popup widget ends.
pub(crate) fn forget(id: c_int) {
    let gone: Vec<Tracked> = STATE.with_borrow_mut(|state| {
        let (gone, kept) = std::mem::take(&mut state.views)
            .into_iter()
            .partition(|t| t.id == id);
        state.views = kept;
        gone
    });
    drop(gone);
    closed(id);
}

/// The browser `id` announced a popup widget (`on_popup_show`, before
/// the widget's first frame): pause the others.
pub(crate) fn opened(id: c_int) {
    let (pause, release): (Vec<Browser>, Option<Browser>) = STATE.with_borrow_mut(|state| {
        let alive: Vec<(&Tracked, Rc<ViewState>)> = state
            .views
            .iter()
            .filter_map(|t| t.view.upgrade().map(|view| (t, view)))
            .collect();
        let standings: Vec<Standing> = alive
            .iter()
            .map(|(t, view)| Standing {
                id: t.id,
                hidden: view.wants_hidden() && !view.paused.get(),
                paused: view.paused.get(),
            })
            .collect();
        let (ids, release_owner) = plan_open(id, &standings);
        let mut pause = Vec::new();
        let mut release = None;
        for (t, view) in &alive {
            if ids.contains(&t.id) {
                view.paused.set(true);
                pause.push(t.browser.clone());
            } else if t.id == id && release_owner {
                view.paused.set(false);
                if !view.wants_hidden() {
                    release = Some(t.browser.clone());
                }
            }
        }
        state.owner = Some(id);
        (pause, release)
    });
    // Outside the borrow: hiding a view closes a popup widget of its
    // own, which comes back here through `closed`.
    if let Some(host) = release.and_then(|browser| browser.host()) {
        show_view(&host);
    }
    if !pause.is_empty() {
        log::debug!(
            "a popup widget opened: {} other view(s) paused",
            pause.len()
        );
    }
    for browser in pause {
        if let Some(host) = browser.host() {
            host.was_hidden(1);
        }
    }
}

/// The popup widget of the browser `id` is gone: the views paused for it
/// paint again. The end of any other browser's widget changes nothing
/// (a paused view's own widget closes because it was paused).
pub(crate) fn closed(id: c_int) {
    let resume: Vec<Browser> = STATE.with_borrow_mut(|state| {
        if state.owner != Some(id) {
            return Vec::new();
        }
        state.owner = None;
        state
            .views
            .iter()
            .filter_map(|t| {
                let view = t.view.upgrade()?;
                if !view.paused.replace(false) || view.wants_hidden() {
                    return None;
                }
                Some(t.browser.clone())
            })
            .collect()
    });
    for browser in resume {
        if let Some(host) = browser.host() {
            show_view(&host);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Standing, plan_open};

    fn view(id: i32, hidden: bool, paused: bool) -> Standing {
        Standing { id, hidden, paused }
    }

    #[test]
    fn every_other_painting_view_is_paused() {
        let views = [
            view(1, false, false),
            view(2, false, false),
            view(3, true, false),
            view(4, false, false),
        ];
        assert_eq!(plan_open(2, &views), (vec![1, 4], false));
    }

    #[test]
    fn a_lone_view_pauses_nothing() {
        assert_eq!(plan_open(1, &[view(1, false, false)]), (vec![], false));
        assert_eq!(plan_open(1, &[]), (vec![], false));
    }

    /// A widget that opens in a view paused for an earlier one takes the
    /// pause over: the views paused already stay so, the earlier owner
    /// joins them, and the new owner paints again.
    #[test]
    fn a_paused_view_that_opens_a_widget_takes_the_pause_over() {
        let views = [
            view(1, false, false),
            view(2, false, true),
            view(3, false, true),
        ];
        assert_eq!(plan_open(2, &views), (vec![1], true));
    }
}
