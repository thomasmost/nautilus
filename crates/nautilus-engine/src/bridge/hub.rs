//! The bridge's CEF-facing half: it carries out what `BridgeState`
//! decides. Calls arrive from two lanes (the relay, for service workers;
//! the page lane, for extension pages), answers go back down the lane a
//! call came on, events go down both, requests become `EngineEvent`s,
//! and a screenshot is taken through the tab's own DevTools agent.
//!
//! Nothing here holds a `RefCell` borrow while it performs an effect: every
//! effect runs after the state that decided it is released, because an
//! effect can reach CEF, and CEF can call back into the hub (an evaluation
//! answered inside its call reaches `page_result`, which calls `call`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, DevToolsMessageObserver, ImplBrowser, ImplBrowserHost,
    ImplDevToolsMessageObserver, ImplDictionaryValue, Registration, WrapDevToolsMessageObserver,
    wrap_dev_tools_message_observer,
};
use osmena_core::{EngineEvent, ExtensionReply, ExtensionRequest, TabId, WindowId};
use osmena_engine::WindowStrip;
use serde_json::Value;

use super::ids::{self, IdSpace};
use super::pages::PageLane;
use super::pattern::Grants;
use super::side_panel;
use super::snapshot::Snapshot;
use super::state::{BridgeState, Effect};
use super::table::{Debuggee, Report, debuggee_of};
use super::transport::Asker;

/// Records a page's report of its own Chromium tab id (`Hub::report_tab`).
pub(crate) type Reporter = Box<dyn Fn(TabId, i64, &str) -> Report>;
/// Applies a page's debugger report: the extension, the debuggee, and
/// whether it attached (`Bridge::page_debugger`).
pub(crate) type DebuggerReports = Box<dyn Fn(&str, Debuggee, bool)>;
use super::transport::{BridgeCall, BridgeTransport, CallId};
use crate::delegate::DelegateState;

/// The browser as `side_panel.rs` needs it: the published strip and the
/// id space extensions use.
pub(crate) struct StripWorld<'a> {
    pub snapshot: &'a Snapshot,
    pub ids: &'a IdSpace,
}

impl side_panel::World for StripWorld<'_> {
    fn tab(&self, id: i64) -> Option<side_panel::TabRef> {
        let tab = self.ids.tab(id)?;
        let window = self.window_of(tab)?;
        Some(side_panel::TabRef { window, tab })
    }

    fn tab_id(&self, tab: TabId) -> Option<i64> {
        self.ids.ext(tab)
    }

    fn window(&self, id: i64) -> Option<WindowId> {
        let window = ids::window_from(id)?;
        self.snapshot.window(window).map(|_| window)
    }

    fn window_id(&self, window: WindowId) -> i64 {
        ids::window_id(window)
    }

    fn window_of(&self, tab: TabId) -> Option<WindowId> {
        self.snapshot.window_of_view(tab).map(|w| w.id)
    }

    fn focused_window(&self) -> Option<WindowId> {
        self.snapshot
            .windows
            .iter()
            .find(|w| w.focused)
            .or_else(|| self.snapshot.windows.first())
            .map(|w| w.id)
    }

    fn shown_panel(&self, window: WindowId) -> Option<TabId> {
        self.snapshot.window(window)?.panel_view
    }
}

/// Which way a call came, and so which way its answer goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lane {
    /// The relay: a service worker.
    Worker,
    /// A process message from an extension page (`pages::PageLane`).
    Page,
}

/// The first DevTools message id the hub uses on a tab's browser; other
/// users of that channel stay far below it.
pub(super) const CAPTURE_IDS: i32 = 1_000_000;
const CAPTURE_FAILED: &str = "Failed to capture tab: the browser returned no image.";
/// How long a screenshot may take before it fails.
const CAPTURE_WAIT: Duration = Duration::from_secs(10);

/// Whether a screenshot is over: its browser is gone, or it has waited for
/// `CAPTURE_WAIT`.
pub(crate) fn capture_spent(started: Instant, now: Instant, browser_alive: bool) -> bool {
    !browser_alive || now.saturating_duration_since(started) >= CAPTURE_WAIT
}

