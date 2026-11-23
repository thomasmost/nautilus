//! The CEF side of the bridge: the `Bridge`, which owns the hidden pages
//! the engine evaluates scripts in (`hidden_page`: each bridged
//! extension's relay page, and the helper extension's page).
//!
//! The engine reaches a hidden page the way `ExtensionManager` reaches
//! `chrome://extensions`: `Runtime.evaluate` through DevTools
//! (`devtools::evaluate`), the answer in `on_dev_tools_method_result`. No
//! render-process code is involved.
//!
//! The decisions are in `policy` and `debugger` (pure); this file carries
//! them out.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use cef::rc::Rc as _;
use cef::{
    Browser, DevToolsMessageObserver, ImplBrowser, ImplBrowserHost, ImplDevToolsMessageObserver,
    Registration, WrapDevToolsMessageObserver, wrap_dev_tools_message_observer,
};
use osmena_core::TabId;
use serde_json::{Value, json};

use super::call_ids::CallIds;
use super::debugger::{Change, Debuggers};
use super::evaluation;
use super::hidden_page::HiddenPage;
use super::hub::{CAPTURE_IDS, Lane};
use super::install::HELPER_ID;
use super::policy::{
    AfterAsk, Answered, Evidence, Identity, JobKind, JobTurn, MAX_OPENINGS, Next, Opening,
    Overflow, REFRESH_INTERVAL, Session, WorkerTarget, WorkerWatch, after_ask, after_delivery,
    asks_on, fails_on_loss, forgets, identity, job_kept, job_turn, next_opening, overflow,
    relay_changes, reply_due, session_evidence, tabs_to_forget,
};
use super::table::{
    Debuggee, Hello, Report, TabTable, Target, debuggee_of, hello_of, parse_targets, target_id_of,
    worker_target_of,
};
use super::transport::{Asker, BridgeCall, BridgeTransport, CallId};
use crate::delegate::{DelegateState, ViewState};

/// The key of the helper's page among the hidden pages; a relay's key
/// is its extension's id, which this can never be.
const HELPER_KEY: &str = "helper";

/// The path of an extension's relay page, after its extension's id
/// (`chrome-extension://<id><RELAY_PATH>`). It is `BRIDGE_DIR`'s relay.
pub(crate) const RELAY_PATH: &str = "/nautilus-bridge/relay.html";

/// The DevTools message id of a tab's probe (`Bridge::track`). The probe is
/// the first DevTools call on its tab's browser, and CEF keeps a caller's id
/// that is not below the browser's next one (`Inner::next_message`), and a
/// browser counts from 1. The same browser later carries the hub's
/// screenshots, whose ids are above `CAPTURE_IDS`, so the probe's must stay
/// below it.
const TAB_PROBE_ID: i32 = 1;
const _: () = assert!(TAB_PROBE_ID < CAPTURE_IDS);
/// A tab's probe whose call was not sent: no result carries this id.
const NOT_SENT: i32 = 0;

/// Waiting jobs beyond this are failed, oldest first, so a page that
/// never comes back cannot grow the queue without bound.
const MAX_JOBS: usize = 256;

/// The script an evaluation of a hidden page checks first: a page whose
/// script did not load answers `false` (or fails), and is closed.
const PROBE: &str = "typeof nautilusRelay === 'object' || typeof nautilusHelper === 'object'";

/// What a pending evaluation was for.
enum Pending {
    /// The helper's list of targets.
    Targets,
    /// A page's first evaluation after it loaded: whether its script is there.
    Probe,
    /// A relay's `identify()`: which worker instance it reaches.
    Identify { extension: String },
    /// The ask of a changed worker target (`check_worker_targets`): the relay's
    /// `identify()`, whose answer decides its relay for change number `change`.
    Ask { extension: String, change: u32 },
    /// A test control's expression in a relay page, whose value is logged
    /// as `label` (`inspect_relay`).
    #[cfg(feature = "test-hooks")]
    Inspect { label: String },
    /// A relay's `next()`: the worker's requests, when it has some.
    Poll,
    /// An answer handed to a relay; only a failure is of interest.
    Reply,
    /// A delivery handed to an extension's relay, kept to send it again.
    Deliver(Delivery),
}

/// What a delivery carries to an extension's worker.
enum Payload {
    /// A click on the extension's action for `tab`.
    Click { tab: TabId },
    /// A listener's event, or `bridge.wake`. `instance` is the worker
    /// instance the extension was known to have when the event was queued
    /// (`None`: before any hello).
    Event {
        event: String,
        args: Vec<Value>,
        instance: Option<String>,
    },
}

/// One delivery to an extension's worker. `id` is what the worker sees,
/// the same on every attempt, so a worker instance that already ran it
/// ignores the repeat. `attempt` counts from 0.
struct Delivery {
    id: u64,
    attempt: u8,
    payload: Payload,
}

impl Delivery {
    /// How the log names it: `delivery 7` for a click, `event tabs.onCreated
    /// (delivery 7)` for an event.
    fn label(&self) -> String {
        match &self.payload {
            Payload::Click { .. } => format!("delivery {}", self.id),
            Payload::Event { event, .. } => format!("event {event} (delivery {})", self.id),
        }
    }
}

#[cfg(test)]
mod delivery_tests {
    use osmena_core::TabId;

    use super::{Delivery, Job, Payload};
    use crate::bridge::policy::JobKind;

    fn click() -> Delivery {
        Delivery {
            id: 7,
            attempt: 0,
            payload: Payload::Click { tab: TabId(3) },
        }
    }

    fn event() -> Delivery {
        Delivery {
            id: 8,
            attempt: 1,
            payload: Payload::Event {
                event: "tabs.onCreated".to_string(),
                args: Vec::new(),
                instance: None,
            },
        }
    }

    // The scripted runs read these words in the log (`ci/side-panel`).
    #[test]
    fn a_click_is_logged_by_its_delivery_and_an_event_by_its_name_too() {
        assert_eq!(click().label(), "delivery 7");
        assert_eq!(event().label(), "event tabs.onCreated (delivery 8)");
    }

    #[test]
    fn the_queue_reads_a_deliverys_kind_from_what_it_carries() {
        let job = |delivery| Job::Deliver {
            extension: "abcdefghijklmnopabcdefghijklmnop".to_string(),
            delivery,
        };
        assert_eq!(job(click()).kind(), JobKind::Click);
        assert_eq!(job(event()).kind(), JobKind::Emit);
    }
}

/// Why a delivery was not queued (`Bridge::deliver`).
enum Undeliverable {
    /// The extension's relay, or the helper's page, was given up.
    GivenUp,
    /// The extension is not bridged and on.
    NotWanted,
}

/// Work that waits for a fresh table, or for its relay to be ready.
enum Job {
    /// Hand `delivery` to `extension`'s worker: a click on its action, or
    /// an event.
    Deliver {
        extension: String,
        delivery: Delivery,
    },
    /// `extension`'s worker attached (or detached) its debugger to
    /// `debuggee`; the worker waits for `call`, made by `instance`.
    Debugger {
        extension: String,
        call: u64,
        instance: String,
        debuggee: Debuggee,
        attach: bool,
    },
}

impl Job {
    fn extension(&self) -> &str {
        match self {
            Job::Deliver { extension, .. } | Job::Debugger { extension, .. } => extension,
        }
    }

    /// What the queue's rules read (`policy`).
    fn kind(&self) -> JobKind {
        match self {
            Job::Deliver { delivery, .. } => match delivery.payload {
                Payload::Click { .. } => JobKind::Click,
                Payload::Event { .. } => JobKind::Emit,
            },
            Job::Debugger { .. } => JobKind::Debugger,
        }
    }
}

/// How a hidden page has fared in its openings (`policy::next_opening`):
/// the openings since its last successful probe (`failed`), the openings whose
/// relay passed its probe and then could not reach its worker (`unreached`,
/// cleared by a worker's hello), when it was last opened, and whether the
/// engine has given up on it. The schedule counts both.
#[derive(Default)]
struct Opener {
    failed: u32,
    unreached: u32,
    last: Option<std::time::Instant>,
    given_up: bool,
}

/// What the engine keeps for a tab it tracks.
struct TabRecord {
    browser: Browser,
    view: Rc<ViewState>,
    /// The DevTools observer that reads the tab's target id.
    _probe: Option<Registration>,
}

