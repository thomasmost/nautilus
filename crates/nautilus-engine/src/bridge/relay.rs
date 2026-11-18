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
use super::install::HELPER_ID;
use super::policy::{
    JobKind, JobTurn, Next, Overflow, after_delivery, failed_by_loss, is_generation, job_turn,
    overflow, relay_changes,
};
use super::rewrite::BRIDGE_DIR;
use super::table::{Debuggee, TabTable, debuggee_of, parse_targets, query_rows, target_id_of};
use crate::delegate::{DelegateState, ViewState};

/// The key of the helper's page among the hidden pages; a relay's key
/// is its extension's id, which this can never be.
const HELPER_KEY: &str = "helper";

/// Waiting jobs beyond this are failed, oldest first, so a page that
/// never comes back cannot grow the queue without bound.
const MAX_JOBS: usize = 256;

/// The least time between two openings of hidden pages by `reopen`. A relay
/// that dies right after it opens (or cannot open at all) is therefore opened
/// again at most four times a second, while one that dies long after its
/// opening comes back on the next turn.
const REOPEN_GAP: std::time::Duration = std::time::Duration::from_millis(250);

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
}

/// Work that waits for a fresh table, or for its relay to be ready.
enum Job {
    /// Answer call `call` of `extension`'s worker with the tabs.
    Query { extension: String, call: u64 },
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
    /// `debuggee`; the worker waits for `call`.
    Debugger {
        extension: String,
        call: u64,
        debuggee: Debuggee,
        attach: bool,
    },
}

impl Job {
    fn extension(&self) -> &str {
        match self {
            Job::Query { extension, .. }
            | Job::Click { extension, .. }
            | Job::Debugger { extension, .. } => extension,
        }
    }

    fn kind(&self) -> JobKind {
        match self {
            Job::Query { .. } => JobKind::Query,
            Job::Click { .. } => JobKind::Click,
            Job::Debugger { .. } => JobKind::Debugger,
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
    helper: Option<HiddenPage>,
    /// The relay page of each bridged, enabled extension, by its id.
    relays: HashMap<String, HiddenPage>,
    /// The extensions the last `sync` wanted a relay for.
    wanted: Vec<String>,
    /// Whether the helper extension is loaded, as the last `sync` said.
    helper_loaded: bool,
    /// When `reopen` last opened a page (see `REOPEN_GAP`).
    last_open: Option<std::time::Instant>,
    /// Evaluations sent and not yet answered, by page key and the
    /// DevTools message id of that page's browser.
    pending: HashMap<(String, i32), Pending>,
    /// Evaluations whose call has not returned yet.
    sending: Vec<Sending>,
    next_token: u64,
    /// The helper has been asked for the targets and has not answered.
    refreshing: bool,
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
        let mut observer = TabProbe::new(self.clone(), tab);
        let registration = host.add_dev_tools_message_observer(Some(&mut observer));
        if host.execute_dev_tools_method(0, Some(&"Target.getTargetInfo".into()), None) == 0 {
            log::warn!("bridge: cannot ask {tab:?} for its target id");
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
                self.inner.borrow_mut().table.set_target(tab, target);
            }
            None => log::warn!("bridge: {tab:?} gave no target id: {parsed}"),
        }
    }