/// The captures among `open` (each its id, its tab and when it was asked) that
/// are spent at `now`, given whether a tab's browser is still alive.
pub(crate) fn spent_captures(
    open: &[(i32, TabId, Instant)],
    now: Instant,
    alive: impl Fn(TabId) -> bool,
) -> Vec<i32> {
    open.iter()
        .filter(|(_, tab, started)| capture_spent(*started, now, alive(*tab)))
        .map(|(id, _, _)| *id)
        .collect()
}

/// A screenshot under way.
struct Capture {
    call: CallId,
    format: String,
    /// Keeps the observer registered until the result is in.
    observer: Registration,
    /// The tab whose browser takes the screenshot, and when it was asked.
    tab: TabId,
    started: Instant,
}

pub(crate) struct Hub {
    state: RefCell<BridgeState>,
    worker: Rc<dyn BridgeTransport>,
    pages: Option<Rc<PageLane>>,
    /// Calls not yet answered: the hub's id to the lane and the lane's id.
    routes: RefCell<HashMap<CallId, (Lane, CallId)>>,
    last_call: Cell<CallId>,
    delegate: Weak<DelegateState>,
    /// The tab-id table: Chromium's id for a tab, when it is known.
    table: Box<dyn Fn(TabId) -> Option<i64>>,
    /// Records a page's report of its own Chromium tab id in the table.
    reporter: Reporter,
    /// Records a page's report of its debugger in the set of debuggers.
    debugger: DebuggerReports,
    /// Asks for another `tick` in a moment.
    nudge: Box<dyn Fn()>,
    captures: RefCell<HashMap<i32, Capture>>,
    last_capture: Cell<i32>,
    /// Observers whose result has arrived, let go on the next tick rather
    /// than inside their own callback.
    spent: RefCell<Vec<Registration>>,
    /// The DevTools results that came in, answered on the next tick rather
    /// than inside the observer that received them: a reply evaluates in the
    /// relay, which must not run inside a DevTools callback.
    finished: RefCell<Vec<(i32, bool, Vec<u8>)>>,
}

impl Hub {
    pub(crate) fn new(
        delegate: &Rc<DelegateState>,
        worker: Rc<dyn BridgeTransport>,
        pages: Option<Rc<PageLane>>,
        table: Box<dyn Fn(TabId) -> Option<i64>>,
        reporter: Reporter,
        debugger: DebuggerReports,
        nudge: Box<dyn Fn()>,
    ) -> Rc<Hub> {
        Rc::new(Hub {
            state: RefCell::new(BridgeState::default()),
            worker,
            pages,
            routes: RefCell::new(HashMap::new()),
            last_call: Cell::new(0),
            delegate: Rc::downgrade(delegate),
            table,
            reporter,
            debugger,
            nudge,
            captures: RefCell::new(HashMap::new()),
            last_capture: Cell::new(CAPTURE_IDS),
            spent: RefCell::new(Vec::new()),
            finished: RefCell::new(Vec::new()),
        })
    }

    /// A bridged function was called. `call.id` is the lane's own id for
    /// it.
    pub(crate) fn call(self: &Rc<Self>, lane: Lane, mut call: BridgeCall) {
        let id = self.last_call.get() + 1;
        self.last_call.set(id);
        self.routes.borrow_mut().insert(id, (lane, call.id));
        call.id = id;
        log::debug!(
            "bridge: {} calls {}.{} ({lane:?}, {:?})",
            call.extension,
            call.namespace,
            call.method,
            call.asker
        );
        self.sync_panels();
        // A page's call is held where the state says (`page_call`); a
        // worker's is answered at once.
        let effects = {
            let mut state = self.state.borrow_mut();
            match lane {
                Lane::Page => state.page_call(call, Instant::now()),
                Lane::Worker => state.call(call),
            }
        };
        self.run(effects);
    }

