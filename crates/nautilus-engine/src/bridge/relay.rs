//! The CEF side of the bridge: hidden pages the engine evaluates
//! scripts in (each bridged extension's relay page, and the helper
//! extension's page), and the `Bridge` that owns them.
//!
//! A hidden page is a windowless browser nobody paints. The engine
//! reaches it the way `ExtensionManager` reaches `chrome://extensions`:
//! `Runtime.evaluate` through `execute_dev_tools_method`, the answer in
//! `on_dev_tools_method_result`. No render-process code is involved.
//!
//! The decisions are in `policy` and `debugger` (pure); this file carries
//! them out.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use cef::rc::Rc as _;
use cef::{
    Browser, BrowserSettings, CefString, Client, DevToolsMessageObserver, DialogHandler,
    DisplayHandler, Frame, ImplBrowser, ImplBrowserHost, ImplClient, ImplDevToolsMessageObserver,
    ImplDictionaryValue, ImplDisplayHandler, ImplFrame, ImplLifeSpanHandler, ImplLoadHandler,
    ImplRenderHandler, ImplRequestHandler, LifeSpanHandler, LoadHandler, LogSeverity, Rect,
    Registration, RenderHandler, RequestHandler, TerminationStatus, WindowInfo, WrapClient,
    WrapDevToolsMessageObserver, WrapDisplayHandler, WrapLifeSpanHandler, WrapLoadHandler,
    WrapRenderHandler, WrapRequestHandler, wrap_client, wrap_dev_tools_message_observer,
    wrap_display_handler, wrap_life_span_handler, wrap_load_handler, wrap_render_handler,
    wrap_request_handler,
};
use osmena_core::TabId;
use serde_json::{Value, json};

use super::debugger::{Change, Debuggers};
use super::evaluation;
use super::hub::Lane;
use super::install::HELPER_ID;
use super::policy::{
    AfterAsk, Answered, Evidence, Identity, JobKind, JobTurn, MAX_OPENINGS, Next, Opening,
    Overflow, REFRESH_INTERVAL, Session, WorkerTarget, WorkerWatch, after_ask, after_delivery,
    asks_on, check_due, failed_by_loss, forgets, identity, is_generation, job_kept, job_turn,
    next_opening, overflow, relay_changes, reply_due, session_evidence, tabs_to_forget,
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

/// The sentinel of a tab's probe whose call is in progress (`Probe::sent`).
const RESERVED: i32 = -1;
/// The sentinel of a tab's probe whose call failed: no result carries it.
const NOT_SENT: i32 = -2;

/// Waiting jobs beyond this are failed, oldest first, so a page that
/// never comes back cannot grow the queue without bound.
const MAX_JOBS: usize = 256;

/// The script an evaluation of a hidden page checks first: a page whose
/// script did not load answers `false` (or fails), and is closed.
const PROBE: &str = "typeof nautilusRelay === 'object' || typeof nautilusHelper === 'object'";

#[cfg(target_os = "macos")]
fn no_window() -> cef::sys::cef_window_handle_t {
    std::ptr::null_mut()
}

#[cfg(windows)]
fn no_window() -> cef::sys::cef_window_handle_t {
    cef::sys::HWND(std::ptr::null_mut())
}

/// A page in a windowless browser that is never shown.
struct HiddenPage {
    browser: Browser,
    /// Tells this page from an earlier one under the same key: a callback
    /// from a page that is gone must not change its successor.
    generation: u64,
    /// The DevTools observer stays registered while this lives.
    _devtools: Option<Registration>,
    /// Its main frame finished loading.
    ready: Cell<bool>,
}

impl HiddenPage {
    fn open(bridge: &Rc<Bridge>, key: &str, url: &str, generation: u64) -> Option<HiddenPage> {
        let mut client = PageClient::new(
            PageRender::new(),
            PageLife::new(
                bridge.delegate.clone(),
                bridge.clone(),
                key.to_string(),
                generation,
            ),
            PageLoad::new(bridge.clone(), key.to_string(), generation),
            PageDisplay::new(key.to_string()),
            bridge.delegate.dialog_handler(),
            PageRequest::new(bridge.clone(), key.to_string(), generation),
        );
        let window_info = WindowInfo::default().set_as_windowless(no_window());
        let settings = BrowserSettings {
            windowless_frame_rate: 1,
            ..BrowserSettings::default()
        };
        let browser = cef::browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&CefString::from(url)),
            Some(&settings),
            None,
            None,
        )?;
        let mut observer = PageDevTools::new(bridge.clone(), key.to_string(), generation);
        let devtools = browser
            .host()
            .and_then(|host| host.add_dev_tools_message_observer(Some(&mut observer)));
        log::debug!("bridge: page {key} opening at {url}");
        Some(HiddenPage {
            browser,
            generation,
            _devtools: devtools,
            ready: Cell::new(false),
        })
    }

    /// Evaluate `expression` (a promise is awaited, the value returned
    /// by value); the DevTools message id the answer will carry.
    fn evaluate(browser: &Browser, expression: &str) -> Option<i32> {
        let host = browser.host()?;
        let mut params = cef::dictionary_value_create()?;
        params.set_string(
            Some(&"expression".into()),
            Some(&CefString::from(expression)),
        );
        params.set_bool(Some(&"awaitPromise".into()), 1);
        params.set_bool(Some(&"returnByValue".into()), 1);
        // Id 0 asks CEF for the next id of this browser; a caller's
        // own id may be raised, so the returned one is what counts.
        let id =
            host.execute_dev_tools_method(0, Some(&"Runtime.evaluate".into()), Some(&mut params));
        (id != 0).then_some(id)
    }

    fn close(&self) {
        if let Some(host) = self.browser.host() {
            host.close_browser(1);
        }
    }
}