#[derive(Default)]
struct Inner {
    table: TabTable,
    /// Each tracked tab's browser and view, until the tab closes.
    tabs: HashMap<TabId, TabRecord>,
    /// Which extensions are debugging which tab (`bridge::debugger`).
    debuggers: Debuggers,
    /// The hidden pages, by key: the relay page of each bridged, enabled
    /// extension under its id, and the helper's page under `HELPER_KEY`.
    pages: HashMap<String, HiddenPage>,
    /// The extensions the last `sync` wanted a relay for.
    wanted: Vec<String>,
    /// Whether the helper extension is loaded, as the last `sync` said.
    helper_loaded: bool,
    /// Each hidden page's openings (`Opener`), by key.
    openers: HashMap<String, Opener>,
    /// The worker instance each extension last reported (`identity`).
    identities: HashMap<String, String>,
    /// Each extension's worker target as the table refreshes saw it, and the
    /// change waiting for its relay's hello (`policy::WorkerWatch`).
    watches: HashMap<String, WorkerWatch>,
    /// The sessions a hello reported for tabs the table could not yet name, by
    /// extension; applied by the next join that names them (`apply_pending`).
    pending_sessions: HashMap<String, Vec<(i64, Session)>>,
    /// When the table was last refreshed for the relays' sake (`refresh_due`).
    last_refresh: Option<std::time::Instant>,
    /// Evaluations sent and not yet answered, by page key and the
    /// DevTools message id of that page's browser.
    pending: HashMap<(String, i32), Pending>,
    /// The message id of the next evaluation in any hidden page. CEF
    /// keeps a caller's id when it is not below the next id of the page's
    /// browser (`CefDevToolsController::ExecuteDevToolsMethod`: each browser
    /// counts from 1 and moves past every id it is given), and nothing else
    /// sends on a hidden page's browser, so one increasing counter for all
    /// the pages is always kept.
    next_message: i32,
    /// The helper has been asked for the targets and has not answered.
    refreshing: bool,
    /// A tab's target id was learned after the last refresh: the next turn
    /// refreshes (`reopen`), so no timer is added for it.
    refresh_wanted: bool,
    /// The worker calls the hub is answering, by the Bridge-local id.
    calls: CallIds,
    jobs: Vec<Job>,
    next_delivery: u64,
    next_generation: u64,
}

/// The engine's end of the bridge. One per engine, owned by the
/// extension manager.
pub(crate) struct Bridge {
    pub(super) delegate: Rc<DelegateState>,
    inner: RefCell<Inner>,
}

/// Whether the page under `key` is the one made under `generation`: a
/// callback from a page that is gone, or from the predecessor of the page
/// now under its key, is not for the current one.
fn is_current(inner: &Inner, key: &str, generation: u64) -> bool {
    page_ref(inner, key).is_some_and(|page| page.generation == generation)
}

fn page_ref<'a>(inner: &'a Inner, key: &str) -> Option<&'a HiddenPage> {
    inner.pages.get(key)
}

/// Remove the page under `key` if it has `generation`.
fn take_page(inner: &mut Inner, key: &str, generation: u64) -> Option<HiddenPage> {
    match inner.pages.get(key) {
        Some(page) if page.generation == generation => inner.pages.remove(key),
        _ => None,
    }
}

/// The extensions whose relay page is open.
fn open_relays(inner: &Inner) -> Vec<String> {
    inner
        .pages
        .keys()
        .filter(|key| key.as_str() != HELPER_KEY)
        .cloned()
        .collect()
}

/// The browser and view of a tracked tab.
type TabHandles = (Browser, Rc<ViewState>);

/// The browser and view of a tracked tab, cloned out so that CEF can be
/// called once the bridge's state is no longer borrowed.
fn tab_handles(inner: &Inner, tab: TabId) -> Option<TabHandles> {
    inner
        .tabs
        .get(&tab)
        .map(|record| (record.browser.clone(), record.view.clone()))
}

/// Set whether an extension debugs the tab, and show or hide its
/// renderer when it is behind another tab: hidden, it is painted only
/// while a debugger is on it. True when the tab is behind another tab, so
/// its visibility was set. Call without the bridge's state borrowed.
fn set_tab_debugged(browser: &Browser, view: &ViewState, debugged: bool) -> bool {
    view.debugged.set(debugged);
    if !view.hidden.get() {
        return false;
    }
    if let Some(host) = browser.host() {
        if view.wants_hidden() {
            host.was_hidden(1);
        } else {
            crate::delegate::show_view(&host);
        }
    }
    true
}

impl Bridge {
    pub(crate) fn new(delegate: Rc<DelegateState>) -> Rc<Bridge> {
        Rc::new(Bridge {
            delegate,
            inner: RefCell::new(Inner {
                next_delivery: 1,
                next_generation: 1,
                next_message: 1,
                ..Inner::default()
            }),
        })
    }

    /// A tab's browser exists: ask it for its DevTools target id, which the
    /// table joins with the helper's list of targets. Nothing else about
    /// the tab is read from DevTools: whether it is hidden follows the
    /// extensions' debuggers (`debugger`), not any session of this probe.
    pub(crate) fn track(self: &Rc<Self>, tab: TabId, browser: &Browser, view: Rc<ViewState>) {
        let Some(host) = browser.host() else {
            return;
        };
        // The id is the probe's own and the observer knows it before the
        // call: CEF can answer inside it.
        let sent = Rc::new(Cell::new(TAB_PROBE_ID));
        let mut observer = TabProbe::new(self.clone(), tab, sent.clone());
        let registration = host.add_dev_tools_message_observer(Some(&mut observer));
        let id =
            host.execute_dev_tools_method(TAB_PROBE_ID, Some(&"Target.getTargetInfo".into()), None);
        if id == 0 {
            sent.set(NOT_SENT);
            log::warn!("bridge: cannot ask {tab:?} for its target id");
        } else if id != TAB_PROBE_ID {
            // Not expected: the probe is the first call on its browser.
            sent.set(id);
            log::warn!(
                "bridge: CEF replaced message id {TAB_PROBE_ID} with {id} for {tab:?}'s probe"
            );
        }
        self.inner.borrow_mut().tabs.insert(
            tab,
            TabRecord {
                browser: browser.clone(),
                view,
                _probe: registration,
            },
        );
    }

    fn target_known(&self, tab: TabId, success: bool, result: &[u8]) {
        let parsed: serde_json::Value = serde_json::from_slice(result).unwrap_or_default();
        match target_id_of(&parsed).filter(|_| success) {
            Some(target) => {
                log::debug!("bridge: {tab:?} is DevTools target {target}");
                // This runs inside a CEF observer, so no evaluation happens
                // here: the next turn refreshes the table (`reopen`).
                let mut inner = self.inner.borrow_mut();
                inner.table.set_target(tab, target);
                inner.refresh_wanted = true;
            }
            None => log::warn!("bridge: {tab:?} gave no target id: {parsed}"),
        }
    }

    /// A tab closed.
    pub(crate) fn untrack(&self, tab: TabId) {
        // The record (its browser and probe) is dropped after the borrow.
        let record = {
            let mut inner = self.inner.borrow_mut();
            let record = inner.tabs.remove(&tab);
            inner.table.forget(tab);
            inner.debuggers.forget_tab(tab);
            record
        };
        drop(record);
    }