    /// The strip's fields of `tab` for `extension` (`BridgeState::strip_of`),
    /// for the relay's click. `None` for a tab the published strip does not
    /// hold, and when the state is in use (the relay sends a click on its
    /// own turn, never from inside a call the hub is routing).
    pub(crate) fn strip_of(&self, extension: &str, tab: TabId) -> Option<Value> {
        self.state.try_borrow().ok()?.strip_of(extension, tab)
    }

    /// Give the state the open side panels' windows, from the registry:
    /// the router answers "the current window" of a panel's page from
    /// them (`View::current_window`), and a held call is routed inside the
    /// state, where the registry is out of reach.
    fn sync_panels(&self) {
        let Some(delegate) = self.delegate.upgrade() else {
            return;
        };
        let panels = delegate
            .panels
            .borrow()
            .all()
            .map(|panel| (panel.view, panel.window))
            .collect();
        self.state.borrow_mut().set_panel_windows(panels);
    }

    /// `chrome.sidePanel`, and the panel's part of `runtime.getContexts`
    /// (`Effect::SidePanel`): answered from the two stores in
    /// `DelegateState`, which `route` may change, so they are borrowed here
    /// and not by the router. The borrows end before anything is run.
    fn side_panel_call(&self, call: &BridgeCall) -> Vec<Effect> {
        let Some(delegate) = self.delegate.upgrade() else {
            return vec![Effect::Reply(
                call.id,
                Err("the browser is closing.".into()),
            )];
        };
        let asker_view = match &call.asker {
            Asker::View(view) => Some(*view),
            _ => None,
        };
        let routed = {
            let state = self.state.borrow();
            let world = StripWorld {
                snapshot: &state.published,
                ids: &state.ids,
            };
            let panels = delegate.panels.borrow();
            if call.namespace == "runtime" {
                let empty = serde_json::json!({});
                let filter = call.args.first().unwrap_or(&empty);
                let originals = call
                    .args
                    .get(1)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let contexts =
                    side_panel::get_contexts(&panels, &world, &call.extension, filter, originals);
                side_panel::Routed::answer(Value::Array(contexts))
            } else {
                // The window a panel's page is in is the registry's to say:
                // the strip names only the panel its window shows, and a
                // tab's panel whose tab is not the active one is not that.
                let origin_window =
                    asker_view
                        .and_then(|view| panels.window_of(view))
                        .or_else(|| {
                            state
                                .view(&call.extension)
                                .current_window(&call.asker)
                                .map(|window| window.id)
                        });
                side_panel::route(
                    &mut delegate.side_panels.borrow_mut(),
                    &panels,
                    &world,
                    &side_panel::Call {
                        extension: &call.extension,
                        origin_window,
                        origin_view: asker_view,
                        method: &call.method,
                        args: &call.args,
                    },
                )
            }
        };
        if routed.save {
            delegate.save_side_panel_behaviors();
        }
        let mut effects: Vec<Effect> = routed
            .also
            .into_iter()
            .map(|request| Effect::EngineRequest {
                extension: call.extension.clone(),
                request,
            })
            .collect();
        let outcome = routed.outcome.into_router();
        effects.extend(
            self.state
                .borrow_mut()
                .outcome_effects(call.id, &call.extension, outcome),
        );
        effects
    }

    /// A click on `extension`'s action with `tab` active: the request
    /// that opens (or closes) its side panel, when the extension asked for
    /// that with `setPanelBehavior` and has an active panel for the tab.
    /// `None`: the click is an `action.onClicked`.
    pub(crate) fn side_panel_click(&self, extension: &str, tab: TabId) -> Option<ExtensionRequest> {
        let delegate = self.delegate.upgrade()?;
        let state = self.state.borrow();
        let world = StripWorld {
            snapshot: &state.published,
            ids: &state.ids,
        };
        side_panel::action_click(
            &delegate.side_panels.borrow(),
            &delegate.panels.borrow(),
            &world,
            extension,
            tab,
        )
    }