wrap_client! {
    struct PageClient {
        render: RenderHandler,
        life: LifeSpanHandler,
        load: LoadHandler,
        display: DisplayHandler,
        dialog: DialogHandler,
        request: RequestHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }

        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(self.dialog.clone())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.request.clone())
        }
    }
}

// A windowless browser must have a render handler that names a view
// rectangle; nothing is done with what it paints.
wrap_render_handler! {
    struct PageRender;

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                rect.x = 0;
                rect.y = 0;
                rect.width = 16;
                rect.height = 16;
            }
        }
    }
}

wrap_life_span_handler! {
    struct PageLife {
        delegate: Rc<DelegateState>,
        bridge: Rc<Bridge>,
        key: String,
        generation: u64,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            self.delegate.add_live(1);
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            self.delegate.add_live(-1);
            // Closed by the engine, the page is already gone and this is
            // a no-op; closed underneath (an extension reload), it goes.
            self.bridge.page_gone(&self.key, self.generation, "it closed", false);
        }
    }
}

// A renderer that terminates takes the page's script with it; the browser
// itself stays until it is closed, so the page is closed too.
wrap_request_handler! {
    struct PageRequest {
        bridge: Rc<Bridge>,
        key: String,
        generation: u64,
    }

    impl RequestHandler {
        fn on_render_process_terminated(
            &self,
            _browser: Option<&mut Browser>,
            _status: TerminationStatus,
            _error_code: ::std::os::raw::c_int,
            _error_string: Option<&CefString>,
        ) {
            self.bridge
                .page_gone(&self.key, self.generation, "its renderer terminated", true);
        }
    }
}

wrap_load_handler! {
    struct PageLoad {
        bridge: Rc<Bridge>,
        key: String,
        generation: u64,
    }

    impl LoadHandler {
        fn on_load_end(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _http_status_code: ::std::os::raw::c_int,
        ) {
            if frame.is_some_and(|f| f.is_main() != 0) {
                self.bridge.page_ready(&self.key, self.generation);
            }
        }
    }
}

wrap_display_handler! {
    struct PageDisplay {
        key: String,
    }

    impl DisplayHandler {
        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            _level: LogSeverity,
            message: Option<&CefString>,
            _source: Option<&CefString>,
            _line: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            log::debug!(
                "bridge: page {} says: {}",
                self.key,
                message.map(CefString::to_string).unwrap_or_default()
            );
            1
        }
    }
}