    /// Any tab the table could name, for a test click when no tab has
    /// had the focus yet.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn any_tab(&self) -> Option<TabId> {
        self.inner.borrow().tabs.keys().min().copied()
    }

    /// The extensions were listed: record what is wanted and close what is
    /// gone. A callback (the extension list's answer) is no place to create a
    /// browser, so what is missing is opened by `reopen`, on the engine's turn.
    pub(crate) fn sync(self: &Rc<Self>, helper_loaded: bool, bridged: &[String]) {
        let (close_helper, to_close) = {
            let mut inner = self.inner.borrow_mut();
            let previous = std::mem::replace(&mut inner.wanted, bridged.to_vec());
            // An extension that came on, went off, or was reloaded (off and on
            // in two lists) starts its openings over; one that stays as it was
            // keeps what it has (a given-up relay stays given up).
            for id in previous.iter().chain(bridged.iter()) {
                if previous.contains(id) != bridged.contains(id) {
                    inner.openers.remove(id);
                }
            }
            if previous != bridged || inner.helper_loaded != helper_loaded {
                inner.openers.remove(HELPER_KEY);
            }
            inner.helper_loaded = helper_loaded;
            let open = open_relays(&inner);
            let open: Vec<&str> = open.iter().map(String::as_str).collect();
            let (_, mut to_close) = relay_changes(&open, bridged);
            // Every extension that is off or gone, open relay or not, is no
            // longer followed: its debuggers, recorded worker and pending
            // sessions go with it (positive evidence, `ExtensionGone`).
            for id in previous.iter().filter(|id| !bridged.contains(id)) {
                if !to_close.contains(id) {
                    to_close.push(id.clone());
                }
            }
            for id in &to_close {
                inner.watches.remove(id);
                inner.pending_sessions.remove(id);
            }
            let close_helper = bridged.is_empty() && inner.pages.contains_key(HELPER_KEY);
            (close_helper, to_close)
        };
        if close_helper {
            log::info!("bridge: no bridged extension is on; the helper's page closes");
            self.close_relay(HELPER_KEY);
        }
        for id in to_close {
            self.close_relay(&id);
            self.forget_debuggers(&id, Evidence::ExtensionGone);
            let stale = self.take_jobs_for(&id);
            self.fail_jobs(stale, "its extension is off or gone");
        }
    }

    /// Open what is wanted and not open: the helper's page once the helper
    /// is loaded, and each bridged, enabled extension's relay. A relay that
    /// closed underneath (its extension reloaded, its renderer died) is
    /// opened again here. The engine calls this on every turn (`spin`),
    /// never from a CEF callback, so a dead relay returns within a turn or
    /// two whatever CEF was doing when it died.
    pub(crate) fn reopen(self: &Rc<Self>) {
        let now = std::time::Instant::now();
        let asked = std::mem::take(&mut self.inner.borrow_mut().refresh_wanted);
        if asked || self.refresh_due(now) {
            self.refresh();
        }
        let (helper_loaded, wanted, helper_open, open) = {
            let inner = self.inner.borrow();
            (
                inner.helper_loaded,
                inner.wanted.clone(),
                inner.pages.contains_key(HELPER_KEY),
                open_relays(&inner),
            )
        };
        if wanted.is_empty() {
            return;
        }
        let (helper_wanted, relays_wanted) = {
            let open: Vec<&str> = open.iter().map(String::as_str).collect();
            (
                helper_loaded && !helper_open,
                relay_changes(&open, &wanted).0,
            )
        };
        let helper = helper_wanted.then(|| HELPER_KEY.to_string());
        for key in helper.into_iter().chain(relays_wanted) {
            if self.opening_due(&key, now) {
                self.open_page(&key);
            }
        }
    }

    /// Whether a table refresh is due for the relays' sake (`REFRESH_INTERVAL`)
    /// while any relay is open. A reloaded worker leaves its relay page alive
    /// with no event (measured: `run.sh reload`), and the only thing that shows
    /// it is the target list, which the refresh reads (`check_worker_targets`).
    /// So a reload is seen within a bounded time, tab activity or not.
    fn refresh_due(&self, now: std::time::Instant) -> bool {
        let mut inner = self.inner.borrow_mut();
        if open_relays(&inner).is_empty() {
            return false;
        }
        let since = inner
            .last_refresh
            .map(|at| now.saturating_duration_since(at));
        // Due when never made, or when the interval has passed.
        if since.is_none_or(|elapsed| elapsed >= REFRESH_INTERVAL) {
            inner.last_refresh = Some(now);
            true
        } else {
            false
        }
    }

    /// Compare each wanted extension's worker target with the one recorded for
    /// it (`WorkerWatch::observe`). A change means the worker was reloaded or
    /// started anew, and the relay that reaches the old one is asked which worker
    /// instance it reaches now (`ask`): a live relay answers with that instance's
    /// hello and is kept, and a zombie cannot answer and is closed
    /// (`decide_ask`). The decision waits for the answer itself; no hello that
    /// came before the change decides anything, and no refresh count is waited on.
    /// Debuggers are kept in both cases; the relay's hello decides them
    /// (`worker_hello`).
    fn check_worker_targets(&self, targets: &[Target]) {
        let mut asks = Vec::new();
        {
            let mut inner = self.inner.borrow_mut();
            for extension in inner.wanted.clone() {
                let listed = worker_target_of(targets, &extension);
                let relay_open = inner.pages.contains_key(&extension);
                let watch = inner.watches.entry(extension.clone()).or_default();
                let recorded = watch.recorded().map(str::to_string);
                let (target, change) = watch.observe(listed.as_deref());
                match target {
                    WorkerTarget::First => {
                        if let Some(listed) = &listed {
                            log::info!("bridge: {extension}'s worker is target {listed}");
                        }
                    }
                    WorkerTarget::Changed => {
                        log::info!(
                            "bridge: {extension}'s worker target changed from {} to {}: the worker was reloaded or started anew",
                            recorded.unwrap_or_default(),
                            listed.unwrap_or_default()
                        );
                        match change {
                            Some(change) if asks_on(target, relay_open) => {
                                asks.push((extension.clone(), change));
                            }
                            _ => log::info!(
                                "bridge: {extension} has no relay open to ask about its new worker; a relay opened later says who its worker is when a debugger needs it"
                            ),
                        }
                    }
                    WorkerTarget::Same | WorkerTarget::Absent => {}
                }
            }
        }
        for (extension, change) in asks {
            self.ask(&extension, change);
        }
    }

    /// Ask `extension`'s relay which worker instance it reaches, for the change
    /// numbered `change` (`WorkerWatch::observe`). Only a change asks, so the
    /// ask wakes nothing that an idle stop had put to sleep. The answer
    /// comes to `Pending::Ask`.
    fn ask(&self, extension: &str, change: u32) {
        log::info!(
            "bridge: {extension}: its worker target changed (change {change}): asks its relay which worker it reaches"
        );
        if !self.evaluate(
            extension,
            "nautilusRelay.identify()",
            Pending::Ask {
                extension: extension.to_string(),
                change,
            },
        ) {
            self.decide_ask(extension, change, Answered::Unanswered);
        }
    }

    /// What the answer to change `change`'s ask does to `extension`'s relay
    /// (`policy::after_ask`). A failed answer closes the relay only for the
    /// latest change; its debuggers are kept, and a fresh relay's hello decides
    /// them. Nothing in flight on a kept relay is touched.
    fn decide_ask(&self, extension: &str, change: u32, answer: Answered) {
        let latest = self
            .inner
            .borrow()
            .watches
            .get(extension)
            .is_some_and(|watch| watch.is_latest(change));
        match after_ask(answer, latest) {
            AfterAsk::Keep => log::info!(
                "bridge: {extension}'s relay reaches a live worker (its answer to change {change}): the relay is kept"
            ),
            AfterAsk::Ignore => log::info!(
                "bridge: {extension}: the answer to change {change} is older than its worker's latest change; the latest change decides"
            ),
            AfterAsk::Close => {
                log::warn!(
                    "bridge: {extension}'s relay could not say which worker it reaches (change {change}): it is a zombie, so the relay is closed and opened again; its debuggers are kept"
                );
                self.close_relay(extension);
            }
        }
    }

    /// Whether the hidden page under `key` may be opened now. Each opening is
    /// counted (`Opener::failed`) and cleared again by a successful probe
    /// (`page_result`). When the schedule says give up, the page is given up
    /// once, with its consequences (`give_up`), and nothing opens it again
    /// until its extension changes (`sync`).
    fn opening_due(self: &Rc<Self>, key: &str, now: std::time::Instant) -> bool {
        let turn = {
            let mut inner = self.inner.borrow_mut();
            let opener = inner.openers.entry(key.to_string()).or_default();
            if opener.given_up {
                return false;
            }
            let since = opener.last.map(|at| now.saturating_duration_since(at));
            // A relay that passed its probe and then could not reach its worker
            // is a failed opening too: it counts until its worker says who
            // it is.
            let turn = next_opening(opener.failed + opener.unreached, since);
            match turn {
                Opening::Now => {
                    opener.failed += 1;
                    opener.last = Some(now);
                }
                Opening::GiveUp => opener.given_up = true,
                Opening::Wait(_) => {}
            }
            turn
        };
        match turn {
            Opening::Now => true,
            Opening::Wait(_) => false,
            Opening::GiveUp => {
                self.give_up(key);
                false
            }
        }
    }

    /// The page under `key` will not be opened again: it failed every opening
    /// since its extension last changed. Its queued work is failed, and the log
    /// says so once. Its debuggers are kept: a relay that cannot be opened says
    /// nothing about its worker's sessions, which may still be live, and a
    /// hidden tab that is debugged must stay shown (`policy::forgets`). They go
    /// by the extension's list, a tab's close, or a worker's hello. A relay's
    /// clicks are refused from now on (`click`); the helper's stop every tab list.
    fn give_up(self: &Rc<Self>, key: &str) {
        if key == HELPER_KEY {
            log::warn!(
                "bridge: the helper's page did not load in {MAX_OPENINGS} openings; giving up until the extensions change. Tab lists and clicks now fail."
            );
            let jobs = std::mem::take(&mut self.inner.borrow_mut().jobs);
            self.fail_jobs(jobs, "the helper's page did not load");
        } else {
            log::warn!(
                "bridge: {key}'s relay did not load in {MAX_OPENINGS} openings; giving up until its extension changes. Its clicks are refused; its debuggers are kept."
            );
            let stale = self.take_jobs_for(key);
            self.fail_jobs(stale, "its relay did not load");
        }
    }

    /// Whether the page under `key` has been given up.
    fn given_up(&self, key: &str) -> bool {
        self.inner
            .borrow()
            .openers
            .get(key)
            .is_some_and(|opener| opener.given_up)
    }

    /// The queued jobs of `extension`, taken out of the queue.
    fn take_jobs_for(&self, extension: &str) -> Vec<Job> {
        let mut inner = self.inner.borrow_mut();
        let (stale, kept): (Vec<Job>, Vec<Job>) = std::mem::take(&mut inner.jobs)
            .into_iter()
            .partition(|job| job.extension() == extension);
        inner.jobs = kept;
        stale
    }

    fn next_generation(&self) -> u64 {
        let mut inner = self.inner.borrow_mut();
        let generation = inner.next_generation;
        inner.next_generation += 1;
        generation
    }

    /// Open the hidden page under `key`: the helper's, or an extension's
    /// relay.
    fn open_page(self: &Rc<Self>, key: &str) {
        let is_helper = key == HELPER_KEY;
        let url = if is_helper {
            format!("chrome-extension://{HELPER_ID}/helper.html")
        } else {
            format!("chrome-extension://{key}{RELAY_PATH}")
        };
        let generation = self.next_generation();
        match HiddenPage::open(self, key, &url, generation) {
            Some(page) => {
                self.inner.borrow_mut().pages.insert(key.to_string(), page);
            }
            None if is_helper => {
                log::error!("bridge: Chromium could not create the helper's page");
            }
            None => log::error!("bridge: Chromium could not create the relay of {key}"),
        }
    }

    /// Close `extension`'s relay (or the helper's page, under `HELPER_KEY`) on
    /// purpose. Its pending evaluations go with it. Its debuggers are kept: a
    /// retry opens a fresh relay for a worker that may still be debugging.
    fn close_relay(&self, extension: &str) {
        let page = {
            let mut inner = self.inner.borrow_mut();
            if extension == HELPER_KEY {
                inner.refreshing = false;
            }
            drop_pending(&mut inner, extension, "its page was closed");
            inner.pages.remove(extension)
        };
        if let Some(page) = page {
            page.close();
        }
    }

    /// `extension` is off or gone (`Evidence::ExtensionGone`): its debuggers
    /// go, and the tabs they debugged are hidden again when none is left. Any
    /// other evidence than the rule's positive ones forgets nothing.
    fn forget_debuggers(&self, extension: &str, evidence: Evidence) {
        if !forgets(evidence) {
            return;
        }
        let freed = {
            let mut inner = self.inner.borrow_mut();
            inner
                .debuggers
                .forget_extension(extension)
                .into_iter()
                .map(|tab| (tab, tab_handles(&inner, tab), inner.table.chromium(tab)))
                .collect::<Vec<_>>()
        };
        for (tab, handles, chromium) in freed {
            log::info!("bridge: {tab:?} has no debugger left ({extension} is gone)");
            self.hide_again(tab, handles, chromium);
        }
    }

    /// The debugger of `extension` on `tab` is forgotten on `evidence`
    /// (`policy::forgets`); the tab is hidden if no debugger is left on it.
    fn undebug(&self, extension: &str, tab: TabId, evidence: Evidence) {
        if !forgets(evidence) {
            return;
        }
        let (handles, chromium) = {
            let mut inner = self.inner.borrow_mut();
            if inner.debuggers.detach(extension, tab) != Change::Undebugged {
                return;
            }
            (tab_handles(&inner, tab), inner.table.chromium(tab))
        };
        log::info!(
            "bridge: {tab:?} has no debugger left ({extension}, its worker says it has no session on the tab: {evidence:?})"
        );
        self.hide_again(tab, handles, chromium);
    }

    /// Hide `tab` once no debugger is left on it, and log that it was hidden
    /// again. Every path that takes a tab's last debugger away ends here, so
    /// the line has one wording: the driver's checks read it, and it is
    /// logged only when the tab was hidden.
    fn hide_again(&self, tab: TabId, handles: Option<TabHandles>, chromium: Option<i64>) {
        let Some((browser, view)) = handles else {
            return;
        };
        if set_tab_debugged(&browser, &view, false) {
            log::info!(
                "bridge: {tab:?} (Chromium tab {}) hidden again: no debugger is left on it",
                chromium_label(chromium)
            );
        }
    }

    /// Acknowledge a debugger report: the worker waits for the answer, and
    /// must not hang on it whatever became of the report.
    fn acknowledge(&self, extension: &str, instance: &str, call: u64) {
        self.reply(extension, instance, call, &json!({"result": null}));
    }

    /// Close every hidden page; the engine pumps until they are gone.
    pub(crate) fn shutdown(&self) {
        // The pages are taken out before they are closed: closing one can
        // call back into this bridge (its DevTools session ends), which
        // must find the state free.
        let (pages, tabs): (Vec<HiddenPage>, Vec<TabRecord>) = {
            let mut guard = self.inner.borrow_mut();
            let inner = &mut *guard;
            let tabs: Vec<TabRecord> = inner.tabs.drain().map(|(_, record)| record).collect();
            inner.pending.clear();
            inner.jobs.clear();
            inner.wanted.clear();
            let pages: Vec<HiddenPage> = inner.pages.drain().map(|(_, page)| page).collect();
            (pages, tabs)
        };
        // The tabs' records and the pages are let go with no borrow held.
        drop(tabs);
        for page in pages {
            page.close();
        }
    }

    /// Evaluate `expression` in the page under `key`. The pending entry is
    /// recorded before the call, under a message id of the bridge's own
    /// (`Inner::next_message`), so an answer that CEF gives inside the call
    /// finds it (`page_result`). The state is not borrowed across the call.
    fn evaluate(&self, key: &str, expression: &str, pending: Pending) -> bool {
        let (browser, id) = {
            let mut inner = self.inner.borrow_mut();
            let Some(browser) = page_ref(&inner, key).map(|page| page.browser.clone()) else {
                return false;
            };
            let id = inner.next_message;
            inner.next_message += 1;
            inner.pending.insert((key.to_string(), id), pending);
            (browser, id)
        };
        let sent = HiddenPage::evaluate(&browser, id, expression);
        if sent == Some(id) {
            return true;
        }
        // The entry may be gone already: its page can be lost inside the call.
        let mut inner = self.inner.borrow_mut();
        let entry = inner.pending.remove(&(key.to_string(), id));
        match sent {
            Some(sent) => {
                // Not expected (see `Inner::next_message`). An answer that came
                // inside the call found no entry and is lost.
                log::warn!("bridge: CEF replaced message id {id} with {sent} for the page {key}");
                inner.next_message = inner.next_message.max(sent.saturating_add(1));
                if let Some(entry) = entry {
                    inner.pending.insert((key.to_string(), sent), entry);
                }
                true
            }
            None => false,
        }
    }

    fn page_is_ready(&self, key: &str) -> Option<bool> {
        let inner = self.inner.borrow();
        page_ref(&inner, key).map(|page| page.ready.get())
    }

    /// A hidden page's main frame loaded.
    pub(super) fn page_ready(self: &Rc<Self>, key: &str, generation: u64) {
        {
            let inner = self.inner.borrow();
            if !is_current(&inner, key, generation) {
                return;
            }
            if let Some(page) = page_ref(&inner, key) {
                page.ready.set(true);
            }
        }
        log::info!("bridge: page {key} ready");
        // The page's script is checked first (`Pending::Probe`): a page whose
        // script did not load is closed, and its success starts the poll.
        self.probe(key);
        if !self.inner.borrow().jobs.is_empty() {
            self.refresh();
        }
    }

    /// Check that the page under `key` has its script. The answer is in
    /// `page_result`: a success counts as the page working (its openings
    /// start over) and starts what the page is for.
    fn probe(&self, key: &str) {
        if !self.evaluate(key, PROBE, Pending::Probe) {
            log::warn!("bridge: cannot check the page {key}");
        }
    }

    /// A hidden page is gone: it was closed, its renderer terminated, or its
    /// DevTools session ended. Its pending evaluations are dropped, and the
    /// jobs that need it fail now (`policy::fails_on_loss`): a debugger
    /// report does not, since it needs the table, and waits for the next
    /// join. A wanted relay is opened again by the next turn (`reopen`),
    /// never from this callback. `close` says the page still has to be
    /// closed by us.
    pub(super) fn page_gone(self: &Rc<Self>, key: &str, generation: u64, why: &str, close: bool) {
        let (page, stale_jobs) = {
            let mut inner = self.inner.borrow_mut();
            let Some(page) = take_page(&mut inner, key, generation) else {
                return;
            };
            drop_pending(&mut inner, key, why);
            let is_helper = key == HELPER_KEY;
            if is_helper {
                inner.refreshing = false;
            }
            let (stale, kept): (Vec<Job>, Vec<Job>) = std::mem::take(&mut inner.jobs)
                .into_iter()
                .partition(|job| fails_on_loss(job.kind(), job.extension(), key, is_helper));
            inner.jobs = kept;
            (page, stale)
        };
        log::warn!("bridge: page {key} is gone ({why})");
        // The extension's debuggers are kept: its worker may still be
        // debugging, and its detach comes through the page that reopens.
        // `sync` forgets them when the extension is no longer wanted.
        if close {
            page.close();
        }
        self.fail_jobs(stale_jobs, "its page is gone");
    }

    /// A page's DevTools session ended: the page cannot be evaluated in any
    /// more, so it is gone, like a closed one. It is closed and removed here,
    /// and the next turn opens it again if it is still wanted. It is not
    /// asked again from this callback: CEF reports the end again to a call
    /// made inside it, which turned into an endless loop (the run that found
    /// this logged the report thousands of times in a millisecond).
    pub(super) fn page_detached(self: &Rc<Self>, key: &str, generation: u64) {
        let current = is_current(&self.inner.borrow(), key, generation);
        if current {
            self.page_gone(key, generation, "its DevTools session ended", true);
        }
    }

    /// Wait for the next requests of `extension`'s worker.
    fn poll(&self, extension: &str) {
        if !self.evaluate(extension, "nautilusRelay.next()", Pending::Poll) {
            log::warn!("bridge: cannot listen to the relay of {extension}");
        }
    }

    /// Queue `job`, then keep the queue within its bound.
    fn queue(&self, job: Job) {
        self.inner.borrow_mut().jobs.push(job);
        self.trim();
    }

    /// Keep the queue within `MAX_JOBS` (`policy::overflow`): a job that
    /// leaves is failed, or, when every queued job is a debugger report, the
    /// oldest is applied against the table now (a debugger report is never
    /// acknowledged and dropped). Repeats until the queue fits.
    fn trim(&self) {
        loop {
            let (job, fail) = {
                let mut inner = self.inner.borrow_mut();
                let kinds: Vec<JobKind> = inner.jobs.iter().map(Job::kind).collect();
                match overflow(&kinds, MAX_JOBS) {
                    Overflow::Fits => return,
                    Overflow::Fail(index) => (inner.jobs.remove(index), true),
                    Overflow::Apply(index) => (inner.jobs.remove(index), false),
                }
            };
            if fail {
                self.fail_jobs(vec![job], "too many were waiting");
            } else {
                self.run(job);
            }
        }
    }

    /// Ask the helper for the targets; the waiting jobs run when it
    /// answers. With the helper not ready yet they wait for it. `refreshing`
    /// is set before the call, which can answer at once.
    fn refresh(&self) {
        if self.given_up(HELPER_KEY) {
            let jobs = std::mem::take(&mut self.inner.borrow_mut().jobs);
            self.fail_jobs(jobs, "the helper's page did not load");
            return;
        }
        if self.inner.borrow().refreshing {
            return;
        }
        if self.page_is_ready(HELPER_KEY) != Some(true) {
            log::debug!(
                "bridge: the helper is not ready; {} job(s) wait",
                self.inner.borrow().jobs.len()
            );
            return;
        }
        self.inner.borrow_mut().refreshing = true;
        if !self.evaluate(HELPER_KEY, "nautilusHelper.targets()", Pending::Targets) {
            self.inner.borrow_mut().refreshing = false;
            log::warn!("bridge: cannot ask the helper for the targets");
        }
    }

    /// `extension`'s action was clicked for `tab`. False when the
    /// extension is not wanted a relay (it is not bridged, or is off).
    pub(crate) fn click(&self, extension: &str, tab: TabId) -> bool {
        match self.deliver(extension, |_| Payload::Click { tab }) {
            Ok(()) => true,
            Err(Undeliverable::GivenUp) => {
                log::warn!("bridge: the click on {extension} is refused: its page did not load");
                false
            }
            Err(Undeliverable::NotWanted) => false,
        }
    }

    /// Queue a delivery to `extension`'s worker; `payload` is given the
    /// worker instance the extension is known to have. An extension that
    /// is not bridged and on, or whose relay (or the helper's page) has
    /// been given up, has nothing to deliver to. A wanted relay that
    /// closed underneath is reopened on the next turn, and the delivery
    /// waits for it (`job_turn`).
    fn deliver(
        &self,
        extension: &str,
        payload: impl FnOnce(Option<String>) -> Payload,
    ) -> Result<(), Undeliverable> {
        if self.given_up(extension) || self.given_up(HELPER_KEY) {
            return Err(Undeliverable::GivenUp);
        }
        let delivery = {
            let mut inner = self.inner.borrow_mut();
            if !inner.wanted.iter().any(|id| id == extension) {
                return Err(Undeliverable::NotWanted);
            }
            let id = inner.next_delivery;
            inner.next_delivery += 1;
            Delivery {
                id,
                attempt: 0,
                payload: payload(inner.identities.get(extension).cloned()),
            }
        };
        self.queue(Job::Deliver {
            extension: extension.to_string(),
            delivery,
        });
        self.refresh();
        Ok(())
    }

    /// One request of `extension`'s worker, as the relay passed it on. The
    /// debugger reports are the bridge's own and stay here; every other method
    /// goes to the hub, which answers it (the router) or sends it to the model,
    /// and whose answer comes back through `BridgeTransport::reply`.
    fn worker_call(&self, extension: &str, item: &serde_json::Value) {
        let Some(call) = item.get("call") else {
            return;
        };
        let Some(worker_id) = call.get("id").and_then(|v| v.as_u64()) else {
            return;
        };
        let method = call
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        // The worker instance that made the call: its answer goes back only to it.
        let instance = call
            .get("instance")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        log::debug!("bridge: {extension} calls {method} ({worker_id})");
        if method == "debugger.attached" || method == "debugger.detached" {
            let debuggee = debuggee_of(call.get("args").and_then(|args| args.get(0)));
            self.debugger_report(
                extension,
                &instance,
                worker_id,
                debuggee,
                method == "debugger.attached",
            );
            return;
        }
        let (namespace, name) = method.split_once('.').unwrap_or((method, ""));
        let args = call
            .get("args")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let id = self
            .inner
            .borrow_mut()
            .calls
            .issue(extension, &instance, worker_id);
        let Some(hub) = self.delegate.hub() else {
            self.inner.borrow_mut().calls.take(id);
            self.reply(
                extension,
                &instance,
                worker_id,
                &json!({"error": "The engine is not ready."}),
            );
            return;
        };
        hub.call(
            Lane::Worker,
            BridgeCall {
                id,
                extension: extension.to_string(),
                asker: Asker::Worker,
                namespace: namespace.to_string(),
                method: name.to_string(),
                args,
                page: None,
            },
        );
    }

    /// The worker attached or detached a debugger to a tab. The worker's
    /// call is answered only after the tab is shown or hidden, so the
    /// debugger's first command never meets a hidden renderer.
    fn debugger_report(
        &self,
        extension: &str,
        instance: &str,
        call: u64,
        debuggee: Option<Debuggee>,
        attach: bool,
    ) {
        let Some(debuggee) = debuggee else {
            log::warn!("bridge: {extension}: a debugger report names no tab or target");
            self.acknowledge(extension, instance, call);
            return;
        };
        let known = self.inner.borrow().table.resolve(&debuggee);
        match known {
            Some(tab) => {
                self.set_debugged(extension, tab, attach);
                self.acknowledge(extension, instance, call);
            }
            // Not in the table yet: the next join decides it (`run`).
            None => {
                self.queue(Job::Debugger {
                    extension: extension.to_string(),
                    call,
                    instance: instance.to_string(),
                    debuggee,
                    attach,
                });
                self.refresh();
            }
        }
    }

    /// Apply one attach or detach to the set of debuggers, and show or
    /// hide the tab when its debugged state changed.
    fn set_debugged(&self, extension: &str, tab: TabId, attach: bool) {
        let (change, handles, chromium) = {
            let mut inner = self.inner.borrow_mut();
            let change = if attach {
                // A session reported before this attach may be stale: pending
                // sessions are dropped (a dropped one only keeps a debugger).
                inner.pending_sessions.remove(extension);
                inner.debuggers.attach(extension, tab)
            } else {
                inner.debuggers.detach(extension, tab)
            };
            (change, tab_handles(&inner, tab), inner.table.chromium(tab))
        };
        match change {
            Change::Unchanged => {
                log::debug!("bridge: {extension}: {tab:?} debugger unchanged");
            }
            Change::Debugged => {
                log::info!("bridge: {extension} debugs {tab:?}; the tab stays painted");
                if let Some((browser, view)) = handles
                    && set_tab_debugged(&browser, &view, true)
                {
                    log::info!(
                        "bridge: {tab:?} (Chromium tab {}) was hidden; shown for the debugger of {extension}",
                        chromium_label(chromium)
                    );
                }
            }
            Change::Undebugged => {
                log::info!("bridge: {extension} no longer debugs {tab:?}");
                self.hide_again(tab, handles, chromium);
            }
        }
    }

    /// Answer call `call` of the worker instance `instance`. The bridge
    /// refuses an answer for another instance, whose call ids it may share.
    fn reply(&self, extension: &str, instance: &str, call: u64, answer: &serde_json::Value) {
        let current = self.inner.borrow().identities.get(extension).cloned();
        if !reply_due(instance, current.as_deref()) {
            log::debug!(
                "bridge: {extension}: call {call} of its old worker instance {instance} is not answered (the worker is {})",
                current.unwrap_or_default()
            );
            return;
        }
        let instance = json!(instance);
        let expression = format!("nautilusRelay.reply({call}, {answer}, {instance})");
        if !self.evaluate(extension, &expression, Pending::Reply) {
            log::warn!("bridge: cannot answer call {call} of {extension}");
        }
    }

    /// `extension`'s worker said who it is: its hello, from the port it is on
    /// or the answer to `identify`. The rule (`policy::forgets`) decides what
    /// goes: only a tab the hello names as not attached is forgotten; an
    /// attached or unknown tab, and a tab the hello does not name, keeps its
    /// debugger.
    ///
    /// A new instance drops the queued tab lists of the old one (their call
    /// ids belong to it). A queued debugger report of the old instance is kept
    /// and applied (its session survives a stop), with its answer suppressed
    /// (`reply_due`). A session for a tab the table cannot name yet is kept
    /// pending (`apply_pending`).
    fn worker_hello(&self, extension: &str, hello: &Hello) {
        let (identity, stale, forget, pending, retired) = {
            let mut inner = self.inner.borrow_mut();
            let seen = inner.identities.get(extension).cloned();
            let identity = identity(seen.as_deref(), &hello.instance);
            inner
                .identities
                .insert(extension.to_string(), hello.instance.clone());
            let current = hello.instance.as_str();
            let (stale, kept): (Vec<Job>, Vec<Job>) = std::mem::take(&mut inner.jobs)
                .into_iter()
                .partition(|job| is_stale_event(job, extension, current));
            inner.jobs = kept;
            // The answers to the old instance's calls would be ignored by the
            // worker that now runs: those calls are retired, and their answers
            // are not sent (`BridgeTransport::reply`).
            let retired = inner.calls.retire_instances(extension, current);
            let mut resolved = Vec::new();
            let mut pending = Vec::new();
            for (chromium, state) in &hello.sessions {
                match inner.table.tab(*chromium) {
                    Some(tab) => resolved.push((tab, *state)),
                    None => pending.push((*chromium, *state)),
                }
            }
            if pending.is_empty() {
                inner.pending_sessions.remove(extension);
            } else {
                inner
                    .pending_sessions
                    .insert(extension.to_string(), pending);
            }
            let tracked = inner.debuggers.tabs_of(extension);
            let forget = tabs_to_forget(&resolved, &tracked);
            let pending_count = inner.pending_sessions.get(extension).map_or(0, Vec::len);
            (identity, stale, forget, pending_count, retired)
        };
        let count = |state: Session| hello.sessions.iter().filter(|(_, s)| *s == state).count();
        if identity == Identity::Restarted {
            log::info!(
                "bridge: {extension} runs a new worker instance; its debugger sessions: {} attached, {} not attached, {} unknown",
                count(Session::Attached),
                count(Session::NotAttached),
                count(Session::Unknown)
            );
        } else {
            log::info!(
                "bridge: {extension}'s worker says hello: {} attached, {} not attached, {} unknown ({pending} waiting for the table)",
                count(Session::Attached),
                count(Session::NotAttached),
                count(Session::Unknown),
                pending = pending
            );
        }
        if retired > 0 {
            log::info!(
                "bridge: {extension}: {retired} call(s) of its replaced worker instance are retired; no answer is sent for them"
            );
        }
        for job in stale {
            if let Job::Deliver { delivery, .. } = job {
                log::warn!(
                    "bridge: {extension}: {} is dropped: its worker instance was replaced",
                    delivery.label()
                );
            }
        }
        for tab in forget {
            self.undebug(extension, tab, session_evidence(Session::NotAttached));
        }
    }

    /// Apply the sessions a hello named for tabs the table could not name then,
    /// now that a join may name them (`worker_hello`). Only a tab the worker
    /// says is not attached is forgotten, and only if it is still debugged by
    /// that extension.
    fn apply_pending(&self) {
        let mut forget = Vec::new();
        {
            let mut inner = self.inner.borrow_mut();
            let extensions: Vec<String> = inner.pending_sessions.keys().cloned().collect();
            for extension in extensions {
                let Some(list) = inner.pending_sessions.remove(&extension) else {
                    continue;
                };
                let mut still = Vec::new();
                for (chromium, state) in list {
                    match inner.table.tab(chromium) {
                        Some(tab) => {
                            if inner.debuggers.tabs_of(&extension).contains(&tab) {
                                forget.push((extension.clone(), tab, session_evidence(state)));
                            }
                        }
                        None => still.push((chromium, state)),
                    }
                }
                if !still.is_empty() {
                    inner.pending_sessions.insert(extension, still);
                }
            }
        }
        for (extension, tab, evidence) in forget {
            self.undebug(&extension, tab, evidence);
        }
    }

    /// A worker's hello reached the engine: the relay that heard it reaches its
    /// worker, so its unreached count is cleared (`Opener::unreached`).
    fn hello_reached(&self, extension: &str) {
        if let Some(opener) = self.inner.borrow_mut().openers.get_mut(extension) {
            opener.unreached = 0;
        }
    }

    /// A relay passed its probe and then could not reach its worker: the
    /// opening is a failed one for the schedule. Its relay is closed by
    /// the caller; its debuggers and queued jobs are kept.
    fn unreached(&self, extension: &str) {
        self.inner
            .borrow_mut()
            .openers
            .entry(extension.to_string())
            .or_default()
            .unreached += 1;
    }

    /// Whether `extension`'s debuggers or queued debugger reports make its
    /// worker's identity matter: a worker that stopped or was reloaded may
    /// have ended them, so the engine asks (`identify`).
    fn needs_identity(&self, extension: &str) -> bool {
        let inner = self.inner.borrow();
        !inner.debuggers.tabs_of(extension).is_empty()
            || inner.jobs.iter().any(
                |job| matches!(job, Job::Debugger { extension: e, .. } if e.as_str() == extension),
            )
    }

    /// Ask `extension`'s relay which worker instance it reaches. The relay
    /// starts the worker if it is stopped, and answers with its `hello`.
    fn identify(&self, extension: &str) {
        log::info!(
            "bridge: {extension}: asks its relay which worker it reaches (its debuggers or a queued report need it)"
        );
        if !self.evaluate(
            extension,
            "nautilusRelay.identify()",
            Pending::Identify {
                extension: extension.to_string(),
            },
        ) {
            log::warn!("bridge: cannot ask the relay of {extension} for its worker");
        }
    }

    /// Run the jobs whose relay is ready, with the table as it now is;
    /// a job whose relay is still loading waits, one whose relay is
    /// gone fails.
    fn run_jobs(self: &Rc<Self>) {
        let jobs = std::mem::take(&mut self.inner.borrow_mut().jobs);
        for job in jobs {
            let turn = {
                let inner = self.inner.borrow();
                let wanted = inner.wanted.iter().any(|id| id == job.extension());
                job_turn(
                    page_ref(&inner, job.extension()).map(|p| p.ready.get()),
                    wanted,
                )
            };
            match turn {
                JobTurn::Wait => self.inner.borrow_mut().jobs.push(job),
                JobTurn::Drop => self.fail_jobs(vec![job], "its relay is gone"),
                JobTurn::Run => self.run(job),
            }
        }
        // Jobs kept above, and any queued by running them, go through the bound.
        self.trim();
    }

    /// Fail jobs that cannot be carried out: a waiting tab list is
    /// answered with an error, a waiting debugger report with its
    /// acknowledgement (the worker must not hang on it), and a click is
    /// logged as dropped.
    fn fail_jobs(&self, jobs: Vec<Job>, why: &str) {
        for job in jobs {
            match job {
                Job::Deliver {
                    extension,
                    delivery,
                } => match delivery.payload {
                    Payload::Click { tab } => log::warn!(
                        "bridge: {extension}: the click on {tab:?} (delivery {}) is dropped: {why}",
                        delivery.id
                    ),
                    Payload::Event { .. } => log::warn!(
                        "bridge: {extension}: {} is dropped: {why}",
                        delivery.label()
                    ),
                },
                Job::Debugger {
                    extension,
                    call,
                    instance,
                    debuggee,
                    ..
                } => {
                    log::warn!(
                        "bridge: {extension}: the debugger of {debuggee:?} is not tracked: {why}"
                    );
                    self.acknowledge(&extension, &instance, call);
                }
            }
        }
    }

    /// Hand `delivery` to `extension`'s relay, which passes it to the
    /// worker. A click names its tab by Chromium's id, read from the table
    /// now; a click on a tab the table cannot name is dropped.
    fn send(&self, extension: &str, delivery: Delivery) {
        let id = delivery.id;
        let (message, sent, unreachable) = match &delivery.payload {
            Payload::Click { tab } => {
                let chromium = self.inner.borrow().table.chromium(*tab);
                let Some(chromium) = chromium else {
                    log::warn!(
                        "bridge: no Chromium tab id for {tab:?}; the click on {extension} is dropped"
                    );
                    return;
                };
                (
                    json!({"id": id, "event": "action.onClicked", "tabId": chromium}),
                    format!(
                        "bridge: {extension}: delivery {id} (action.onClicked, tab {chromium}) sent"
                    ),
                    format!("bridge: cannot reach the relay of {extension}"),
                )
            }
            Payload::Event { event, args, .. } => (
                json!({"id": id, "event": event, "args": args}),
                format!("bridge: {extension}: event {event} (delivery {id}) sent"),
                format!("bridge: cannot reach the relay of {extension} for {event}"),
            ),
        };
        // A click's sending is part of a run's record; an event's is not.
        let level = match delivery.payload {
            Payload::Click { .. } => log::Level::Info,
            Payload::Event { .. } => log::Level::Debug,
        };
        let expression = format!("nautilusRelay.deliver({message})");
        if self.evaluate(extension, &expression, Pending::Deliver(delivery)) {
            log::log!(level, "{sent}");
        } else {
            log::warn!("{unreachable}");
        }
    }

    fn run(&self, job: Job) {
        match job {
            Job::Deliver {
                extension,
                delivery,
            } => self.send(&extension, delivery),
            Job::Debugger {
                extension,
                call,
                instance,
                debuggee,
                attach,
            } => {
                let tab = self.inner.borrow().table.resolve(&debuggee);
                match tab {
                    Some(tab) => self.set_debugged(&extension, tab, attach),
                    None => log::warn!(
                        "bridge: {extension}: {debuggee:?} is no Nautilus tab in the table; its debugger is not tracked"
                    ),
                }
                self.acknowledge(&extension, &instance, call);
            }
        }
    }

    /// An evaluation in a hidden page answered.
    pub(super) fn page_result(
        self: &Rc<Self>,
        key: &str,
        generation: u64,
        id: i32,
        success: bool,
        result: &[u8],
    ) {
        let pending = {
            let mut inner = self.inner.borrow_mut();
            if !is_current(&inner, key, generation) {
                return;
            }
            inner.pending.remove(&(key.to_string(), id))
        };
        let Some(pending) = pending else {
            return;
        };
        let value = evaluation(success, result);
        match pending {
            Pending::Targets => {
                let targets = {
                    let mut inner = self.inner.borrow_mut();
                    inner.refreshing = false;
                    match &value {
                        Ok(value) => {
                            let targets = parse_targets(value);
                            let before = inner.table.rows().to_vec();
                            inner.table.join(&targets);
                            log::debug!(
                                "bridge: table has {} tab(s) of {} target(s)",
                                inner.table.rows().len(),
                                targets.len()
                            );
                            // The rows name each tab by both its id and
                            // Chromium's, so a run's log can match them.
                            if before != inner.table.rows() {
                                log::info!(
                                    "bridge: table has {} tab(s) of {} target(s)",
                                    inner.table.rows().len(),
                                    targets.len()
                                );
                                for row in inner.table.rows() {
                                    log::info!(
                                        "bridge: table: {:?} is Chromium tab {} at {}",
                                        row.tab,
                                        row.chromium,
                                        row.url
                                    );
                                }
                            }
                            Some(targets)
                        }
                        Err(e) => {
                            log::warn!("bridge: the helper could not list the targets: {e}");
                            None
                        }
                    }
                };
                if let Some(targets) = targets {
                    self.check_worker_targets(&targets);
                    self.apply_pending();
                }
                self.run_jobs();
                // The hub is called with no borrow of `inner` held: it calls
                // back into this bridge.
                let hub = self.delegate.hub();
                if let Some(hub) = hub {
                    hub.table_changed();
                }
            }
            Pending::Probe => match value {
                Ok(Value::Bool(true)) => {
                    // The page works: its openings start over.
                    self.inner
                        .borrow_mut()
                        .openers
                        .entry(key.to_string())
                        .or_default()
                        .failed = 0;
                    if key == HELPER_KEY {
                        if !self.inner.borrow().jobs.is_empty() {
                            self.refresh();
                        }
                    } else {
                        self.poll(key);
                        if self.needs_identity(key) {
                            self.identify(key);
                        }
                    }
                }
                Ok(other) => {
                    log::warn!(
                        "bridge: the page {key} has no script ({other}); it is closed and opened again later"
                    );
                    self.close_relay(key);
                }
                Err(e) => {
                    log::warn!(
                        "bridge: the page {key} did not answer its check ({e}); it is closed"
                    );
                    self.close_relay(key);
                }
            },
            Pending::Identify { extension } => match value.map(|hello| hello_of(&hello)) {
                Ok(Some(hello)) => {
                    self.hello_reached(&extension);
                    self.worker_hello(&extension, &hello);
                }
                // The relay cannot reach its worker (or its hello is malformed):
                // the relay is closed, so the next turn opens a fresh one, and
                // the opening counts as failed (`unreached`). Nothing is
                // forgotten: a silent worker says nothing about its sessions.
                Ok(None) => {
                    log::warn!(
                        "bridge: {extension}'s worker sent a hello with no instance; its relay is closed and opened again"
                    );
                    self.unreached(&extension);
                    self.close_relay(&extension);
                }
                Err(e) => {
                    log::warn!(
                        "bridge: {extension}'s worker did not say who it is ({e}); its relay is closed and opened again, and its debuggers are kept"
                    );
                    self.unreached(&extension);
                    self.close_relay(&extension);
                }
            },
            Pending::Ask { extension, change } => match value.map(|hello| hello_of(&hello)) {
                Ok(Some(hello)) => {
                    self.hello_reached(&extension);
                    self.worker_hello(&extension, &hello);
                    self.decide_ask(&extension, change, Answered::Live);
                }
                Ok(None) => {
                    log::warn!("bridge: {extension}'s answer to change {change} has no instance");
                    self.decide_ask(&extension, change, Answered::Unanswered);
                }
                Err(e) => {
                    log::info!("bridge: {extension}'s relay did not answer change {change} ({e})");
                    self.decide_ask(&extension, change, Answered::Unanswered);
                }
            },
            #[cfg(feature = "test-hooks")]
            Pending::Inspect { label } => match value {
                Ok(answer) => log::info!("bridge: relay {key} answers {label}: {answer}"),
                Err(e) => log::info!("bridge: relay {key} answers {label} with an error: {e}"),
            },
            Pending::Poll => match value {
                Ok(items) => {
                    for item in items.as_array().into_iter().flatten() {
                        if let Some(hello) = item.get("hello") {
                            // The worker's own hello, on a port it opened: the
                            // only way its identity comes unasked (no identify).
                            if let Some(hello) = hello_of(hello) {
                                self.hello_reached(key);
                                self.worker_hello(key, &hello);
                            }
                        } else if item.get("gone").is_some() {
                            // The worker's port went: it stopped (an idle stop,
                            // which keeps its sessions) or was reloaded. Nothing
                            // is asked: asking would start a stopped worker
                            // A reload is seen by the worker target
                            // (`check_worker_targets`), and the next port's
                            // hello says the rest.
                            log::debug!("bridge: {key}'s worker port closed; nothing is asked");
                        } else {
                            self.worker_call(key, item);
                        }
                    }
                    self.poll(key);
                }
                // A failed evaluation means the relay is gone (its extension
                // was reloaded or removed): it is closed, and the next turn
                // opens it again if it is still wanted.
                Err(e) => {
                    log::warn!("bridge: the relay of {key} stopped listening: {e}");
                    self.close_relay(key);
                }
            },
            Pending::Reply => {
                if let Err(e) = value {
                    log::warn!("bridge: an answer to {key} was lost: {e}");
                }
            }
            Pending::Deliver(delivery) => self.delivered(key, delivery, &value),
        }
    }

    /// The relay of `extension` answered a delivery (`policy::after_delivery`).
    fn delivered(&self, extension: &str, mut delivery: Delivery, outcome: &Result<Value, String>) {
        let label = delivery.label();
        match after_delivery(outcome, delivery.attempt) {
            Next::Finished => match delivery.payload {
                Payload::Click { .. } => {
                    let attempts = outcome
                        .as_ref()
                        .ok()
                        .and_then(|v| v.get("attempts"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    log::info!(
                        "bridge: {extension}: {label} taken by the worker after {attempts} attempt(s)"
                    );
                }
                Payload::Event { .. } => {
                    log::debug!("bridge: {extension}: {label} taken by the worker");
                }
            },
            Next::Retry => {
                log::warn!(
                    "bridge: {extension}: {label} was not acknowledged ({}); trying once more with a fresh relay",
                    describe(outcome)
                );
                // Closed here, reopened by the next turn (`reopen`): a
                // callback creates no browser. The id is kept, so an
                // instance that ran the delivery ignores the repeat.
                self.close_relay(extension);
                delivery.attempt += 1;
                self.queue(Job::Deliver {
                    extension: extension.to_string(),
                    delivery,
                });
            }
            Next::Abandon { reopen } => {
                log::warn!(
                    "bridge: {extension}: {label} dropped: {}",
                    describe(outcome)
                );
                if reopen {
                    self.close_relay(extension);
                }
            }
        }
    }

    /// Evaluate `expression` in `extension`'s relay page and log its value as
    /// `label` (a test control's inspection, `TEST HOOK: relay_eval`). Nothing
    /// else reads the answer.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn inspect_relay(&self, extension: &str, label: &str, expression: &str) {
        if !self.evaluate(
            extension,
            expression,
            Pending::Inspect {
                label: label.to_string(),
            },
        ) {
            log::info!("bridge: relay {extension} has no page to answer {label}");
        }
    }

    /// The table, for a scripted run's log.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn log_table(&self) {
        let inner = self.inner.borrow();
        for row in inner.table.rows() {
            log::info!(
                "bridge: table: {:?} is Chromium tab {} at {}",
                row.tab,
                row.chromium,
                row.url
            );
        }
        log::info!("bridge: table: {} row(s)", inner.table.rows().len());
    }
}