    /// Drop the side panel options of tabs the published strip no longer
    /// holds: an extension's `setOptions({tabId})` outlives its tab
    /// otherwise, for the whole run. Called wherever a strip may have
    /// been published.
    fn forget_closed_tabs(&self) {
        let Some(delegate) = self.delegate.upgrade() else {
            return;
        };
        let state = self.state.borrow();
        let world = StripWorld {
            snapshot: &state.published,
            ids: &state.ids,
        };
        delegate
            .side_panels
            .borrow_mut()
            .keep_tabs(|tab| side_panel::World::tab(&world, tab).is_some());
    }

    /// Tell extensions about the panels that opened and closed
    /// (`chrome.sidePanel.onOpened`, `onClosed`): each notice goes to the
    /// contexts of its extension that listen for the event
    /// (`BridgeState::emit`).
    pub(crate) fn send_panel_notices(self: &Rc<Self>) {
        let Some(delegate) = self.delegate.upgrade() else {
            return;
        };
        let notices = delegate.take_panel_notices();
        if notices.is_empty() {
            return;
        }
        let effects: Vec<Effect> = {
            let state = self.state.borrow();
            let world = StripWorld {
                snapshot: &state.published,
                ids: &state.ids,
            };
            notices
                .iter()
                .flat_map(|notice| {
                    let (extension, event, info) = side_panel::notice_event(&world, notice);
                    state.emit(&extension, event, vec![info])
                })
                .collect()
        };
        self.run(effects);
    }