wrap_dev_tools_message_observer! {
    struct PageDevTools {
        bridge: Rc<Bridge>,
        key: String,
        generation: u64,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            self.bridge.page_result(
                &self.key,
                self.generation,
                message_id,
                success != 0,
                result.unwrap_or_default(),
            );
        }

        /// The page's DevTools session ended: the results still pending
        /// will never arrive (CEF's header says so), so they are dropped
        /// and what was waiting for them is asked again.
        fn on_dev_tools_agent_detached(&self, _browser: Option<&mut Browser>) {
            self.bridge.page_detached(&self.key, self.generation);
        }
    }
}

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
    Inspect { label: String },
    /// A relay's `next()`: the worker's requests, when it has some.
    Poll,
    /// An answer handed to a relay; only a failure is of interest.
    Reply,
    /// A delivery to an extension's worker.
    Deliver {
        tab: TabId,
        attempt: u8,
        delivery: u64,
    },
    /// An event for an extension's worker (`Job::Emit`), kept to retry it.
    Emit {
        event: String,
        args: Vec<Value>,
        attempt: u8,
        delivery: u64,
        instance: Option<String>,
    },
}

/// Work that waits for a fresh table, or for its relay to be ready.
enum Job {
    /// Deliver a click on `extension`'s action for `tab`. `delivery` is
    /// the id the worker will see, kept for every attempt so a worker
    /// instance that already ran it ignores the repeat.
    Click {
        extension: String,
        tab: TabId,
        attempt: u8,
        delivery: u64,
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
    /// Deliver `event` with `args` to `extension`'s worker: a listener's
    /// event, or `bridge.wake`. `delivery` is its id, the same on every
    /// attempt (a worker instance that ran it ignores the repeat).
    /// `instance` is the worker instance the extension was known to have
    /// when the event was queued (`None`: before any hello).
    Emit {
        extension: String,
        event: String,
        args: Vec<Value>,
        attempt: u8,
        delivery: u64,
        instance: Option<String>,
    },
}

/// `Job::Emit` (an event for an extension's worker) sits in the queue with
/// the fields of its delivery; the kind is what the queue's rules read.
impl Job {
    fn extension(&self) -> &str {
        match self {
            Job::Click { extension, .. }
            | Job::Debugger { extension, .. }
            | Job::Emit { extension, .. } => extension,
        }
    }

    fn kind(&self) -> JobKind {
        match self {
            Job::Click { .. } => JobKind::Click,
            Job::Debugger { .. } => JobKind::Debugger,
            Job::Emit { .. } => JobKind::Emit,
        }
    }
}

/// An evaluation whose call is in progress: its pending entry is reserved
/// before the call, because CEF can answer inside the call, before it has
/// returned the message id. Such an answer is matched here by `key`.
struct Sending {
    token: u64,
    key: String,
    pending: Option<Pending>,
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

/// The engine's ids for worker calls. A worker numbers its calls itself, so
/// two extensions' workers can both have a call 7, and a worker instance
/// that is replaced leaves calls whose answers its successor would ignore.
/// The hub is given a Bridge-local id per call (`issue`); this map gives back
/// the worker's (extension, instance, call) when the answer comes (`take`).
#[derive(Default)]
pub(crate) struct CallIds {
    last: CallId,
    calls: HashMap<CallId, (String, String, u64)>,
}

impl CallIds {
    /// The Bridge-local id of call `worker_call` of `instance` of `extension`.
    pub(crate) fn issue(&mut self, extension: &str, instance: &str, worker_call: u64) -> CallId {
        self.last += 1;
        self.calls.insert(
            self.last,
            (extension.to_string(), instance.to_string(), worker_call),
        );
        self.last
    }

    /// The triple of `call`, removed: an answer is sent once.
    pub(crate) fn take(&mut self, call: CallId) -> Option<(String, String, u64)> {
        self.calls.remove(&call)
    }

    /// Drop the calls of `extension`'s instances other than `current`, the
    /// instance a hello has just named. Returns how many were dropped.
    pub(crate) fn retire_instances(&mut self, extension: &str, current: &str) -> usize {
        let before = self.calls.len();
        self.calls
            .retain(|_, (e, instance, _)| e != extension || instance == current);
        before - self.calls.len()
    }
}

#[cfg(test)]
mod call_ids_tests {
    use super::CallIds;