impl Bridge {
    /// A page reported its own Chromium tab id (`pages.rs`, `Hub::report_tab`):
    /// the table records it or refuses it, and says which in the log.
    pub(crate) fn report_tab(&self, tab: TabId, chromium: i64, extension: &str) -> Report {
        let verdict = self
            .inner
            .borrow_mut()
            .table
            .report(tab, chromium, extension);
        match verdict {
            Report::Recorded => {
                log::info!("bridge: {extension}'s page reports {tab:?} is Chromium tab {chromium}")
            }
            Report::Same => {}
            Report::Refused(why) => log::warn!(
                "bridge: {extension}'s page report of Chromium tab {chromium} for {tab:?} was ignored: {why}"
            ),
        }
        verdict
    }

    /// The Chromium id of `tab`, when the table knows it (the hub's lookup).
    pub(crate) fn chromium_id(&self, tab: TabId) -> Option<i64> {
        self.inner.borrow().table.chromium(tab)
    }

    /// Queue delivery of `event`, with `args`, to `extension`'s worker
    /// (`BridgeTransport::emit`, `wake`). An extension that is not bridged and
    /// on, or whose relay has given up, has nothing to deliver to: the event is
    /// dropped (logged) and never retried, as an undeliverable click is.
    fn event(&self, extension: &str, event: &str, args: &[Value]) {
        let queued = self.deliver(extension, |instance| Payload::Event {
            event: event.to_string(),
            args: args.to_vec(),
            instance,
        });
        match queued {
            Ok(()) => {}
            Err(Undeliverable::GivenUp) => {
                log::warn!("bridge: {event} for {extension} is dropped: its page did not load");
            }
            Err(Undeliverable::NotWanted) => log::debug!(
                "bridge: {event} for {extension} is not delivered: it is not bridged and on"
            ),
        }
    }
}