    /// `Engine::strip_changed`.
    pub(crate) fn strip_changed(self: &Rc<Self>, windows: &[WindowStrip<'_>]) {
        let snapshot = Snapshot::copy_of(windows);
        // Publishing can route the page calls that waited for it.
        self.sync_panels();
        let effects = self
            .state
            .borrow_mut()
            .strip_changed(snapshot, &*self.table, Instant::now());
        self.forget_closed_tabs();
        self.run(effects);
    }

    /// `Engine::extension_reply`.
    pub(crate) fn extension_reply(self: &Rc<Self>, token: u64, reply: ExtensionReply) {
        let effects = self
            .state
            .borrow_mut()
            .extension_reply(token, reply, Instant::now());
        self.run(effects);
    }

    /// A page of `tab` reported its own Chromium tab id. A recorded report is
    /// published at once, so the page's calls that follow see that id.
    pub(crate) fn report_tab(self: &Rc<Self>, tab: TabId, chromium: i64, extension: &str) {
        if (self.reporter)(tab, chromium, extension) == Report::Recorded {
            self.table_changed();
        }
    }

    /// The page of `extension`'s side panel in `view` reported its
    /// Chromium tab id: kept beside the tab table, and told to the
    /// extension's listening contexts (`BridgeState::panel_reported`).
    pub(crate) fn report_panel(self: &Rc<Self>, view: TabId, chromium: i64, extension: &str) {
        // The state hears of the open panels before it judges the view.
        self.sync_panels();
        let effects = self
            .state
            .borrow_mut()
            .panel_reported(view, extension, chromium);
        self.run(effects);
    }

    /// The table gained or lost a tab's Chromium id.
    pub(crate) fn table_changed(self: &Rc<Self>) {
        let effects = self
            .state
            .borrow_mut()
            .table_changed(&*self.table, Instant::now());
        self.forget_closed_tabs();
        self.run(effects);
    }

    /// The enabled extensions the bridge serves: each one's id, its grants,
    /// and whether it has a service worker (`BridgeState::set_extensions`).
    pub(crate) fn set_extensions(self: &Rc<Self>, extensions: Vec<(String, Grants, bool)>) {
        let effects = self.state.borrow_mut().set_extensions(extensions);
        self.run(effects);
    }

    /// What the router reads of each served extension, called beside
    /// `set_extensions` with the same extensions.
    pub(crate) fn set_facts(&self, facts: Vec<(String, super::ExtensionFacts)>) {
        self.state.borrow_mut().set_facts(facts);
    }

    /// `extension`'s keyboard command `name` for `tab`: its
    /// `commands.onCommand` listeners, in the worker (started if it was
    /// stopped) and in each page that listens.
    pub(crate) fn command(self: &Rc<Self>, extension: &str, name: &str, tab: TabId) {
        let effects = self.state.borrow().command(extension, name, tab);
        log::info!(
            "extension {extension}: command {name:?} for {tab:?}{}",
            if effects.is_empty() {
                ": no context listens for it"
            } else {
                ""
            }
        );
        self.run(effects);
    }

    /// Called on every spin of the engine.
    pub(crate) fn tick(self: &Rc<Self>) {
        // Taken out before they are dropped: letting go of a registration
        // is a CEF call, which is not made with a borrow held.
        let spent = std::mem::take(&mut *self.spent.borrow_mut());
        drop(spent);
        self.settle_captures(Instant::now());
        // A page whose frame is gone takes its listeners with it.
        if let Some(pages) = &self.pages {
            let gone = pages.gone();
            if !gone.is_empty() {
                self.state.borrow_mut().page_gone(&gone);
            }
        }
        if self.state.borrow().needs_tick() {
            // A held call may be routed in this tick.
            self.sync_panels();
        }
        let effects = {
            let mut state = self.state.borrow_mut();
            if !state.needs_tick() {
                return;
            }
            state.tick(&*self.table, Instant::now())
        };
        // A held strip may have been published in this tick.
        self.forget_closed_tabs();
        self.run(effects);
    }

    /// Let go of everything that holds a CEF object, before CEF shuts
    /// down.
    pub(crate) fn shutdown(&self) {
        let captures = std::mem::take(&mut *self.captures.borrow_mut());
        let spent = std::mem::take(&mut *self.spent.borrow_mut());
        self.routes.borrow_mut().clear();
        drop(captures);
        drop(spent);
    }

    /// Carry out `effects`. No borrow of the hub's state is held here.
    fn run(self: &Rc<Self>, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Reply(call, result) => self.reply(call, result),
                Effect::Emit {
                    extension,
                    event,
                    args,
                } => {
                    log::debug!("bridge: {event} to {extension}'s worker");
                    self.worker.emit(&extension, event, &args);
                }
                Effect::EmitPage {
                    page,
                    extension,
                    event,
                    args,
                } => {
                    log::debug!("bridge: {event} to {extension}'s page {page}");
                    if let Some(pages) = &self.pages {
                        pages.emit_to(page, &extension, event, &args);
                    }
                }
                Effect::Wake(extension) => self.worker.wake(&extension),
                Effect::Request {
                    token,
                    extension,
                    request,
                } => {
                    if let Some(delegate) = self.delegate.upgrade() {
                        log::debug!(
                            "bridge: {extension} asks the browser for {request:?} (token {token})"
                        );
                        // A panel this opens is expected by the time its
                        // view is asked for (`TabHost::create_view`).
                        delegate.note_request(token, &extension, &request);
                        delegate.push(EngineEvent::ExtensionRequest {
                            token,
                            extension,
                            request,
                        });
                    }
                }
                Effect::Capture {
                    call,
                    tab,
                    format,
                    quality,
                } => {
                    self.capture(call, tab, format, quality);
                }
                Effect::SidePanel(call) => {
                    let effects = self.side_panel_call(&call);
                    self.run(effects);
                }
                Effect::Debugger(call) => {
                    // The report first, the answer after: the page's
                    // `attach` resolves once the tab is shown.
                    match debuggee_of(call.args.first()) {
                        Some(debuggee) => {
                            (self.debugger)(&call.extension, debuggee, call.method == "attached");
                        }
                        None => log::warn!(
                            "bridge: {}: a page's debugger report names no tab or target",
                            call.extension
                        ),
                    }
                    self.reply(call.id, Ok(Value::Null));
                }
                Effect::EngineRequest { extension, request } => {
                    if let Some(delegate) = self.delegate.upgrade() {
                        log::debug!(
                            "bridge: the engine asks the browser for {request:?} on behalf of {extension}"
                        );
                        delegate.request_of_engine(&extension, request);
                    }
                }
            }
        }
        let wants_tick = self.state.borrow().needs_tick();
        if wants_tick {
            (self.nudge)();
        }
    }

    fn reply(&self, call: CallId, result: Result<Value, String>) {
        let removed = self.routes.borrow_mut().remove(&call);
        let Some((lane, local)) = removed else {
            return;
        };
        if let Err(message) = &result {
            log::debug!("bridge: call {call} rejects: {message}");
        }
        match lane {
            Lane::Worker => self.worker.reply(local, result),
            Lane::Page => {
                if let Some(pages) = &self.pages {
                    pages.reply(local, result);
                }
            }
        }
    }

    /// `tabs.captureVisibleTab`: `Page.captureScreenshot` on the tab's own
    /// browser, whose result comes back to `captured`.
    fn capture(self: &Rc<Self>, call: CallId, tab: TabId, format: String, quality: i64) {
        let host = self
            .delegate
            .upgrade()
            .and_then(|delegate| delegate.browser_of(tab))
            .and_then(|browser| browser.host());
        let (Some(host), Some(mut params)) = (host, cef::dictionary_value_create()) else {
            self.reply(call, Err(CAPTURE_FAILED.to_string()));
            return;
        };
        params.set_string(
            Some(&"format".into()),
            Some(&CefString::from(format.as_str())),
        );
        if format == "jpeg" {
            params.set_int(Some(&"quality".into()), quality as i32);
        }
        let mut observer = CaptureObserver::new(Rc::downgrade(self));
        let Some(observer) = host.add_dev_tools_message_observer(Some(&mut observer)) else {
            self.reply(call, Err(CAPTURE_FAILED.to_string()));
            return;
        };
        let id = self.last_capture.get() + 1;
        self.last_capture.set(id);
        self.captures.borrow_mut().insert(
            id,
            Capture {
                call,
                format,
                observer,
                tab,
                started: Instant::now(),
            },
        );
        if host.execute_dev_tools_method(
            id,
            Some(&"Page.captureScreenshot".into()),
            Some(&mut params),
        ) == 0
        {
            self.captured(id, false, &[]);
        }
        (self.nudge)();
    }

    /// Answers the DevTools results that came in, and fails the screenshots
    /// whose browser is gone or that have waited too long (`capture_spent`).
    /// While any screenshot is pending, the next tick is asked for.
    fn settle_captures(&self, now: Instant) {
        let finished = std::mem::take(&mut *self.finished.borrow_mut());
        for (id, success, result) in finished {
            self.captured(id, success, &result);
        }
        let open: Vec<(i32, TabId, Instant)> = self
            .captures
            .borrow()
            .iter()
            .map(|(id, capture)| (*id, capture.tab, capture.started))
            .collect();
        for id in spent_captures(&open, now, |tab| self.browser_alive(tab)) {
            self.captured(id, false, &[]);
        }
        if !self.captures.borrow().is_empty() {
            (self.nudge)();
        }
    }

    /// Whether `tab` still has a browser (its screenshot can be taken).
    fn browser_alive(&self, tab: TabId) -> bool {
        self.delegate
            .upgrade()
            .and_then(|delegate| delegate.browser_of(tab))
            .is_some()
    }

    fn captured(&self, id: i32, success: bool, result: &[u8]) {
        let removed = self.captures.borrow_mut().remove(&id);
        let Some(capture) = removed else {
            return;
        };
        let data = success
            .then(|| serde_json::from_slice::<Value>(result).ok())
            .flatten()
            .and_then(|value| {
                value
                    .get("data")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        let result = match data {
            Some(data) => Ok(Value::String(format!(
                "data:image/{};base64,{data}",
                capture.format
            ))),
            None => Err(CAPTURE_FAILED.to_string()),
        };
        self.spent.borrow_mut().push(capture.observer);
        self.reply(capture.call, result);
    }
}

wrap_dev_tools_message_observer! {
    struct CaptureObserver {
        hub: Weak<Hub>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            if let Some(hub) = self.hub.upgrade() {
                hub.finished.borrow_mut().push((
                    message_id,
                    success != 0,
                    result.unwrap_or_default().to_vec(),
                ));
                (hub.nudge)();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::side_panel::World as _;
    use super::*;
    use crate::bridge::snapshot::tests::{snapshot, tab, window};

    #[test]
    fn the_strip_and_the_ids_are_the_side_panels_world() {
        // Window 1 (not focused) holds tabs 1 and 2 and shows the panel view
        // 9; window 2 (focused) holds tab 3, which is deferred.
        let mut first = window(1, false, vec![tab(1, "a*"), tab(2, "b")]);
        first.panel_view = Some(TabId(9));
        let strip = snapshot(vec![first, window(2, true, vec![tab(3, "c~")])]);
        let mut ids = IdSpace::default();
        ids.sync(&[
            (TabId(1), Some(101)),
            (TabId(2), Some(102)),
            (TabId(3), None),
        ]);
        let world = StripWorld {
            snapshot: &strip,
            ids: &ids,
        };
        // A tab by the id extensions use, with its window.
        assert_eq!(
            world.tab(102),
            Some(side_panel::TabRef {
                window: WindowId(1),
                tab: TabId(2)
            })
        );
        assert_eq!(world.tab(777), None);
        assert_eq!(world.tab_id(TabId(1)), Some(101));
        // A deferred tab has a synthetic id, and is found by it.
        let synthetic = world.tab_id(TabId(3)).expect("a deferred tab has an id");
        assert_eq!(
            world.tab(synthetic).map(|found| found.window),
            Some(WindowId(2))
        );
        // A window exists only while the strip holds it.
        assert_eq!(world.window(ids::window_id(WindowId(2))), Some(WindowId(2)));
        assert_eq!(world.window(ids::window_id(WindowId(5))), None);
        assert_eq!(world.window(ids::WINDOW_ID_CURRENT), None);
        assert_eq!(
            world.window(world.window_id(WindowId(1))),
            Some(WindowId(1))
        );
        // A strip tab's window, and the window of the panel it shows.
        assert_eq!(world.window_of(TabId(2)), Some(WindowId(1)));
        assert_eq!(world.window_of(TabId(9)), Some(WindowId(1)));
        assert_eq!(world.window_of(TabId(8)), None);
        assert_eq!(world.focused_window(), Some(WindowId(2)));
        assert_eq!(world.shown_panel(WindowId(1)), Some(TabId(9)));
        assert_eq!(world.shown_panel(WindowId(2)), None);
        // With no window focused, the first stands in.
        let unfocused = snapshot(vec![window(4, false, vec![tab(1, "a*")])]);
        let world = StripWorld {
            snapshot: &unfocused,
            ids: &ids,
        };
        assert_eq!(world.focused_window(), Some(WindowId(4)));
    }

    #[test]
    fn a_capture_is_spent_when_its_browser_is_gone_or_its_wait_is_over() {
        let t0 = Instant::now();
        assert!(!capture_spent(t0, t0, true));
        assert!(capture_spent(t0, t0, false));
        assert!(!capture_spent(
            t0,
            t0 + CAPTURE_WAIT - Duration::from_millis(1),
            true
        ));
        assert!(capture_spent(t0, t0 + CAPTURE_WAIT, true));
    }

    #[test]
    fn the_spent_captures_are_the_ones_whose_browser_is_gone_or_whose_wait_is_over() {
        let t0 = Instant::now();
        let open = [
            (1, TabId(1), t0),
            (2, TabId(2), t0),
            (3, TabId(3), t0 + CAPTURE_WAIT - Duration::from_millis(1)),
            (4, TabId(4), t0),
        ];
        // Tab 2's browser is gone; captures 1 and 4 have waited out their time;
        // capture 3 is one millisecond short of it.
        let alive = |tab: TabId| tab != TabId(2);
        assert_eq!(
            spent_captures(&open, t0 + CAPTURE_WAIT, alive),
            vec![1, 2, 4]
        );
        assert_eq!(spent_captures(&open, t0, |_| true), Vec::<i32>::new());
        assert_eq!(spent_captures(&[], t0, |_| false), Vec::<i32>::new());
    }
}