    #[test]
    fn calls_of_two_workers_with_the_same_number_stay_apart() {
        let mut ids = CallIds::default();
        let a = ids.issue("aaaa", "i1", 7);
        let b = ids.issue("bbbb", "i2", 7);
        assert_ne!(a, b);
        assert_eq!(ids.take(b), Some(("bbbb".to_string(), "i2".to_string(), 7)));
        assert_eq!(ids.take(a), Some(("aaaa".to_string(), "i1".to_string(), 7)));
        assert_eq!(ids.take(a), None, "an answer is sent once");
    }

    #[test]
    fn a_new_worker_instance_retires_the_calls_of_the_old_one_and_no_others() {
        let mut ids = CallIds::default();
        let old_one = ids.issue("aaaa", "old", 1);
        let old_two = ids.issue("aaaa", "old", 2);
        let other = ids.issue("bbbb", "old", 3);
        let same = ids.issue("aaaa", "new", 4);
        assert_eq!(
            ids.retire_instances("aaaa", "new"),
            2,
            "the two calls of the old instance go"
        );
        assert_eq!(ids.take(old_one), None);
        assert_eq!(ids.take(old_two), None);
        assert_eq!(
            ids.take(other),
            Some(("bbbb".to_string(), "old".to_string(), 3)),
            "another extension's calls stay"
        );
        assert_eq!(
            ids.take(same),
            Some(("aaaa".to_string(), "new".to_string(), 4)),
            "the current instance's calls stay"
        );
        assert_eq!(
            ids.retire_instances("aaaa", "new"),
            0,
            "a hello of the same instance retires nothing"
        );
    }
}

#[derive(Default)]
struct Inner {
    table: TabTable,
    /// Each tracked tab's browser and view, until the tab closes.
    tabs: HashMap<TabId, TabRecord>,
    /// Which extensions are debugging which tab (`bridge::debugger`).
    debuggers: Debuggers,
    helper: Option<HiddenPage>,
    /// The relay page of each bridged, enabled extension, by its id.
    relays: HashMap<String, HiddenPage>,
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
    /// Evaluations whose call has not returned yet.
    sending: Vec<Sending>,
    next_token: u64,
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
    delegate: Rc<DelegateState>,
    inner: RefCell<Inner>,
}

/// Whether the page under `key` is the one made under `generation`.
fn is_current(inner: &Inner, key: &str, generation: u64) -> bool {
    is_generation(page_ref(inner, key).map(|page| page.generation), generation)
}

fn page_ref<'a>(inner: &'a Inner, key: &str) -> Option<&'a HiddenPage> {
    if key == HELPER_KEY {
        inner.helper.as_ref()
    } else {
        inner.relays.get(key)
    }
}