impl BridgeTransport for Bridge {
    fn reply(&self, call: CallId, result: Result<Value, String>) {
        let taken = self.inner.borrow_mut().calls.take(call);
        let Some((extension, instance, worker_call)) = taken else {
            // The worker instance was replaced (`CallIds::retire_instances`),
            // or the call was already answered: nothing is sent.
            log::debug!("bridge: call {call} has no worker call to answer");
            return;
        };
        let answer = match result {
            Ok(value) => json!({"result": value}),
            Err(message) => json!({"error": message}),
        };
        Bridge::reply(self, &extension, &instance, worker_call, &answer);
    }

    fn emit(&self, extension: &str, event: &str, args: &[Value]) {
        self.event(extension, event, args);
    }

    fn wake(&self, extension: &str) {
        self.event(extension, "bridge.wake", &[]);
    }
}

/// Whether `job` is a queued event of `extension` that the hello of the
/// instance `current` makes stale (`policy::job_kept`).
fn is_stale_event(job: &Job, extension: &str, current: &str) -> bool {
    match job {
        Job::Deliver {
            extension: e,
            delivery:
                Delivery {
                    payload:
                        Payload::Event {
                            instance: Some(instance),
                            ..
                        },
                    ..
                },
        } => e.as_str() == extension && !job_kept(JobKind::Emit, instance, current),
        _ => false,
    }
}