    /// A tab closed.
    pub(crate) fn untrack(&self, tab: TabId) {
        let mut inner = self.inner.borrow_mut();
        inner.tabs.remove(&tab);
        inner.table.forget(tab);
        inner.debuggers.forget_tab(tab);
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
            inner.wanted = bridged.to_vec();
            inner.helper_loaded = helper_loaded;
            let open: Vec<String> = inner.relays.keys().cloned().collect();
            let open: Vec<&str> = open.iter().map(String::as_str).collect();
            let (_, to_close) = relay_changes(&open, bridged);
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
            self.forget_debuggers(&id);
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
        let (helper_loaded, wanted, helper_open, open, last) = {
            let inner = self.inner.borrow();
            let open: Vec<String> = inner.relays.keys().cloned().collect();
            (
                inner.helper_loaded,
                inner.wanted.clone(),
                inner.helper.is_some(),
                open,
                inner.last_open,
            )
        };
        if wanted.is_empty() || last.is_some_and(|at| at.elapsed() < REOPEN_GAP) {
            return;
        }
        let (helper_wanted, relays_wanted) = {
            let open: Vec<&str> = open.iter().map(String::as_str).collect();
            (
                helper_loaded && !helper_open,
                relay_changes(&open, &wanted).0,
            )
        };
        if helper_wanted {
            let url = format!("chrome-extension://{HELPER_ID}/helper.html");
            let generation = self.next_generation();
            let page = HiddenPage::open(self, HELPER_KEY, &url, generation);
            if page.is_none() {
                log::error!("bridge: Chromium could not create the helper's page");
            }
            self.inner.borrow_mut().helper = page;
        }
        if helper_wanted || !relays_wanted.is_empty() {
            self.inner.borrow_mut().last_open = Some(std::time::Instant::now());
        }
        for id in relays_wanted {
            self.open_relay(&id);
        }
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
        let url = format!("chrome-extension://{extension}/{BRIDGE_DIR}/relay.html");
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

    /// Close `extension`'s relay on purpose. Its pending evaluations go
    /// with it. Its debuggers are kept: a retry opens a fresh relay for a
    /// worker that may still be debugging.
    fn close_relay(&self, extension: &str) {
        let page = {
            let mut inner = self.inner.borrow_mut();
            let page = inner.relays.remove(extension);
            drop_pending(&mut inner, extension, "its relay was closed");
            page
        };
        if let Some(page) = page {
            page.close();
        }
    }

    /// `extension`'s debuggers are gone with its relay: the tabs they
    /// debugged are shown or hidden accordingly.
    fn forget_debuggers(&self, extension: &str) {
        let freed: Vec<(TabId, Option<TabHandles>)> = {
            let mut inner = self.inner.borrow_mut();
            inner
                .debuggers
                .forget_extension(extension)
                .into_iter()
                .map(|tab| (tab, tab_handles(&inner, tab)))
                .collect()
        };
        for (tab, handles) in freed {
            log::info!("bridge: {tab:?} has no debugger left ({extension} is gone)");
            if let Some((browser, view)) = handles {
                set_tab_debugged(&browser, &view, false);
            }
        }
    }

    /// Close every hidden page; the engine pumps until they are gone.
    pub(crate) fn shutdown(&self) {
        // The pages are taken out before they are closed: closing one can
        // call back into this bridge (its DevTools session ends), which
        // must find the state free.
        let pages: Vec<HiddenPage> = {
            let mut guard = self.inner.borrow_mut();
            let inner = &mut *guard;
            inner.tabs.clear();
            inner.pending.clear();
            inner.jobs.clear();
            inner.wanted.clear();
            let mut pages: Vec<HiddenPage> = inner.helper.take().into_iter().collect();
            pages.extend(inner.relays.drain().map(|(_, page)| page));
            pages
        };
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
        if key != HELPER_KEY {
            self.poll(key);
        }
        if !self.inner.borrow().jobs.is_empty() {
            self.refresh();
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

    /// One request of `extension`'s worker, as the relay passed it on.
    fn worker_call(&self, extension: &str, item: &serde_json::Value) {
        let Some(call) = item.get("call") else {
            return;
        };
        let Some(id) = call.get("id").and_then(|v| v.as_u64()) else {
            return;
        };
        let method = call
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        log::debug!("bridge: {extension} calls {method} ({id})");
        match method {
            "tabs.query" => {
                self.queue(Job::Query {
                    extension: extension.to_string(),
                    call: id,
                });
                self.refresh();
            }
            "debugger.attached" | "debugger.detached" => {
                let debuggee = debuggee_of(call.get("args").and_then(|args| args.get(0)));
                self.debugger_report(extension, id, debuggee, method == "debugger.attached");
            }
            _ => {
                // Fail closed: a call the engine does not know is refused,
                // never passed to Chromium.
                let answer = json!({"error": format!("{method} is not supported by Nautilus.")});
                self.reply(extension, id, &answer);
            }
        }
    }

    /// The worker attached or detached a debugger to a tab. The worker's
    /// call is answered only after the tab is shown or hidden, so the
    /// debugger's first command never meets a hidden renderer.
    fn debugger_report(
        &self,
        extension: &str,
        call: u64,
        debuggee: Option<Debuggee>,
        attach: bool,
    ) {
        let Some(debuggee) = debuggee else {
            log::warn!("bridge: {extension}: a debugger report names no tab or target");
            self.reply(extension, call, &json!({"result": null}));
            return;
        };
        let known = self.inner.borrow().table.resolve(&debuggee);
        match known {
            Some(tab) => {
                self.set_debugged(extension, tab, attach);
                self.reply(extension, call, &json!({"result": null}));
            }
            // Not in the table yet: the next join decides it (`run`).
            None => {
                self.queue(Job::Debugger {
                    extension: extension.to_string(),
                    call,
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

    fn reply(&self, extension: &str, call: u64, answer: &serde_json::Value) {
        let expression = format!("nautilusRelay.reply({call}, {answer})");
        if !self.evaluate(extension, &expression, Pending::Reply) {
            log::warn!("bridge: cannot answer call {call} of {extension}");
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
                Job::Query { extension, call } => {
                    let answer = json!({"error": format!("the tab list is unavailable: {why}.")});
                    self.reply(&extension, call, &answer);
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
                    debuggee,
                    ..
                } => {
                    log::warn!(
                        "bridge: {extension}: the debugger of {debuggee:?} is not tracked: {why}"
                    );
                    self.reply(&extension, call, &json!({"result": null}));
                }
            }
        }
    }

    fn run(&self, job: Job) {
        match job {
            Job::Query { extension, call } => {
                let rows = {
                    let inner = self.inner.borrow();
                    query_rows(inner.table.rows(), self.delegate.last_focused())
                };
                self.reply(&extension, call, &json!({"result": rows}));
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
                self.reply(&extension, call, &json!({"result": null}));
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
                {
                    let mut inner = self.inner.borrow_mut();
                    inner.refreshing = false;
                    match &value {
                        Ok(value) => {
                            let targets = parse_targets(value);
                            let before = inner.table.rows().to_vec();
                            inner.table.join(&targets);
                            log::info!(
                                "bridge: table has {} tab(s) of {} target(s)",
                                inner.table.rows().len(),
                                targets.len()
                            );
                            // The rows name each tab by both its id and
                            // Chromium's, so a run's log can match them.
                            if before != inner.table.rows() {
                                for row in inner.table.rows() {
                                    log::info!(
                                        "bridge: table: {:?} is Chromium tab {} at {}",
                                        row.tab,
                                        row.chromium,
                                        row.url
                                    );
                                }
                            }
                        }
                        Err(e) => log::warn!("bridge: the helper could not list the targets: {e}"),
                    }
                }
                self.run_jobs();
            }
            Pending::Poll => match value {
                Ok(items) => {
                    for item in items.as_array().into_iter().flatten() {
                        self.worker_call(key, item);
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

/// Drop the pending evaluations of `key`; a delivery among them is lost
/// with its page, and the log says so.
fn drop_pending(inner: &mut Inner, key: &str, why: &str) {
    inner.pending.retain(|(k, _), pending| {
        if k != key {
            return true;
        }
        if let Pending::Deliver { delivery, .. } = pending {
            log::warn!("bridge: {key}: delivery {delivery} is unknown ({why})");
        }
        false
    });
    // An evaluation still inside its call is dropped with its page: its
    // answer, if any, is for a page that is gone.
    for sent in inner.sending.iter_mut().filter(|s| s.key == key) {
        if let Some(Pending::Deliver { delivery, .. }) = &sent.pending {
            log::warn!("bridge: {key}: delivery {delivery} is unknown ({why})");
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

wrap_dev_tools_message_observer! {
    struct TabProbe {
        bridge: Rc<Bridge>,
        tab: TabId,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            _message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            self.bridge
                .target_known(self.tab, success != 0, result.unwrap_or_default());
        }
    }
}