/// Remove the page under `key` if it has `generation`.
fn take_page(inner: &mut Inner, key: &str, generation: u64) -> Option<HiddenPage> {
    if key == HELPER_KEY {
        return match &inner.helper {
            Some(page) if page.generation == generation => inner.helper.take(),
            _ => None,
        };
    }
    match inner.relays.get(key) {
        Some(page) if page.generation == generation => inner.relays.remove(key),
        _ => None,
    }
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
        host.was_hidden(i32::from(view.wants_hidden()));
        if debugged {
            host.invalidate(cef::PaintElementType::VIEW);
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
                next_token: 1,
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
        let probe = Rc::new(Probe::default());
        // Reserved before the call: CEF can answer inside it (see `Probe`).
        probe.sent.set(RESERVED);
        let mut observer = TabProbe::new(self.clone(), tab, probe.clone());
        let registration = host.add_dev_tools_message_observer(Some(&mut observer));
        let id = host.execute_dev_tools_method(0, Some(&"Target.getTargetInfo".into()), None);
        if id == 0 {
            probe.sent.set(NOT_SENT);
            probe.early.borrow_mut().clear();
            log::warn!("bridge: cannot ask {tab:?} for its target id");
        } else {
            probe.sent.set(id);
            let early: Vec<(i32, bool, Vec<u8>)> = probe.early.borrow_mut().drain(..).collect();
            for (message, success, result) in early {
                if message == id {
                    self.target_known(tab, success, &result);
                }
            }
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
            let open: Vec<String> = inner.relays.keys().cloned().collect();
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
            let close_helper = bridged.is_empty() && inner.helper.is_some();
            (close_helper, to_close)
        };
        if close_helper {
            log::info!("bridge: no bridged extension is on; the helper's page closes");
            let page = {
                let mut inner = self.inner.borrow_mut();
                drop_pending(&mut inner, HELPER_KEY, "the helper's page closed");
                inner.refreshing = false;
                inner.helper.take()
            };
            if let Some(page) = page {
                page.close();
            }
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
            let open: Vec<String> = inner.relays.keys().cloned().collect();
            (
                inner.helper_loaded,
                inner.wanted.clone(),
                inner.helper.is_some(),
                open,
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
        if helper_wanted && self.opening_due(HELPER_KEY, now) {
            let url = format!("chrome-extension://{HELPER_ID}/helper.html");
            let generation = self.next_generation();
            let page = HiddenPage::open(self, HELPER_KEY, &url, generation);
            if page.is_none() {
                log::error!("bridge: Chromium could not create the helper's page");
            }
            self.inner.borrow_mut().helper = page;
        }
        for id in relays_wanted {
            if self.opening_due(&id, now) {
                self.open_relay(&id);
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
        if inner.relays.is_empty() {
            return false;
        }
        let since = inner
            .last_refresh
            .map(|at| now.saturating_duration_since(at));
        if check_due(since, REFRESH_INTERVAL) {
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
                let relay_open = inner.relays.contains_key(&extension);
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
    /// ask wakes nothing that an idle stop had put to sleep (I15). The answer
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
            // is a failed opening too (I12): it counts until its worker says who
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

    fn open_relay(self: &Rc<Self>, extension: &str) {
        let url = format!("chrome-extension://{extension}{RELAY_PATH}");
        let generation = self.next_generation();
        match HiddenPage::open(self, extension, &url, generation) {
            Some(page) => {
                self.inner
                    .borrow_mut()
                    .relays
                    .insert(extension.to_string(), page);
            }
            None => log::error!("bridge: Chromium could not create the relay of {extension}"),
        }
    }

    /// Close `extension`'s relay (or the helper's page, under `HELPER_KEY`) on
    /// purpose. Its pending evaluations go with it. Its debuggers are kept: a
    /// retry opens a fresh relay for a worker that may still be debugging.
    fn close_relay(&self, extension: &str) {
        let page = {
            let mut inner = self.inner.borrow_mut();
            let page = if extension == HELPER_KEY {
                inner.refreshing = false;
                inner.helper.take()
            } else {
                inner.relays.remove(extension)
            };
            drop_pending(&mut inner, extension, "its page was closed");
            page
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
    /// again, in the words the detach path uses (`set_debugged`): the driver's
    /// checks read that line, so it is logged only when the tab was hidden.
    fn hide_again(&self, tab: TabId, handles: Option<TabHandles>, chromium: Option<i64>) {
        let Some((browser, view)) = handles else {
            return;
        };
        if set_tab_debugged(&browser, &view, false) {
            let chromium = chromium.map_or_else(|| "?".to_string(), |id| id.to_string());
            log::info!(
                "bridge: {tab:?} (Chromium tab {chromium}) hidden again: no debugger is left on it"
            );
        }
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
            let mut pages: Vec<HiddenPage> = inner.helper.take().into_iter().collect();
            pages.extend(inner.relays.drain().map(|(_, page)| page));
            (pages, tabs)
        };
        // The tabs' records and the pages are let go with no borrow held.
        drop(tabs);
        for page in pages {
            page.close();
        }
    }

    /// Evaluate `expression` in the page under `key`; the pending entry is
    /// reserved before the call and settled after it, so an answer that
    /// CEF gives inside the call (before the call returns its id) is still
    /// matched (`page_result`). The state is not borrowed across the call.
    fn evaluate(&self, key: &str, expression: &str, pending: Pending) -> bool {
        let browser = {
            let inner = self.inner.borrow();
            page_ref(&inner, key).map(|page| page.browser.clone())
        };
        let Some(browser) = browser else {
            return false;
        };
        let token = self.reserve(key, pending);
        let id = HiddenPage::evaluate(&browser, expression);
        self.settle(token, key, id)
    }

    fn reserve(&self, key: &str, pending: Pending) -> u64 {
        let mut inner = self.inner.borrow_mut();
        let token = inner.next_token;
        inner.next_token += 1;
        inner.sending.push(Sending {
            token,
            key: key.to_string(),
            pending: Some(pending),
        });
        token
    }

    /// The call returned `id`: the reservation becomes a pending entry under
    /// it, unless the answer already came (and took the entry). A failed call
    /// leaves nothing pending.
    fn settle(&self, token: u64, key: &str, id: Option<i32>) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(at) = inner.sending.iter().position(|s| s.token == token) else {
            return id.is_some();
        };
        let sent = inner.sending.remove(at);
        match id {
            Some(id) => {
                if let Some(pending) = sent.pending {
                    inner.pending.insert((key.to_string(), id), pending);
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
    fn page_ready(self: &Rc<Self>, key: &str, generation: u64) {
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
    /// jobs that need it fail now (`policy::failed_by_loss`): a debugger
    /// report does not, since it needs the table, and waits for the next
    /// join. A wanted relay is opened again by the next turn (`reopen`),
    /// never from this callback. `close` says the page still has to be
    /// closed by us.
    fn page_gone(self: &Rc<Self>, key: &str, generation: u64, why: &str, close: bool) {
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
            let jobs = std::mem::take(&mut inner.jobs);
            let fail = {
                let kinds: Vec<(JobKind, &str)> = jobs
                    .iter()
                    .map(|job| (job.kind(), job.extension()))
                    .collect();
                failed_by_loss(key, is_helper, &kinds)
            };
            let (mut stale, mut kept) = (Vec::new(), Vec::new());
            for (job, failed) in jobs.into_iter().zip(fail) {
                if failed {
                    stale.push(job);
                } else {
                    kept.push(job);
                }
            }
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
    fn page_detached(self: &Rc<Self>, key: &str, generation: u64) {
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
        if self.given_up(extension) || self.given_up(HELPER_KEY) {
            log::warn!("bridge: the click on {extension} is refused: its page did not load");
            return false;
        }
        {
            let mut inner = self.inner.borrow_mut();
            // A wanted relay that closed underneath is reopened on the next
            // turn, and its click waits for it (`job_turn`).
            if !inner.wanted.iter().any(|id| id == extension) {
                return false;
            }
            let delivery = inner.next_delivery;
            inner.next_delivery += 1;
            drop(inner);
            self.queue(Job::Click {
                extension: extension.to_string(),
                tab,
                attempt: 0,
                delivery,
            });
        }
        self.refresh();
        true
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
            self.reply(extension, instance, call, &json!({"result": null}));
            return;
        };
        let known = self.inner.borrow().table.resolve(&debuggee);
        match known {
            Some(tab) => {
                self.set_debugged(extension, tab, attach);
                self.reply(extension, instance, call, &json!({"result": null}));
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
        // The Chromium id is in the log so that a run can name the tab a
        // debugger was on by the id its extension knows it by.
        let chromium = chromium.map_or_else(|| "?".to_string(), |id| id.to_string());
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
                        "bridge: {tab:?} (Chromium tab {chromium}) was hidden; shown for the debugger of {extension}"
                    );
                }
            }
            Change::Undebugged => {
                log::info!("bridge: {extension} no longer debugs {tab:?}");
                if let Some((browser, view)) = handles
                    && set_tab_debugged(&browser, &view, false)
                {
                    log::info!(
                        "bridge: {tab:?} (Chromium tab {chromium}) hidden again: no debugger is left on it"
                    );
                }
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
            if let Job::Emit {
                event, delivery, ..
            } = job
            {
                log::warn!(
                    "bridge: {extension}: event {event} (delivery {delivery}) is dropped: its worker instance was replaced"
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
    /// opening is a failed one for the schedule (I12). Its relay is closed by
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
                Job::Emit {
                    extension,
                    event,
                    delivery,
                    ..
                } => {
                    log::warn!(
                        "bridge: {extension}: event {event} (delivery {delivery}) is dropped: {why}"
                    );
                }
                Job::Click {
                    extension,
                    tab,
                    delivery,
                    ..
                } => {
                    log::warn!(
                        "bridge: {extension}: the click on {tab:?} (delivery {delivery}) is dropped: {why}"
                    );
                }
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
                    self.reply(&extension, &instance, call, &json!({"result": null}));
                }
            }
        }
    }

    fn run(&self, job: Job) {
        match job {
            Job::Emit {
                extension,
                event,
                args,
                attempt,
                delivery,
                instance,
            } => {
                let message = json!({"id": delivery, "event": event, "args": args});
                let expression = format!("nautilusRelay.deliver({message})");
                let pending = Pending::Emit {
                    event: event.clone(),
                    args,
                    attempt,
                    delivery,
                    instance,
                };
                if self.evaluate(&extension, &expression, pending) {
                    log::debug!("bridge: {extension}: event {event} (delivery {delivery}) sent");
                } else {
                    log::warn!("bridge: cannot reach the relay of {extension} for {event}");
                }
            }
            Job::Click {
                extension,
                tab,
                attempt,
                delivery,
            } => {
                let chromium = self.inner.borrow().table.chromium(tab);
                let Some(chromium) = chromium else {
                    log::warn!(
                        "bridge: no Chromium tab id for {tab:?}; the click on {extension} is dropped"
                    );
                    return;
                };
                let message =
                    json!({"id": delivery, "event": "action.onClicked", "tabId": chromium});
                let expression = format!("nautilusRelay.deliver({message})");
                let pending = Pending::Deliver {
                    tab,
                    attempt,
                    delivery,
                };
                if self.evaluate(&extension, &expression, pending) {
                    log::info!(
                        "bridge: {extension}: delivery {delivery} (action.onClicked, tab {chromium}) sent"
                    );
                } else {
                    log::warn!("bridge: cannot reach the relay of {extension}");
                }
            }
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
                self.reply(&extension, &instance, call, &json!({"result": null}));
            }
        }
    }

    /// An evaluation in a hidden page answered.
    fn page_result(
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
            match inner.pending.remove(&(key.to_string(), id)) {
                Some(pending) => Some(pending),
                // The answer came while its call was still running, before
                // the call returned the id: it is the reservation's.
                None => inner
                    .sending
                    .iter_mut()
                    .rev()
                    .find(|s| s.key == key && s.pending.is_some())
                    .and_then(|s| s.pending.take()),
            }
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
                            // (I15). A reload is seen by the worker target
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
            Pending::Deliver {
                tab,
                attempt,
                delivery,
            } => match after_delivery(&value, attempt) {
                Next::Finished => {
                    let attempts = value
                        .as_ref()
                        .ok()
                        .and_then(|v| v.get("attempts"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    log::info!(
                        "bridge: {key}: delivery {delivery} taken by the worker after {attempts} attempt(s)"
                    );
                }
                Next::Retry => {
                    log::warn!(
                        "bridge: {key}: delivery {delivery} was not acknowledged ({}); trying once more with a fresh relay",
                        describe(&value)
                    );
                    // Closed here, reopened by the next turn (`reopen`): a
                    // callback creates no browser.
                    self.close_relay(key);
                    self.queue(Job::Click {
                        extension: key.to_string(),
                        tab,
                        attempt: attempt + 1,
                        delivery,
                    });
                }
                Next::Abandon { reopen } => {
                    log::warn!(
                        "bridge: {key}: delivery {delivery} dropped: {}",
                        describe(&value)
                    );
                    if reopen {
                        self.close_relay(key);
                    }
                }
            },
            Pending::Emit {
                event,
                args,
                attempt,
                delivery,
                instance,
            } => match after_delivery(&value, attempt) {
                Next::Finished => {
                    log::debug!(
                        "bridge: {key}: event {event} (delivery {delivery}) taken by the worker"
                    );
                }
                Next::Retry => {
                    log::warn!(
                        "bridge: {key}: event {event} (delivery {delivery}) was not acknowledged ({}); trying once more with a fresh relay",
                        describe(&value)
                    );
                    // Closed here, reopened by the next turn (`reopen`): a
                    // callback creates no browser. The id is kept, so an
                    // instance that ran the event ignores the repeat.
                    self.close_relay(key);
                    self.queue(Job::Emit {
                        extension: key.to_string(),
                        event,
                        args,
                        attempt: attempt + 1,
                        delivery,
                        instance,
                    });
                }
                Next::Abandon { reopen } => {
                    log::warn!(
                        "bridge: {key}: event {event} (delivery {delivery}) dropped: {}",
                        describe(&value)
                    );
                    if reopen {
                        self.close_relay(key);
                    }
                }
            },
        }
    }

    /// Evaluate `expression` in `extension`'s relay page and log its value as
    /// `label` (a test control's inspection, `TEST HOOK: relay_eval`). Nothing
    /// else reads the answer.
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
        if self.given_up(extension) || self.given_up(HELPER_KEY) {
            log::warn!("bridge: {event} for {extension} is dropped: its page did not load");
            return;
        }
        let (delivery, instance) = {
            let mut inner = self.inner.borrow_mut();
            if !inner.wanted.iter().any(|id| id == extension) {
                log::debug!(
                    "bridge: {event} for {extension} is not delivered: it is not bridged and on"
                );
                return;
            }
            let delivery = inner.next_delivery;
            inner.next_delivery += 1;
            (delivery, inner.identities.get(extension).cloned())
        };
        self.queue(Job::Emit {
            extension: extension.to_string(),
            event: event.to_string(),
            args: args.to_vec(),
            attempt: 0,
            delivery,
            instance,
        });
        self.refresh();
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
        Job::Emit {
            extension: e,
            instance: Some(instance),
            ..
        } => e.as_str() == extension && !job_kept(JobKind::Emit, instance, current),
        _ => false,
    }
}

/// Log a pending evaluation that is lost with its page: a delivery or an
/// event among them is unknown to the engine from now on.
fn log_lost(key: &str, pending: &Pending, why: &str) {
    match pending {
        Pending::Deliver { delivery, .. } => {
            log::warn!("bridge: {key}: delivery {delivery} is unknown ({why})");
        }
        Pending::Emit {
            event, delivery, ..
        } => {
            log::warn!("bridge: {key}: event {event} (delivery {delivery}) is unknown ({why})");
        }
        _ => {}
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
    // An evaluation still inside its call is dropped with its page: its
    // answer, if any, is for a page that is gone.
    for sent in inner.sending.iter_mut().filter(|s| s.key == key) {
        if let Some(pending) = &sent.pending {
            log_lost(key, pending, why);
        }
        sent.pending = None;
    }
}

/// A delivery's outcome as the log shows it.
fn describe(value: &Result<Value, String>) -> String {
    match value {
        Ok(value) => value.to_string(),
        Err(e) => e.clone(),
    }
}

/// One tab's probe for its target id: the message id of the one
/// `Target.getTargetInfo` it sends (`sent`). Until the call returns, that id
/// is `RESERVED`, and CEF's answers in the window are kept (`early`); they
/// count only once the call has returned the id they carry. So the probe
/// acts on its own result and on no other method's result.
#[derive(Default)]
struct Probe {
    sent: Cell<i32>,
    early: RefCell<Vec<(i32, bool, Vec<u8>)>>,
}

wrap_dev_tools_message_observer! {
    struct TabProbe {
        bridge: Rc<Bridge>,
        tab: TabId,
        probe: Rc<Probe>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            let result = result.unwrap_or_default();
            match self.probe.sent.get() {
                RESERVED => self
                    .probe
                    .early
                    .borrow_mut()
                    .push((message_id, success != 0, result.to_vec())),
                sent if sent == message_id => {
                    self.bridge.target_known(self.tab, success != 0, result);
                }
                _ => {}
            }
        }
    }
}