/// Log a pending evaluation that is lost with its page: a delivery or an
/// event among them is unknown to the engine from now on.
fn log_lost(key: &str, pending: &Pending, why: &str) {
    if let Pending::Deliver(delivery) = pending {
        log::warn!("bridge: {key}: {} is unknown ({why})", delivery.label());
    }
}

/// Drop the pending evaluations of `key`; a delivery among them is lost
/// with its page, and the log says so.
fn drop_pending(inner: &mut Inner, key: &str, why: &str) {
    inner.pending.retain(|(k, _), pending| {
        if k != key {
            return true;
        }
        log_lost(key, pending, why);
        false
    });
}

/// A tab's Chromium id as the log shows it, so that a run can name the tab
/// a debugger was on by the id its extension knows it by; `?` when the
/// table does not know it.
fn chromium_label(chromium: Option<i64>) -> String {
    chromium.map_or_else(|| "?".to_string(), |id| id.to_string())
}

/// A delivery's outcome as the log shows it.
fn describe(value: &Result<Value, String>) -> String {
    match value {
        Ok(value) => value.to_string(),
        Err(e) => e.clone(),
    }
}

// One tab's probe for its target id. `sent` is the message id of the one
// `Target.getTargetInfo` it sends (`TAB_PROBE_ID`, set before the call), so
// the probe acts on its own result and on no other method's result.
wrap_dev_tools_message_observer! {
    struct TabProbe {
        bridge: Rc<Bridge>,
        tab: TabId,
        sent: Rc<Cell<i32>>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            if self.sent.get() == message_id {
                self.bridge
                    .target_known(self.tab, success != 0, result.unwrap_or_default());
            }
        }
    }
}
