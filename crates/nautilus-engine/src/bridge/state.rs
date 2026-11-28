//! What the bridge remembers between calls, with no CEF in it: the strip
//! as extensions were last told of it, the one waiting to be told, the
//! ids, each extension's grants and listeners, and the calls waiting on
//! the model. Every entry point returns the `Effect`s to carry out.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use osmena_core::{ExtensionReply, ExtensionRequest, TabId, WindowId};
use serde_json::{Value, json};

use super::differ;
use super::ids::IdSpace;
use super::pattern::Grants;
use super::router::{self, Outcome, Settled, Then};
use super::shape::View;
use super::snapshot::Snapshot;
use super::table::PanelIds;
use super::transport::{Asker, BridgeCall, CallId};
use super::{ExtensionFacts, NO_FACTS};

/// How long a strip waits for Chromium's id of a new live tab before it
/// is published with a synthetic one. A page's call waits for the same
/// bound, for the tab it is in or for its extension to be served.
pub(crate) const PUBLISH_WAIT: Duration = Duration::from_secs(2);

/// The error a page's call gets when its extension is still not bridged
/// when the call's wait is over.
pub(crate) const UNBRIDGED: &str = "the extension is not bridged.";

/// What a call waits for before the engine answers it: a page's, for its
/// tab or its extension; a worker's, for the tabs' real ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hold {
    /// A worker's call that reads or names tabs, made before the tabs
    /// have the ids Chromium knows them by (`BridgeState::worker_call`).
    Ids,
    /// The tab the page is in is not in the published strip, and a strip is
    /// still pending.
    Tab(TabId),
    /// The page's extension is not served yet.
    Served,
    /// An earlier held call must go first, so that calls keep their order.
    Queued,
}

/// A call held until its `Hold` is met or the bound has passed.
struct HeldCall {
    call: BridgeCall,
    hold: Hold,
    since: Instant,
}

/// What an extension the engine has not listed may see: nothing private.
static NO_GRANTS: Grants = Grants {
    tabs: false,
    all_urls: false,
    hosts: Vec::new(),
};

/// Something for the hub to do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    /// Settle a call.
    Reply(CallId, Result<Value, String>),
    /// Run `event`'s listeners in `extension`'s service worker.
    Emit {
        extension: String,
        event: &'static str,
        args: Vec<Value>,
    },
    /// As `Emit`, for the delivery a side panel's page waits on: the hub
    /// says when the worker has taken it, or that it could not be queued
    /// (`BridgeState::told` with `token`).
    EmitTold {
        extension: String,
        event: &'static str,
        args: Vec<Value>,
        token: u64,
    },
    /// Run `event`'s listeners in the page context `page` of `extension`.
    EmitPage {
        page: u64,
        extension: String,
        event: &'static str,
        args: Vec<Value>,
    },
    /// Start `extension`'s worker so it registers its listeners.
    Wake(String),
    /// Report `EngineEvent::ExtensionRequest`.
    Request {
        token: u64,
        extension: String,
        request: ExtensionRequest,
    },
    /// Screenshot `tab` and settle `call` with the image.
    Capture {
        call: CallId,
        tab: TabId,
        format: String,
        quality: i64,
    },
    /// A call the side panel's stores answer (`chrome.sidePanel`, and
    /// `runtime.getContexts` with its `runtime.openPanels`): the hub routes it (`Hub::side_panel_call`),
    /// because those stores live in `DelegateState` and a call may change
    /// them. It comes out of `call` like any other effect, so a page's
    /// call is held, ordered and refused as the rest are.
    SidePanel(BridgeCall),
    /// A page's report that its `chrome.debugger` attached to a tab or
    /// detached from one (`debugger.attached`, `debugger.detached`,
    /// `js/debugger.js`): the hub hands it to the relay's set of debuggers
    /// (`Bridge::page_debugger`), which shows or hides the tab, and then
    /// answers the call. A worker's report never comes here: its relay
    /// takes it first (`Bridge::worker_call`).
    Debugger(BridgeCall),
    /// A request the engine makes for itself (a side panel that an action
    /// click or a `setOptions` closes or opens): `Hub::run` gives it to
    /// `DelegateState::request_of_engine`, which takes an engine token.
    EngineRequest {
        extension: String,
        request: ExtensionRequest,
    },
}

/// The event that tells a context the Chromium tab ids of its extension's
/// open side panels (`js/runtime.js` taps it): one argument, the list.
pub(crate) const PANELS_EVENT: &str = "bridge.panels";

/// The engine's answer to a page's report of its own Chromium tab id
/// (`js/page_report.js`): one argument, `{panel}`, whether the page is a
/// side panel's. A page of an extension that can have a side panel holds
/// its `runtime.sendMessage` until it hears.
pub(crate) const REPORTED_EVENT: &str = "bridge.reported";

/// The engine's word to a side panel's page that its extension's other
/// contexts have been told the panel's Chromium id, so a message it sends
/// now arrives without a `sender.tab`. No argument.
pub(crate) const TOLD_EVENT: &str = "bridge.told";

/// A side panel's page that holds its first message until the worker has
/// taken the delivery that names the panel's id.
struct Telling {
    token: u64,
    /// The page lane's key of the panel's frame.
    page: u64,
    extension: String,
    since: Instant,
}

/// Whether a worker's `call` reads or names tabs, so that its answer
/// hands out tab ids or it is given one: every `chrome.tabs` function
/// (not the bridge's own `stripOf`, which answers a strip's fields for an
/// id the context already has), and a `chrome.windows` read asked for
/// with its tabs (`populate`). Nothing else is held for the ids.
fn hands_out_tab_ids(call: &BridgeCall) -> bool {
    match call.namespace.as_str() {
        "tabs" => call.method != "stripOf",
        "windows" => {
            matches!(
                call.method.as_str(),
                "get" | "getAll" | "getCurrent" | "getLastFocused"
            ) && call
                .args
                .iter()
                .any(|arg| arg.get("populate").and_then(Value::as_bool) == Some(true))
        }
        _ => false,
    }
}

/// Whether `call` is one the side panel's stores answer, not the router.
fn side_panel_call(call: &BridgeCall) -> bool {
    call.namespace == "sidePanel"
        || (call.namespace == "runtime"
            && matches!(call.method.as_str(), "getContexts" | "openPanels"))
}

/// Whether `call` is a report of an extension's own debugger, which the
/// relay's set of debuggers takes, not the router.
fn debugger_report(call: &BridgeCall) -> bool {
    call.namespace == "debugger" && matches!(call.method.as_str(), "attached" | "detached")
}

/// A request the model has not answered.
struct Pending {
    call: CallId,
    extension: String,
    /// How to answer, on the call's last request; `None` on the others.
    then: Option<Then>,
}

/// A reply whose answer needs a strip that is not published yet.
struct Waiting {
    call: CallId,
    extension: String,
    then: Then,
    reply: ExtensionReply,
    since: Instant,
    /// The publish count when the reply came.
    seq: u64,
}

#[derive(Default)]
pub(crate) struct BridgeState {
    /// The strip extensions have been told of: what queries are answered
    /// from and what the next one is compared with.
    pub(super) published: Snapshot,
    /// The newest strip, when it is not published yet, and since when.
    held: Option<(Snapshot, Instant)>,
    pub(super) ids: IdSpace,
    grants: HashMap<String, Grants>,
    /// What the router reads of each served extension (`set_facts`).
    facts: HashMap<String, ExtensionFacts>,
    /// The served extensions that have a service worker.
    workers: BTreeSet<String>,
    /// The events each extension's service worker has listened for this run.
    listeners: BTreeMap<String, BTreeSet<String>>,
    /// The events each extension page context has listened for, by the
    /// context's key, with the extension whose page it is. A context's
    /// registrations end with its frame (`page_gone`).
    page_listeners: BTreeMap<u64, (String, BTreeSet<String>)>,
    pending: HashMap<u64, Pending>,
    /// The first refusal among a call's requests.
    refused: HashMap<CallId, String>,
    waiting: Vec<Waiting>,
    /// Page calls held for their tab or their extension, and worker calls
    /// held for the tabs' ids, in arrival order.
    held_calls: Vec<HeldCall>,
    /// The helper's first listing of Chromium's tab ids has been joined
    /// (`table_joined`).
    joined: bool,
    /// A worker's call waited out its bound for ids that did not come:
    /// a listing that never comes is waited for once, not by every call.
    waited_out: bool,
    last_token: u64,
    last_focused: Option<WindowId>,
    /// The window of each open side panel's view (`set_panel_windows`).
    panel_windows: HashMap<TabId, WindowId>,
    /// The Chromium tab ids of the open panels' pages, which the scripts
    /// take off `sender.tab` (`table::PanelIds`).
    panel_ids: PanelIds,
    /// The panel pages waiting for `told`, and the last token given.
    telling: Vec<Telling>,
    last_telling: u64,
    seq: u64,
}

impl BridgeState {
    pub(super) fn view(&self, extension: &str) -> View<'_> {
        View {
            snapshot: &self.published,
            ids: &self.ids,
            grants: self.grants.get(extension).unwrap_or(&NO_GRANTS),
            last_focused: self.last_focused,
            panels: &self.panel_windows,
            facts: self.facts.get(extension).unwrap_or(&NO_FACTS),
        }
    }

    /// The strip's fields of `tab` (`View::strip_json`), as `extension`
    /// is given them: what a click on its action carries for the worker
    /// to lay over Chromium's own tab object. `None` for a tab the
    /// published strip does not hold.
    pub(crate) fn strip_of(&self, extension: &str, tab: TabId) -> Option<Value> {
        let view = self.view(extension);
        let (window, index) = view.snapshot.locate(tab)?;
        Some(view.strip_json(window, index))
    }

    /// What the router reads of each served extension beyond its grants.
    /// No effects.
    pub(crate) fn set_facts(&mut self, facts: Vec<(String, ExtensionFacts)>) {
        self.facts = facts.into_iter().collect();
    }

    /// The open side panels' views and their windows, from the engine's
    /// registry: what "the current window" is for a call from a panel's
    /// page (`View::current_window`).
    pub(crate) fn set_panel_windows(&mut self, panels: HashMap<TabId, WindowId>) {
        // A closed panel's Chromium id is no panel's any more. The scripts
        // are not told (that would start a stopped worker to forget an id
        // Chromium never gives out again in this run); they hear the whole
        // list at the next panel, and ask for it when they start.
        self.panel_ids.keep(|view| panels.contains_key(&view));
        self.panel_windows = panels;
    }

    /// The page of `extension`'s panel in `view` reported its Chromium tab
    /// id (`pages::panel_report`); `page` is the page lane's key of its
    /// frame. Kept while the panel is open, and told
    /// to the contexts of that extension that listen (`bridge.panels`,
    /// with every id of the extension's panels), so a message from the
    /// panel's page reaches them without a `sender.tab`. Refused: the view
    /// is no open panel, or the id is a strip tab's.
    ///
    /// The page is answered first (`REPORTED_EVENT`): it holds its
    /// `runtime.sendMessage` until it knows whether it is a panel's, and a
    /// panel's page then holds it until `TOLD_EVENT`. That comes at once
    /// when no worker has to be told (none listens, or the id is one every
    /// context has), and otherwise when the worker has taken the delivery
    /// (`Effect::EmitTold`, `told`): the report travels to the worker
    /// through the relay, and the page's own message, which goes straight
    /// through Chromium, would arrive before it.
    pub(crate) fn panel_reported(
        &mut self,
        view: TabId,
        extension: &str,
        chromium: i64,
        page: u64,
        now: Instant,
    ) -> Vec<Effect> {
        let to_page = |event: &'static str, args: Vec<Value>| Effect::EmitPage {
            page,
            extension: extension.to_string(),
            event,
            args,
        };
        let strip_tab = self
            .ids
            .tab(chromium)
            .is_some_and(|tab| self.published.locate(tab).is_some());
        if strip_tab || !self.panel_windows.contains_key(&view) {
            log::warn!(
                "bridge: {extension}'s side panel report of Chromium tab {chromium} for {view:?} was ignored"
            );
            return vec![to_page(REPORTED_EVENT, vec![json!({ "panel": false })])];
        }
        let mut effects = vec![to_page(REPORTED_EVENT, vec![json!({ "panel": true })])];
        if !self.panel_ids.report(view, extension, chromium) {
            // The id every context was told already (the page reloaded).
            effects.push(to_page(TOLD_EVENT, Vec::new()));
            return effects;
        }
        log::info!("bridge: {extension}'s side panel {view:?} is Chromium tab {chromium} (no tab)");
        let mut waits = false;
        for effect in self.emit(
            extension,
            PANELS_EVENT,
            vec![json!(self.panel_ids.of(extension))],
        ) {
            match effect {
                Effect::Emit {
                    extension,
                    event,
                    args,
                } => {
                    self.last_telling += 1;
                    self.telling.push(Telling {
                        token: self.last_telling,
                        page,
                        extension: extension.clone(),
                        since: now,
                    });
                    waits = true;
                    effects.push(Effect::EmitTold {
                        extension,
                        event,
                        args,
                        token: self.last_telling,
                    });
                }
                other => effects.push(other),
            }
        }
        if !waits {
            effects.push(to_page(TOLD_EVENT, Vec::new()));
        }
        effects
    }

    /// The delivery `token` of a panel's id is over: the worker took it, or
    /// it could not be delivered. The panel's page is told it may send
    /// (`TOLD_EVENT`). A token nobody waits on any more says nothing.
    pub(crate) fn told(&mut self, token: u64) -> Vec<Effect> {
        let Some(at) = self.telling.iter().position(|t| t.token == token) else {
            return Vec::new();
        };
        let telling = self.telling.remove(at);
        log::debug!(
            "bridge: {}'s side panel page {} may send: its id has been told",
            telling.extension,
            telling.page
        );
        vec![Effect::EmitPage {
            page: telling.page,
            extension: telling.extension,
            event: TOLD_EVENT,
            args: Vec::new(),
        }]
    }

    /// Whether `tick` has anything to wait for.
    pub(crate) fn needs_tick(&self) -> bool {
        self.held.is_some()
            || !self.waiting.is_empty()
            || !self.held_calls.is_empty()
            || !self.telling.is_empty()
    }

    /// Whether `extension` is one the bridge serves: the page lane drops
    /// a page's call from any other.
    pub(crate) fn serves(&self, extension: &str) -> bool {
        self.grants.contains_key(extension)
    }

    /// The served extensions: each one's id, what it may see, and whether it
    /// has a service worker (the loader). Returns a wake for each worker not
    /// known before; an extension without a worker is served through its
    /// pages alone.
    pub(crate) fn set_extensions(
        &mut self,
        extensions: Vec<(String, Grants, bool)>,
    ) -> Vec<Effect> {
        let woken = extensions
            .iter()
            .filter(|(id, _, worker)| *worker && !self.workers.contains(id))
            .map(|(id, _, _)| Effect::Wake(id.clone()))
            .collect();
        self.workers = extensions
            .iter()
            .filter(|(_, _, worker)| *worker)
            .map(|(id, _, _)| id.clone())
            .collect();
        self.grants = extensions
            .into_iter()
            .map(|(id, grants, _)| (id, grants))
            .collect();
        let grants = &self.grants;
        self.listeners.retain(|id, _| grants.contains_key(id));
        self.page_listeners
            .retain(|_, (id, _)| grants.contains_key(id));
        woken
    }

    /// The page contexts `keys` are gone (their frames were): their
    /// registrations go with them.
    pub(crate) fn page_gone(&mut self, keys: &[u64]) {
        self.page_listeners.retain(|key, _| !keys.contains(key));
    }

    /// A bridged function was called.
    pub(crate) fn call(&mut self, call: BridgeCall) -> Vec<Effect> {
        if call.namespace == "bridge" {
            let event = call.args.first().and_then(Value::as_str);
            let result = match (call.method.as_str(), event) {
                ("listen", Some(event)) => {
                    match call.page {
                        Some(page) => {
                            self.page_listeners
                                .entry(page)
                                .or_insert_with(|| (call.extension.clone(), BTreeSet::new()))
                                .1
                                .insert(event.to_string());
                        }
                        None => {
                            self.listeners
                                .entry(call.extension.clone())
                                .or_default()
                                .insert(event.to_string());
                        }
                    }
                    Ok(Value::Null)
                }
                // What a context asks as it starts: the Chromium tab ids
                // of its extension's open side panels.
                ("panels", _) => Ok(json!(self.panel_ids.of(&call.extension))),
                _ => Err(router::unsupported("bridge", &call.method)),
            };
            return vec![Effect::Reply(call.id, result)];
        }
        // `sender.tab`'s strip, asked for an id that is a panel's: the
        // mark, in place of the `null` a tab the strip lacks gets.
        if call.namespace == "tabs"
            && call.method == "stripOf"
            && call
                .args
                .first()
                .and_then(Value::as_i64)
                .is_some_and(|id| self.panel_ids.holds(&call.extension, id))
        {
            return vec![Effect::Reply(call.id, Ok(json!({ "panel": true })))];
        }
        if side_panel_call(&call) {
            return vec![Effect::SidePanel(call)];
        }
        if debugger_report(&call) {
            return vec![Effect::Debugger(call)];
        }
        let outcome = router::route(&call, &self.view(&call.extension));
        self.outcome_effects(call.id, &call.extension, outcome)
    }

    /// What `outcome` of `call` comes to: the one place a router outcome
    /// becomes effects, for the router's calls and for the hub's.
    pub(super) fn outcome_effects(
        &mut self,
        call: CallId,
        extension: &str,
        outcome: Outcome,
    ) -> Vec<Effect> {
        match outcome {
            Outcome::Answer(value) => vec![Effect::Reply(call, Ok(value))],
            Outcome::Reject(message) => vec![Effect::Reply(call, Err(message))],
            Outcome::Forward {
                method,
                args,
                strip,
            } => vec![Effect::Reply(
                call,
                Ok(router::forward_value(&method, args, strip)),
            )],
            Outcome::Capture {
                tab,
                format,
                quality,
            } => vec![Effect::Capture {
                call,
                tab,
                format,
                quality,
            }],
            Outcome::Request { requests, then } => self.request(call, extension, requests, then),
        }
    }

    /// An event that is no difference of the strip (`sidePanel.onOpened`,
    /// `onClosed`), for the contexts of `extension` that listen for it: its
    /// worker, if it has one and the worker registered the event, and each
    /// of its pages that did. A stopped worker that does not listen is not
    /// started for it.
    pub(super) fn emit(
        &self,
        extension: &str,
        event: &'static str,
        args: Vec<Value>,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        let worker_listens = self.workers.contains(extension)
            && self
                .listeners
                .get(extension)
                .is_some_and(|names| names.contains(event));
        if worker_listens {
            effects.push(Effect::Emit {
                extension: extension.to_string(),
                event,
                args: args.clone(),
            });
        }
        for (page, (owner, names)) in &self.page_listeners {
            if owner == extension && names.contains(event) {
                effects.push(Effect::EmitPage {
                    page: *page,
                    extension: extension.to_string(),
                    event,
                    args: args.clone(),
                });
            }
        }
        effects
    }

    /// The shell resolved a shortcut to `extension`'s command `name`, with
    /// `tab` the active tab of the window it was pressed in:
    /// `commands.onCommand(name, tab)` for every context of the extension
    /// that listens, the tab as that extension may see it. A stopped
    /// worker that listens is started by the delivery; one that does not
    /// is left alone.
    pub(super) fn command(&self, extension: &str, name: &str, tab: TabId) -> Vec<Effect> {
        let view = self.view(extension);
        let mut args = vec![json!(name)];
        if let Some((window, index)) = view.snapshot.locate(tab) {
            args.push(view.tab_json(window, index));
        }
        self.emit(extension, "commands.onCommand", args)
    }

    /// A call from an extension page (the page lane). It is held while its
    /// extension is not served, or while the tab it is in is not in the
    /// published strip and a strip is still pending; `settle` answers it when
    /// that is so, or at `PUBLISH_WAIT`. A call that finds calls held ahead of
    /// it is held behind them, so the calls are answered in arrival order.
    pub(crate) fn page_call(&mut self, call: BridgeCall, now: Instant) -> Vec<Effect> {
        let hold = if !self.serves(&call.extension) {
            Some(Hold::Served)
        } else if let Asker::View(tab) = call.asker
            && self.held.is_some()
            && self.published.locate(tab).is_none()
        {
            Some(Hold::Tab(tab))
        } else if self
            .held_calls
            .iter()
            .any(|held| held.hold != Hold::Ids && held.call.extension == call.extension)
        {
            // Behind the extension's held page calls; a worker's held call
            // is another lane's, and holds no page up.
            Some(Hold::Queued)
        } else {
            None
        };
        match hold {
            Some(hold) => {
                self.held_calls.push(HeldCall {
                    call,
                    hold,
                    since: now,
                });
                Vec::new()
            }
            None => self.call(call),
        }
    }

    /// A call from a service worker (the relay's lane). One that reads or
    /// names tabs is held while the ids it would be answered with are not
    /// yet Chromium's (`ids_pending`), for `PUBLISH_WAIT` at most, and
    /// then answered in arrival order (`settle`): an extension that keeps
    /// a synthetic id from its first `tabs.query` and hands it to
    /// Chromium's own `debugger.attach`, `scripting.executeScript` or
    /// `tabs.sendMessage` gets "No tab with id". Any other call is
    /// answered at once.
    pub(crate) fn worker_call(&mut self, call: BridgeCall, now: Instant) -> Vec<Effect> {
        if hands_out_tab_ids(&call) && self.ids_pending() {
            self.held_calls.push(HeldCall {
                call,
                hold: Hold::Ids,
                since: now,
            });
            return Vec::new();
        }
        self.call(call)
    }

    /// Whether a worker's tab call would be answered too early: before the
    /// first strip is published (the answer would be no tabs), or while a
    /// live tab of the published strip still has a synthetic id that a
    /// join can replace: the helper's first listing has not come, or a
    /// strip is waiting for its ids (which the two-second rule ends).
    fn ids_pending(&self) -> bool {
        if self.seq == 0 {
            return self.held.is_some() || !self.waited_out;
        }
        let synthetic = self
            .published
            .windows
            .iter()
            .flat_map(|window| &window.tabs)
            .any(|tab| !tab.deferred && !self.ids.is_live(tab.id));
        synthetic && (self.held.is_some() || !(self.joined || self.waited_out))
    }

    /// The helper's listing of Chromium's tab ids was joined into the
    /// table: the first one ends the wait of the workers' tab calls.
    pub(crate) fn table_joined(
        &mut self,
        chromium: &dyn Fn(TabId) -> Option<i64>,
        now: Instant,
    ) -> Vec<Effect> {
        self.joined = true;
        self.table_changed(chromium, now)
    }

    fn request(
        &mut self,
        call: CallId,
        extension: &str,
        requests: Vec<ExtensionRequest>,
        then: Then,
    ) -> Vec<Effect> {
        let last = requests.len().saturating_sub(1);
        let mut then = Some(then);
        requests
            .into_iter()
            .enumerate()
            .map(|(at, request)| {
                self.last_token += 1;
                let token = self.last_token;
                self.pending.insert(
                    token,
                    Pending {
                        call,
                        extension: extension.to_string(),
                        then: if at == last { then.take() } else { None },
                    },
                );
                Effect::Request {
                    token,
                    extension: extension.to_string(),
                    request,
                }
            })
            .collect()
    }

    /// The model answered the request `token`.
    pub(crate) fn extension_reply(
        &mut self,
        token: u64,
        reply: ExtensionReply,
        now: Instant,
    ) -> Vec<Effect> {
        let Some(pending) = self.pending.remove(&token) else {
            return Vec::new();
        };
        if let ExtensionReply::Refused(message) = &reply {
            // In the extension's ids, not the model's.
            let message = router::refusal(message, &self.ids);
            self.refused.entry(pending.call).or_insert(message);
        }
        let Some(then) = pending.then else {
            return Vec::new();
        };
        if let Some(message) = self.refused.remove(&pending.call) {
            return vec![Effect::Reply(pending.call, Err(message))];
        }
        self.waiting.push(Waiting {
            call: pending.call,
            extension: pending.extension,
            then,
            reply,
            since: now,
            seq: self.seq,
        });
        self.settle(now)
    }

    /// Answer every waiting call the published strip now allows.
    fn settle(&mut self, now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        // A delivery of a panel's id that nobody reported on (its relay
        // went with it): its page gave up waiting long before this.
        self.telling
            .retain(|telling| now.saturating_duration_since(telling.since) < PUBLISH_WAIT);
        // Held calls, in arrival order within each extension and lane (its
        // pages, its worker): a call goes once its hold is met or its bound
        // has passed, and a later call of the same extension and lane never
        // goes before it. Another extension's held calls do not wait for
        // it, and neither does the other lane's.
        let mut blocked: BTreeSet<(String, bool)> = BTreeSet::new();
        let mut kept = Vec::new();
        for held in std::mem::take(&mut self.held_calls) {
            let lane = (held.call.extension.clone(), held.hold == Hold::Ids);
            if blocked.contains(&lane) {
                kept.push(held);
                continue;
            }
            let ready = match held.hold {
                Hold::Queued => true,
                Hold::Served => self.serves(&held.call.extension),
                Hold::Tab(tab) => self.published.locate(tab).is_some() || self.held.is_none(),
                Hold::Ids => !self.ids_pending(),
            };
            let waited = now.saturating_duration_since(held.since);
            if !ready && waited < PUBLISH_WAIT {
                blocked.insert(lane);
                kept.push(held);
                continue;
            }
            if held.hold == Hold::Ids {
                // Answered with what there is; and when the ids did not
                // come, the calls after it do not wait for them again.
                self.waited_out |= !ready;
                effects.extend(self.call(held.call));
                continue;
            }
            if self.serves(&held.call.extension) {
                effects.extend(self.call(held.call));
            } else {
                effects.push(Effect::Reply(held.call.id, Err(UNBRIDGED.to_string())));
            }
        }
        self.held_calls = kept;
        for waiting in std::mem::take(&mut self.waiting) {
            let settled = router::settle(
                &waiting.then,
                &waiting.reply,
                &self.view(&waiting.extension),
                self.seq > waiting.seq,
                now.saturating_duration_since(waiting.since),
            );
            match settled {
                Settled::Now(result) => effects.push(Effect::Reply(waiting.call, result)),
                Settled::Wait => self.waiting.push(waiting),
                Settled::More { requests, then } => {
                    effects.extend(self.request(waiting.call, &waiting.extension, requests, then));
                }
            }
        }
        effects
    }

    /// The shell described the strip. `chromium` is the table: Chromium's
    /// tab id for a tab, when it is known.
    pub(crate) fn strip_changed(
        &mut self,
        snapshot: Snapshot,
        chromium: &dyn Fn(TabId) -> Option<i64>,
        now: Instant,
    ) -> Vec<Effect> {
        let since = self.held.take().map_or(now, |(_, since)| since);
        self.held = Some((snapshot, since));
        self.publish(chromium, now)
    }

    /// The table gained or lost an entry.
    pub(crate) fn table_changed(
        &mut self,
        chromium: &dyn Fn(TabId) -> Option<i64>,
        now: Instant,
    ) -> Vec<Effect> {
        if self.held.is_none() {
            // Nothing new about the strip; the ids may have changed.
            self.held = Some((self.published.clone(), now));
        }
        self.publish(chromium, now)
    }

    /// Time passed.
    pub(crate) fn tick(
        &mut self,
        chromium: &dyn Fn(TabId) -> Option<i64>,
        now: Instant,
    ) -> Vec<Effect> {
        if self.held.is_some() {
            self.publish(chromium, now)
        } else {
            self.settle(now)
        }
    }

    /// Publish the held strip if every live tab in it has an id
    /// extensions can use, or it has waited long enough: bring the ids up
    /// to date, tell the listeners what changed, answer who was waiting.
    fn publish(&mut self, chromium: &dyn Fn(TabId) -> Option<i64>, now: Instant) -> Vec<Effect> {
        let Some((next, since)) = self.held.take() else {
            return self.settle(now);
        };
        let tabs = || next.windows.iter().flat_map(|window| &window.tabs);
        let ready = tabs().all(|tab| {
            tab.deferred || chromium(tab.id).is_some() || self.ids.ext(tab.id).is_some()
        });
        if !ready && now.saturating_duration_since(since) < PUBLISH_WAIT {
            self.held = Some((next, since));
            return self.settle(now);
        }
        // A deferred tab stays on its synthetic id even while a hidden
        // browser warms it: that browser goes again.
        let pairs: Vec<(TabId, Option<i64>)> = tabs()
            .map(|tab| (tab.id, if tab.deferred { None } else { chromium(tab.id) }))
            .collect();
        let replaced = self.ids.sync(&pairs);
        let events = differ::diff(&self.published, &next, &self.ids, &replaced);
        if let Some(focused) = next.windows.iter().find(|window| window.focused) {
            self.last_focused = Some(focused.id);
        }
        if self
            .last_focused
            .is_some_and(|window| next.window(window).is_none())
        {
            self.last_focused = None;
        }
        self.published = next;
        self.seq += 1;
        let mut effects = Vec::new();
        // A worker is woken only for an event it registered, and only an
        // extension with a worker has one.
        for (extension, names) in &self.listeners {
            if !self.workers.contains(extension) {
                continue;
            }
            let view = self.view(extension);
            for event in &events {
                if let Some((name, args)) = differ::render(event, &view)
                    && names.contains(name)
                {
                    effects.push(Effect::Emit {
                        extension: extension.clone(),
                        event: name,
                        args,
                    });
                }
            }
        }
        for (page, (extension, names)) in &self.page_listeners {
            let view = self.view(extension);
            for event in &events {
                if let Some((name, args)) = differ::render(event, &view)
                    && names.contains(name)
                {
                    effects.push(Effect::EmitPage {
                        page: *page,
                        extension: extension.clone(),
                        event: name,
                        args,
                    });
                }
            }
        }
        let keep = self.published.tab_ids();
        self.ids.prune(&keep);
        effects.extend(self.settle(now));
        effects
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use osmena_core::{ExtensionReply, ExtensionRequest, TabId};
    use serde_json::json;
    use url::Url;

    use super::*;
    use crate::bridge::ids::SYNTHETIC_BASE;
    use crate::bridge::router::SETTLE_QUIET;
    use crate::bridge::snapshot::SnapTab;
    use crate::bridge::snapshot::tests::{snapshot, tab, window};
    use crate::bridge::transport::Asker;

    const EXT: &str = "abcdefghijklmnopabcdefghijklmnop";
    const OTHER: &str = "ponmlkjihgfedcbaponmlkjihgfedcba";

    fn live(tab: TabId) -> Option<i64> {
        Some(100 + tab.0 as i64)
    }

    fn unknown(_: TabId) -> Option<i64> {
        None
    }

    fn all() -> Grants {
        Grants {
            tabs: true,
            all_urls: true,
            hosts: vec!["<all_urls>".to_string()],
        }
    }

    fn call(id: CallId, extension: &str, namespace: &str, method: &str, args: Value) -> BridgeCall {
        BridgeCall {
            id,
            extension: extension.to_string(),
            asker: Asker::Worker,
            namespace: namespace.to_string(),
            method: method.to_string(),
            args: args.as_array().cloned().unwrap_or_default(),
            page: None,
        }
    }

    fn listen(state: &mut BridgeState, extension: &str, event: &str) {
        assert_eq!(
            state.call(call(0, extension, "bridge", "listen", json!([event]))),
            vec![Effect::Reply(0, Ok(Value::Null))]
        );
    }

    /// A bridge that knows two extensions (one entitled to everything,
    /// one to nothing) and has published one window of `tabs`.
    fn started(tabs: Vec<SnapTab>) -> (BridgeState, Instant) {
        let mut state = BridgeState::default();
        state.set_extensions(vec![
            (EXT.to_string(), all(), true),
            (OTHER.to_string(), Grants::default(), true),
        ]);
        let now = Instant::now();
        assert_eq!(
            state.strip_changed(snapshot(vec![window(1, true, tabs)]), &live, now),
            vec![]
        );
        (state, now)
    }

    fn queried_ids(state: &mut BridgeState) -> Vec<i64> {
        let effects = state.call(call(1, EXT, "tabs", "query", json!([{}])));
        let [Effect::Reply(1, Ok(Value::Array(tabs)))] = &effects[..] else {
            panic!("{effects:?}");
        };
        tabs.iter().map(|t| t["id"].as_i64().unwrap()).collect()
    }

    #[test]
    fn a_new_extension_is_woken_once() {
        let mut state = BridgeState::default();
        assert_eq!(
            state.set_extensions(vec![(EXT.to_string(), all(), true)]),
            vec![Effect::Wake(EXT.to_string())]
        );
        assert_eq!(
            state.set_extensions(vec![
                (EXT.to_string(), all(), true),
                (OTHER.to_string(), Grants::default(), true)
            ]),
            vec![Effect::Wake(OTHER.to_string())]
        );
        assert_eq!(
            state.set_extensions(vec![(EXT.to_string(), all(), true)]),
            vec![]
        );
    }

    #[test]
    fn a_strip_is_held_until_the_table_knows_every_live_tab_or_two_seconds_pass() {
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        listen(&mut state, EXT, "tabs.onReplaced");
        let t0 = Instant::now();
        let strip = snapshot(vec![window(1, true, vec![tab(1, "a*"), tab(2, "b~")])]);
        assert_eq!(state.strip_changed(strip, &unknown, t0), vec![]);
        assert!(state.needs_tick());
        assert_eq!(queried_ids(&mut state), Vec::<i64>::new());
        assert_eq!(state.tick(&unknown, t0 + Duration::from_secs(1)), vec![]);
        assert_eq!(queried_ids(&mut state), Vec::<i64>::new());
        // Two seconds: published with synthetic ids for both.
        assert_eq!(state.tick(&unknown, t0 + PUBLISH_WAIT), vec![]);
        assert!(!state.needs_tick());
        assert_eq!(
            queried_ids(&mut state),
            vec![SYNTHETIC_BASE, SYNTHETIC_BASE + 1]
        );
        // The table catches up: the live tab's id is replaced; the
        // deferred one keeps its own.
        assert_eq!(
            state.table_changed(&live, t0 + PUBLISH_WAIT),
            vec![Effect::Emit {
                extension: EXT.to_string(),
                event: "tabs.onReplaced",
                args: vec![json!(101), json!(SYNTHETIC_BASE)],
            }]
        );
        assert_eq!(queried_ids(&mut state), vec![101, SYNTHETIC_BASE + 1]);
        // A tab that was published without Chromium's id holds nothing up
        // afterwards.
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a*")])]),
            &unknown,
            t0,
        );
        state.tick(&unknown, t0 + PUBLISH_WAIT);
        let later = t0 + PUBLISH_WAIT + Duration::from_millis(1);
        let only_new = |t: TabId| (t == TabId(2)).then_some(102);
        state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a*"), tab(2, "b")])]),
            &only_new,
            later,
        );
        assert_eq!(queried_ids(&mut state), vec![SYNTHETIC_BASE, 102]);
    }

    /// The tab ids a reply to `tabs.query` carries.
    fn ids_of(effect: &Effect, call: CallId) -> Vec<i64> {
        let Effect::Reply(id, Ok(Value::Array(tabs))) = effect else {
            panic!("{effect:?}");
        };
        assert_eq!(*id, call);
        tabs.iter().map(|t| t["id"].as_i64().unwrap()).collect()
    }

    #[test]
    fn a_workers_tab_call_before_the_first_listing_waits_for_the_real_ids() {
        // The browser starts, and the strip is published with synthetic
        // ids after two seconds because nothing has listed Chromium's ids
        // yet (the helper is not loaded). Then an extension's worker
        // starts and lists the tabs.
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        let t0 = Instant::now();
        let strip = snapshot(vec![window(1, true, vec![tab(1, "a*"), tab(2, "b")])]);
        state.strip_changed(strip, &unknown, t0);
        state.tick(&unknown, t0 + PUBLISH_WAIT);
        let later = t0 + PUBLISH_WAIT + Duration::from_millis(500);
        // Held: an id answered now would be a synthetic one, which
        // Chromium's own `debugger`, `scripting` and `tabs.sendMessage`
        // do not know.
        assert_eq!(
            state.worker_call(call(1, EXT, "tabs", "query", json!([{}])), later),
            vec![]
        );
        assert_eq!(
            state.worker_call(call(2, EXT, "tabs", "get", json!([SYNTHETIC_BASE])), later),
            vec![]
        );
        assert!(state.needs_tick());
        assert_eq!(state.tick(&unknown, later + Duration::from_secs(1)), vec![]);
        // The helper's first listing is joined: both are answered, in the
        // order they came, with Chromium's ids.
        let effects = state.table_joined(&live, later + Duration::from_secs(1));
        assert_eq!(effects.len(), 2, "{effects:?}");
        assert_eq!(ids_of(&effects[0], 1), vec![101, 102]);
        let Effect::Reply(2, Ok(forward)) = &effects[1] else {
            panic!("{effects:?}");
        };
        assert_eq!(forward["__forward"]["args"][0], 101);
        assert!(!state.needs_tick());
        // From now on a worker's call is answered at once.
        let effects = state.worker_call(call(3, EXT, "tabs", "query", json!([{}])), later);
        assert_eq!(ids_of(&effects[0], 3), vec![101, 102]);
    }

    #[test]
    fn a_workers_tab_call_is_answered_with_what_there_is_at_the_bound() {
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        let t0 = Instant::now();
        // Before the shell has described any strip: held, not answered [].
        assert_eq!(
            state.worker_call(call(1, EXT, "tabs", "query", json!([{}])), t0),
            vec![]
        );
        let strip = snapshot(vec![window(1, true, vec![tab(1, "a*")])]);
        assert_eq!(state.strip_changed(strip, &unknown, t0), vec![]);
        assert_eq!(
            state.tick(&unknown, t0 + PUBLISH_WAIT - Duration::from_millis(1)),
            vec![]
        );
        // Nothing ever lists the tab: at the bound the strip is published
        // with a synthetic id, and the call is answered with it.
        let effects = state.tick(&unknown, t0 + PUBLISH_WAIT);
        assert_eq!(effects.len(), 1, "{effects:?}");
        assert_eq!(ids_of(&effects[0], 1), vec![SYNTHETIC_BASE]);
        // A listing that never comes is waited for once, not by every call.
        let after = t0 + PUBLISH_WAIT + Duration::from_secs(5);
        let effects = state.worker_call(call(2, EXT, "tabs", "query", json!([{}])), after);
        assert_eq!(ids_of(&effects[0], 2), vec![SYNTHETIC_BASE]);

        // A strip whose tabs the table already knows is published at once,
        // and the call that waited for the first strip with it.
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        assert_eq!(
            state.worker_call(call(1, EXT, "tabs", "query", json!([{}])), t0),
            vec![]
        );
        let strip = snapshot(vec![window(1, true, vec![tab(1, "a*")])]);
        let effects = state.strip_changed(strip, &live, t0);
        assert_eq!(ids_of(&effects[0], 1), vec![101]);
        // No strip at all within the bound: answered with no tabs.
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        assert_eq!(
            state.worker_call(call(1, EXT, "tabs", "query", json!([{}])), t0),
            vec![]
        );
        let effects = state.tick(&unknown, t0 + PUBLISH_WAIT);
        assert_eq!(ids_of(&effects[0], 1), Vec::<i64>::new());
    }

    #[test]
    fn a_workers_call_that_hands_out_no_tab_id_is_not_held() {
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        let t0 = Instant::now();
        let strip = snapshot(vec![window(1, true, vec![tab(1, "a*")])]);
        state.strip_changed(strip, &unknown, t0);
        state.tick(&unknown, t0 + PUBLISH_WAIT);
        let now = t0 + PUBLISH_WAIT;
        // A tab call is held.
        assert_eq!(
            state.worker_call(call(1, EXT, "tabs", "query", json!([{}])), now),
            vec![]
        );
        // These are answered at once, ahead of it: a registration, the
        // panels' ids, the commands, a window without its tabs, a group
        // read, the strip's fields of a tab (which names no new id), and a
        // side panel call (the hub's).
        for (id, namespace, method, args) in [
            (2, "bridge", "listen", json!(["tabs.onCreated"])),
            (3, "bridge", "panels", json!([])),
            (4, "commands", "getAll", json!([])),
            (5, "windows", "getAll", json!([])),
            (6, "windows", "getAll", json!([{ "populate": false }])),
            (7, "windows", "getCurrent", json!([])),
            (8, "tabGroups", "query", json!([{}])),
            (9, "tabs", "stripOf", json!([SYNTHETIC_BASE])),
            (10, "sidePanel", "getLayout", json!([])),
        ] {
            let effects = state.worker_call(call(id, EXT, namespace, method, args), now);
            assert_eq!(effects.len(), 1, "{namespace}.{method}: {effects:?}");
            assert!(
                matches!(&effects[0], Effect::Reply(reply, _) if *reply == id)
                    || matches!(&effects[0], Effect::SidePanel(_)),
                "{namespace}.{method}: {effects:?}"
            );
        }
        // A window asked for with its tabs hands out their ids: held, in
        // each of the four forms.
        for (id, method, args) in [
            (11, "getAll", json!([{ "populate": true }])),
            (12, "getCurrent", json!([{ "populate": true }])),
            (13, "getLastFocused", json!([{ "populate": true }])),
            (14, "get", json!([1, { "populate": true }])),
        ] {
            assert_eq!(
                state.worker_call(call(id, EXT, "windows", method, args), now),
                vec![],
                "windows.{method}"
            );
        }
        // All five go when the listing is joined, in the order they came.
        let effects = state.table_joined(&live, now);
        let order: Vec<CallId> = effects
            .iter()
            .map(|effect| match effect {
                Effect::Reply(id, Ok(_)) => *id,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(order, vec![1, 11, 12, 13, 14]);

        // A page's calls of the same extension are not queued behind a
        // worker's held call: each lane keeps its own order.
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        let strip = snapshot(vec![window(1, true, vec![tab(1, "a*")])]);
        state.strip_changed(strip, &unknown, t0);
        state.tick(&unknown, t0 + PUBLISH_WAIT);
        assert_eq!(
            state.worker_call(call(1, EXT, "tabs", "query", json!([{}])), now),
            vec![]
        );
        let mut from_page = call(2, EXT, "tabs", "query", json!([{}]));
        from_page.asker = Asker::View(TabId(1));
        from_page.page = Some(5);
        let effects = state.page_call(from_page, now);
        assert_eq!(ids_of(&effects[0], 2), vec![SYNTHETIC_BASE]);
    }

    #[test]
    fn a_published_change_is_emitted_to_listening_extensions_only() {
        let (mut state, now) = started(vec![tab(1, "a*")]);
        listen(&mut state, EXT, "tabs.onCreated");
        listen(&mut state, OTHER, "tabs.onRemoved");
        // A tab opens in front: created, activated and highlighted
        // happen; one extension listens, to one of them.
        let effects = state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a"), tab(2, "b*")])]),
            &live,
            now,
        );
        let [
            Effect::Emit {
                extension,
                event,
                args,
            },
        ] = &effects[..]
        else {
            panic!("{effects:?}");
        };
        assert_eq!((extension.as_str(), *event), (EXT, "tabs.onCreated"));
        assert_eq!(args[0]["id"], 102);
        assert_eq!(args[0]["url"], "https://b.example/");
        // The other hears of a removal.
        assert_eq!(
            state.strip_changed(
                snapshot(vec![window(1, true, vec![tab(2, "b*")])]),
                &live,
                now
            ),
            vec![Effect::Emit {
                extension: OTHER.to_string(),
                event: "tabs.onRemoved",
                args: vec![
                    json!(101),
                    json!({ "windowId": 1, "isWindowClosing": false })
                ],
            }]
        );
        // An extension that is gone is forgotten with its listeners.
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        assert_eq!(state.strip_changed(snapshot(vec![]), &live, now), vec![]);
    }

    #[test]
    fn create_waits_for_the_table_then_resolves() {
        let (mut state, now) = started(vec![tab(1, "a*")]);
        listen(&mut state, EXT, "tabs.onCreated");
        assert_eq!(
            state.call(call(
                7,
                EXT,
                "tabs",
                "create",
                json!([{ "url": "https://c.example/" }])
            )),
            vec![Effect::Request {
                token: 1,
                extension: EXT.to_string(),
                request: ExtensionRequest::OpenTab {
                    window: None,
                    url: Url::parse("https://c.example/").unwrap(),
                    active: true,
                },
            }]
        );
        // The model answers with its id for the tab; no strip has it yet.
        assert_eq!(
            state.extension_reply(1, ExtensionReply::Tab(TabId(3)), now),
            vec![]
        );
        assert!(state.needs_tick());
        // The strip arrives; Chromium's id for the new tab has not.
        let strip = snapshot(vec![window(1, true, vec![tab(1, "a"), tab(3, "c*")])]);
        let partly = |t: TabId| (t != TabId(3)).then_some(100 + t.0 as i64);
        assert_eq!(state.strip_changed(strip, &partly, now), vec![]);
        // The table learns it: the event first, then the promise, both
        // with the id Chromium will accept.
        let effects = state.table_changed(&live, now);
        assert_eq!(effects.len(), 2, "{effects:?}");
        let Effect::Emit {
            event: "tabs.onCreated",
            args,
            ..
        } = &effects[0]
        else {
            panic!("{effects:?}");
        };
        assert_eq!(args[0]["id"], 103);
        let Effect::Reply(7, Ok(created)) = &effects[1] else {
            panic!("{effects:?}");
        };
        assert_eq!(created["id"], 103);
        assert_eq!(created["index"], 1);
        assert_eq!(created["active"], true);
        assert!(!state.needs_tick());
        // A reply nobody waits for is ignored.
        assert_eq!(state.extension_reply(99, ExtensionReply::Done, now), vec![]);
    }

    #[test]
    fn a_refused_request_rejects_with_the_models_words_and_the_extensions_id() {
        let (mut state, now) = started(vec![tab(1, "a*"), tab(2, "b")]);
        let effects = state.call(call(4, EXT, "tabs", "remove", json!([102])));
        assert_eq!(
            effects,
            vec![Effect::Request {
                token: 1,
                extension: EXT.to_string(),
                request: ExtensionRequest::CloseTabs(vec![TabId(2)]),
            }]
        );
        assert_eq!(
            state.extension_reply(
                1,
                ExtensionReply::Refused("No tab with id: 2.".to_string()),
                now
            ),
            // The model named its `TabId(2)`; the extension asked about 102.
            vec![Effect::Reply(4, Err("No tab with id: 102.".to_string()))]
        );
        // An accepted one resolves with undefined when the strip that no
        // longer shows the tab is published.
        state.call(call(5, EXT, "tabs", "remove", json!([102])));
        assert_eq!(state.extension_reply(2, ExtensionReply::Done, now), vec![]);
        assert_eq!(
            state.strip_changed(
                snapshot(vec![window(1, true, vec![tab(1, "a*")])]),
                &live,
                now
            ),
            vec![Effect::Reply(5, Ok(Value::Null))]
        );
    }

    #[test]
    fn several_requests_settle_on_the_last_reply_and_one_refusal_rejects_the_call() {
        let (mut state, now) = started(vec![tab(1, "a*"), tab(2, "b"), tab(3, "c")]);
        let effects = state.call(call(
            6,
            EXT,
            "tabs",
            "move",
            json!([[103, 102], { "index": 0 }]),
        ));
        assert!(matches!(
            effects[..],
            [
                Effect::Request { token: 1, .. },
                Effect::Request { token: 2, .. }
            ]
        ));
        assert_eq!(
            state.extension_reply(
                1,
                ExtensionReply::Refused("No tab with id: 3.".to_string()),
                now
            ),
            vec![]
        );
        assert_eq!(
            state.extension_reply(2, ExtensionReply::Done, now),
            vec![Effect::Reply(6, Err("No tab with id: 103.".to_string()))]
        );
    }

    #[test]
    fn an_answer_that_no_new_strip_follows_settles_after_a_moment() {
        let (mut state, now) = started(vec![tab(1, "a*"), tab(2, "b")]);
        // Tab 2 is already at index 1: the model will change nothing.
        let effects = state.call(call(3, EXT, "tabs", "move", json!([102, { "index": 1 }])));
        assert!(matches!(effects[..], [Effect::Request { token: 1, .. }]));
        assert_eq!(state.extension_reply(1, ExtensionReply::Done, now), vec![]);
        assert_eq!(state.tick(&live, now + Duration::from_millis(100)), vec![]);
        let effects = state.tick(&live, now + SETTLE_QUIET);
        let [Effect::Reply(3, Ok(moved))] = &effects[..] else {
            panic!("{effects:?}");
        };
        assert_eq!(moved["index"], 1);
        assert!(!state.needs_tick());
    }

    #[test]
    fn forwards_captures_and_the_scripts_own_messages() {
        let (mut state, _) = started(vec![tab(1, "a*")]);
        let effects = state.call(call(2, EXT, "tabs", "get", json!([101])));
        let [Effect::Reply(2, Ok(value))] = &effects[..] else {
            panic!("{effects:?}");
        };
        assert_eq!(value["__forward"]["method"], "get");
        assert_eq!(value["__forward"]["args"], json!([101]));
        assert_eq!(value["__forward"]["strip"]["101"]["windowId"], 1);
        assert_eq!(
            state.call(call(3, EXT, "tabs", "captureVisibleTab", json!([]))),
            vec![Effect::Capture {
                call: 3,
                tab: TabId(1),
                format: "jpeg".to_string(),
                quality: 90
            }]
        );
        assert_eq!(
            state.call(call(5, EXT, "bridge", "selfDestruct", json!([]))),
            vec![Effect::Reply(
                5,
                Err("bridge.selfDestruct is not supported.".to_string())
            )]
        );
        // An extension the engine has not listed is answered with what
        // anyone may know: no addresses.
        let effects = state.call(call(
            6,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "tabs",
            "query",
            json!([{}]),
        ));
        let [Effect::Reply(6, Ok(Value::Array(tabs)))] = &effects[..] else {
            panic!("{effects:?}");
        };
        assert_eq!(tabs[0].get("url"), None);
    }

    #[test]
    fn a_worker_s_current_window_is_the_one_focused_last() {
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        let now = Instant::now();
        let two = |first: bool, second: bool| {
            snapshot(vec![
                window(1, first, vec![tab(1, "a*")]),
                window(2, second, vec![tab(2, "b*")]),
            ])
        };
        let current = |state: &mut BridgeState| {
            let effects = state.call(call(
                1,
                EXT,
                "tabs",
                "query",
                json!([{ "active": true, "currentWindow": true }]),
            ));
            let [Effect::Reply(1, Ok(Value::Array(tabs)))] = &effects[..] else {
                panic!("{effects:?}");
            };
            tabs[0]["id"].as_i64().unwrap()
        };
        state.strip_changed(two(false, true), &live, now);
        assert_eq!(current(&mut state), 102);
        // The browser loses focus: still the window that had it.
        state.strip_changed(two(false, false), &live, now);
        assert_eq!(current(&mut state), 102);
        state.strip_changed(two(true, false), &live, now);
        assert_eq!(current(&mut state), 101);
        // That window closes: the first that is left.
        state.strip_changed(
            snapshot(vec![window(2, false, vec![tab(2, "b*")])]),
            &live,
            now,
        );
        assert_eq!(current(&mut state), 102);
    }

    #[test]
    fn only_a_bridged_extension_is_served() {
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        assert!(state.serves(EXT));
        assert!(!state.serves("bcdefghijklmnopabcdefghijklmnopa"));
    }

    /// A page's call on the page lane, from the view of `tab`.
    fn page_call(id: CallId, tab: u64, method: &str) -> BridgeCall {
        BridgeCall {
            id,
            extension: EXT.to_string(),
            asker: Asker::View(TabId(tab)),
            namespace: "tabs".to_string(),
            method: method.to_string(),
            args: Vec::new(),
            page: Some(id),
        }
    }

    fn answered(effects: &[Effect]) -> Vec<CallId> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Reply(id, _) => Some(*id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_page_call_for_a_tab_not_yet_published_waits_for_the_publish() {
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        let t0 = Instant::now();
        state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a*")])]),
            &live,
            t0,
        );
        // The second tab's id is not known yet: the strip is held.
        state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a*"), tab(2, "b")])]),
            &unknown,
            t0,
        );
        assert!(
            state
                .page_call(page_call(7, 2, "getCurrent"), t0)
                .is_empty()
        );
        assert!(state.needs_tick());
        // The table learns the id: the strip publishes, and the call is
        // answered from it.
        let effects = state.table_changed(&live, t0);
        // A live tab's `get` is forwarded to Chromium under its id (102).
        assert_eq!(
            effects.iter().find_map(|effect| match effect {
                Effect::Reply(7, Ok(value)) => Some(value["__forward"]["args"][0].clone()),
                _ => None,
            }),
            Some(json!(102))
        );
    }

    #[test]
    fn a_page_call_from_a_published_view_is_answered_at_once() {
        let (mut state, now) = started(vec![tab(1, "a*")]);
        let effects = state.page_call(page_call(3, 1, "getCurrent"), now);
        assert_eq!(answered(&effects), vec![3]);
    }

    #[test]
    fn a_page_call_for_a_tab_that_never_arrives_is_answered_at_the_bound() {
        let (mut state, t0) = started(vec![tab(1, "a*")]);
        state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a*"), tab(2, "b")])]),
            &unknown,
            t0,
        );
        assert!(
            state
                .page_call(page_call(4, 9, "getCurrent"), t0)
                .is_empty()
        );
        assert!(state.tick(&unknown, t0 + Duration::from_secs(1)).is_empty());
        let effects = state.tick(&unknown, t0 + PUBLISH_WAIT);
        assert_eq!(answered(&effects), vec![4]);
        assert!(!state.needs_tick());
    }

    #[test]
    fn held_page_calls_are_answered_in_arrival_order() {
        let (mut state, t0) = started(vec![tab(1, "a*")]);
        state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a*"), tab(2, "b")])]),
            &unknown,
            t0,
        );
        assert!(
            state
                .page_call(page_call(5, 2, "getCurrent"), t0)
                .is_empty()
        );
        assert!(
            state
                .page_call(page_call(6, 2, "getCurrent"), t0)
                .is_empty()
        );
        assert_eq!(answered(&state.table_changed(&live, t0)), vec![5, 6]);
    }

    #[test]
    fn a_page_call_of_an_extension_not_yet_served_waits_until_it_is_served() {
        let mut state = BridgeState::default();
        let t0 = Instant::now();
        assert!(
            state
                .page_call(page_call(8, 1, "getCurrent"), t0)
                .is_empty()
        );
        assert!(state.needs_tick());
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        let effects = state.tick(&unknown, t0);
        assert_eq!(answered(&effects), vec![8]);
        assert!(matches!(&effects[..], [Effect::Reply(8, Ok(_))]));
    }

    #[test]
    fn a_page_call_of_an_extension_never_served_is_rejected_at_the_bound() {
        let mut state = BridgeState::default();
        let t0 = Instant::now();
        assert!(
            state
                .page_call(page_call(9, 1, "getCurrent"), t0)
                .is_empty()
        );
        assert!(state.tick(&unknown, t0 + Duration::from_secs(1)).is_empty());
        let effects = state.tick(&unknown, t0 + PUBLISH_WAIT);
        assert_eq!(effects, vec![Effect::Reply(9, Err(UNBRIDGED.to_string()))]);
        assert!(!state.needs_tick());
    }

    /// A page's call from `extension`'s page in the view of `tab`, on the
    /// page lane, in the page context `id`.
    fn page_call_from(extension: &str, id: CallId, tab: u64) -> BridgeCall {
        BridgeCall {
            id,
            extension: extension.to_string(),
            asker: Asker::View(TabId(tab)),
            namespace: "tabs".to_string(),
            method: "getCurrent".to_string(),
            args: Vec::new(),
            page: Some(id),
        }
    }

    #[test]
    fn an_extension_without_a_worker_is_served_and_never_woken() {
        let mut state = BridgeState::default();
        let t0 = Instant::now();
        assert_eq!(
            state.set_extensions(vec![(EXT.to_string(), all(), false)]),
            vec![]
        );
        assert!(state.serves(EXT));
        // Its page's call is not held for a worker that does not exist.
        assert_eq!(
            answered(&state.page_call(page_call_from(EXT, 10, 1), t0)),
            vec![10]
        );
        assert!(!state.needs_tick());
    }

    #[test]
    fn a_worker_is_woken_when_an_extension_gains_one() {
        let mut state = BridgeState::default();
        assert_eq!(
            state.set_extensions(vec![(EXT.to_string(), all(), false)]),
            vec![]
        );
        assert_eq!(
            state.set_extensions(vec![(EXT.to_string(), all(), true)]),
            vec![Effect::Wake(EXT.to_string())]
        );
    }

    #[test]
    fn an_unserved_extensions_held_call_does_not_hold_another_extension() {
        let mut state = BridgeState::default();
        let t0 = Instant::now();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        // OTHER is not served, so its page's call waits for it.
        assert!(state.page_call(page_call_from(OTHER, 1, 1), t0).is_empty());
        // EXT's page is served: its call is answered at once.
        assert_eq!(
            answered(&state.page_call(page_call_from(EXT, 2, 1), t0)),
            vec![2]
        );
    }

    /// A page context's `bridge.listen` for `event`, from its frame `page`.
    fn page_listen(state: &mut BridgeState, extension: &str, page: u64, event: &str) {
        let mut listen = call(page, extension, "bridge", "listen", json!([event]));
        listen.page = Some(page);
        assert_eq!(
            state.call(listen),
            vec![Effect::Reply(page, Ok(Value::Null))]
        );
    }

    #[test]
    fn a_page_listener_is_emitted_to_its_page_and_never_to_the_worker() {
        let (mut state, now) = started(vec![tab(1, "a*")]);
        page_listen(&mut state, EXT, 5, "tabs.onCreated");
        let effects = state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a"), tab(2, "b*")])]),
            &live,
            now,
        );
        assert!(matches!(
            &effects[..],
            [Effect::EmitPage { page: 5, extension, event: "tabs.onCreated", .. }] if extension == EXT
        ));
    }

    #[test]
    fn a_page_listener_does_not_wake_an_extension_without_a_worker() {
        let mut state = BridgeState::default();
        assert_eq!(
            state.set_extensions(vec![(EXT.to_string(), all(), false)]),
            vec![]
        );
        let now = Instant::now();
        state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a*")])]),
            &live,
            now,
        );
        page_listen(&mut state, EXT, 5, "tabs.onCreated");
        let effects = state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a"), tab(2, "b*")])]),
            &live,
            now,
        );
        assert!(matches!(&effects[..], [Effect::EmitPage { page: 5, .. }]));
    }

    #[test]
    fn each_listening_page_context_is_emitted_to_and_the_worker_only_its_own_events() {
        let (mut state, now) = started(vec![tab(1, "a*")]);
        listen(&mut state, EXT, "tabs.onRemoved");
        page_listen(&mut state, EXT, 5, "tabs.onCreated");
        page_listen(&mut state, EXT, 6, "tabs.onCreated");
        let effects = state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a"), tab(2, "b*")])]),
            &live,
            now,
        );
        let pages: Vec<u64> = effects
            .iter()
            .map(|effect| match effect {
                Effect::EmitPage {
                    page,
                    event: "tabs.onCreated",
                    ..
                } => *page,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(pages, vec![5, 6]);
    }

    #[test]
    fn a_page_contexts_listeners_end_with_its_frame() {
        let (mut state, now) = started(vec![tab(1, "a*")]);
        page_listen(&mut state, EXT, 5, "tabs.onCreated");
        state.page_gone(&[5]);
        let effects = state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a"), tab(2, "b*")])]),
            &live,
            now,
        );
        assert_eq!(effects, vec![]);
    }

    #[test]
    fn calls_held_for_an_unserved_extension_are_refused_in_order_at_the_bound() {
        let mut state = BridgeState::default();
        let t0 = Instant::now();
        assert!(state.page_call(page_call_from(OTHER, 1, 1), t0).is_empty());
        assert!(state.page_call(page_call_from(OTHER, 3, 1), t0).is_empty());
        assert_eq!(
            state.tick(&unknown, t0 + PUBLISH_WAIT),
            vec![
                Effect::Reply(1, Err(UNBRIDGED.to_string())),
                Effect::Reply(3, Err(UNBRIDGED.to_string())),
            ]
        );
        assert!(!state.needs_tick());
    }

    /// A page's `chrome.sidePanel.<method>` from the view of `tab`.
    fn panel_call(extension: &str, id: CallId, tab: u64, method: &str) -> BridgeCall {
        BridgeCall {
            id,
            extension: extension.to_string(),
            asker: Asker::View(TabId(tab)),
            namespace: "sidePanel".to_string(),
            method: method.to_string(),
            args: vec![json!({ "windowId": 1 })],
            page: Some(id),
        }
    }

    #[test]
    fn a_side_panel_call_is_handed_to_the_hub_from_either_lane() {
        let (mut state, now) = started(vec![tab(1, "a*")]);
        // The worker's lane.
        let open = call(1, EXT, "sidePanel", "open", json!([{ "windowId": 1 }]));
        assert_eq!(state.call(open.clone()), vec![Effect::SidePanel(open)]);
        let contexts = call(2, EXT, "runtime", "getContexts", json!([{}, []]));
        assert_eq!(
            state.call(contexts.clone()),
            vec![Effect::SidePanel(contexts)]
        );
        // And the question getContexts asks first, as it is called.
        let panels = call(5, EXT, "runtime", "openPanels", json!([]));
        assert_eq!(state.call(panels.clone()), vec![Effect::SidePanel(panels)]);
        // A method of `sidePanel` nobody knows is the hub's to reject too.
        let unknown = call(3, EXT, "sidePanel", "frobnicate", json!([]));
        assert_eq!(
            state.call(unknown.clone()),
            vec![Effect::SidePanel(unknown)]
        );
        // Only those two methods of `runtime` are: the rest is the router's,
        // which serves no `runtime`.
        assert_eq!(
            state.call(call(4, EXT, "runtime", "sendMessage", json!([]))),
            vec![Effect::Reply(
                4,
                Err(router::unsupported("runtime", "sendMessage"))
            )]
        );
        // The page lane, from a tab the strip holds: nothing to wait for.
        let from_page = panel_call(EXT, 5, 1, "open");
        assert_eq!(
            state.page_call(from_page.clone(), now),
            vec![Effect::SidePanel(from_page)]
        );
    }

    #[test]
    fn the_strip_of_a_tab_is_what_a_click_on_it_carries() {
        let (state, _) = started(vec![tab(1, "a"), tab(2, "b*")]);
        // The same seven fields `sender.tab` and a forwarded tab get.
        let view = state.view(EXT);
        assert_eq!(
            state.strip_of(EXT, TabId(2)),
            Some(view.strip_json(&view.snapshot.windows[0], 1))
        );
        let strip = state
            .strip_of(EXT, TabId(2))
            .expect("the tab is in the strip");
        assert_eq!(strip["windowId"], json!(1));
        assert_eq!(strip["index"], json!(1));
        assert_eq!(strip["active"], json!(true));
        assert_eq!(strip["groupId"], json!(-1));
        // A tab the published strip does not hold has none.
        assert_eq!(state.strip_of(EXT, TabId(9)), None);
    }

    #[test]
    fn a_debugger_report_is_handed_to_the_hub_from_either_lane() {
        let (mut state, now) = started(vec![tab(1, "a*")]);
        // A page's report (the worker's own is taken by its relay before
        // the hub; one that reached here would be handed over the same).
        let attached = BridgeCall {
            page: Some(7),
            asker: Asker::View(TabId(1)),
            ..call(1, EXT, "debugger", "attached", json!([42]))
        };
        assert_eq!(
            state.page_call(attached.clone(), now),
            vec![Effect::Debugger(attached)]
        );
        let detached = call(2, EXT, "debugger", "detached", json!([{ "targetId": "T" }]));
        assert_eq!(
            state.call(detached.clone()),
            vec![Effect::Debugger(detached)]
        );
        // Nothing else of `debugger` is the engine's: Chromium's own
        // functions are not bridged, and a name nobody knows is refused.
        assert_eq!(
            state.call(call(3, EXT, "debugger", "frobnicate", json!([]))),
            vec![Effect::Reply(
                3,
                Err(router::unsupported("debugger", "frobnicate"))
            )]
        );
        assert_eq!(
            state.call(call(4, EXT, "debugger", "sendCommand", json!([]))),
            vec![Effect::Reply(
                4,
                Err(router::unsupported("debugger", "sendCommand"))
            )]
        );
    }

    #[test]
    fn a_held_debugger_report_is_handed_to_the_hub_when_its_hold_ends() {
        // A report from a panel's page (a view in no strip) made while a
        // strip is pending waits for the strip, like the page's other
        // calls, and is handed over then.
        let (mut state, now) = started(vec![tab(1, "a*")]);
        state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a*"), tab(2, "b")])]),
            &unknown,
            now,
        );
        let report = BridgeCall {
            page: Some(3),
            asker: Asker::View(TabId(9)),
            ..call(3, EXT, "debugger", "attached", json!([42]))
        };
        assert!(state.page_call(report.clone(), now).is_empty());
        assert_eq!(
            state.table_changed(&live, now),
            vec![Effect::Debugger(report)]
        );
    }

    #[test]
    fn a_held_side_panel_call_is_handed_to_the_hub_when_its_hold_ends() {
        // Held for its extension: handed over once the extension is served,
        // after the call held ahead of it and in that order.
        let mut state = BridgeState::default();
        let t0 = Instant::now();
        let first = panel_call(EXT, 1, 1, "getOptions");
        let second = panel_call(EXT, 2, 1, "open");
        assert!(state.page_call(first.clone(), t0).is_empty());
        assert!(state.page_call(second.clone(), t0).is_empty());
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        assert_eq!(
            state.tick(&unknown, t0 + Duration::from_millis(10)),
            vec![Effect::SidePanel(first), Effect::SidePanel(second)]
        );
        assert!(!state.needs_tick());

        // Held for its view (a panel's view is in no strip) while a strip
        // is pending: handed over when the strip publishes.
        let (mut state, now) = started(vec![tab(1, "a*")]);
        state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a*"), tab(2, "b")])]),
            &unknown,
            now,
        );
        let from_panel = panel_call(EXT, 3, 9, "closeSelf");
        assert!(state.page_call(from_panel.clone(), now).is_empty());
        assert_eq!(
            state.table_changed(&live, now),
            vec![Effect::SidePanel(from_panel)]
        );
    }

    #[test]
    fn a_held_side_panel_call_of_an_unserved_extension_is_refused_at_the_bound() {
        let mut state = BridgeState::default();
        let t0 = Instant::now();
        assert!(
            state
                .page_call(panel_call(OTHER, 1, 1, "open"), t0)
                .is_empty()
        );
        assert_eq!(
            state.tick(&unknown, t0 + PUBLISH_WAIT),
            vec![Effect::Reply(1, Err(UNBRIDGED.to_string()))]
        );
    }

    #[test]
    fn a_request_answered_with_nothing_resolves_on_done_and_rejects_on_refusal() {
        let (mut state, now) = started(vec![tab(1, "a*")]);
        let request = ExtensionRequest::CloseSidePanel {
            window: WindowId(1),
            tab: None,
        };
        let outcome = || Outcome::Request {
            requests: vec![request.clone()],
            then: Then::Undefined,
        };
        let effects = state.outcome_effects(7, EXT, outcome());
        let [
            Effect::Request {
                token,
                extension,
                request: asked,
            },
        ] = &effects[..]
        else {
            panic!("{effects:?}");
        };
        assert_eq!((extension.as_str(), asked), (EXT, &request));
        assert_eq!(
            state.extension_reply(*token, ExtensionReply::Done, now),
            vec![Effect::Reply(7, Ok(Value::Null))]
        );
        let effects = state.outcome_effects(8, EXT, outcome());
        let [Effect::Request { token, .. }] = &effects[..] else {
            panic!("{effects:?}");
        };
        assert_eq!(
            state.extension_reply(*token, ExtensionReply::Refused("No window.".into()), now),
            vec![Effect::Reply(8, Err("No window.".into()))]
        );
        // An answer and a rejection pass straight through.
        assert_eq!(
            state.outcome_effects(9, EXT, Outcome::Answer(json!(false))),
            vec![Effect::Reply(9, Ok(json!(false)))]
        );
        assert_eq!(
            state.outcome_effects(10, EXT, Outcome::Reject("no".into())),
            vec![Effect::Reply(10, Err("no".into()))]
        );
    }

    /// What the panel's page (the page lane's key 9) is answered: whether
    /// it is a panel's.
    fn reported(panel: bool) -> Effect {
        Effect::EmitPage {
            page: 9,
            extension: EXT.to_string(),
            event: REPORTED_EVENT,
            args: vec![json!({ "panel": panel })],
        }
    }

    /// The word that lets the panel's page send its first message.
    fn told() -> Effect {
        Effect::EmitPage {
            page: 9,
            extension: EXT.to_string(),
            event: TOLD_EVENT,
            args: vec![],
        }
    }

    #[test]
    fn a_panels_chromium_id_is_told_to_its_extension_and_is_never_a_tab() {
        const PANELS: &str = "bridge.panels";
        let (mut state, now) = started(vec![tab(1, "a*")]);
        let panels_of = |state: &mut BridgeState, extension: &str| {
            let effects = state.call(call(4, extension, "bridge", "panels", json!([])));
            let [Effect::Reply(4, Ok(ids))] = &effects[..] else {
                panic!("{effects:?}");
            };
            ids.clone()
        };
        assert_eq!(panels_of(&mut state, EXT), json!([]));

        // A report for a view that is no open panel is refused: its page is
        // told it is no panel's, and holds nothing.
        assert_eq!(
            state.panel_reported(TabId(20), EXT, 433, 9, now),
            vec![reported(false)]
        );
        assert_eq!(panels_of(&mut state, EXT), json!([]));

        // The panel is open: its page's id is kept. Nobody listens yet, so
        // nobody is told (and no stopped worker is started for it; a context
        // that starts later asks): the page hears at once that it may send.
        state.set_panel_windows(HashMap::from([(TabId(20), WindowId(1))]));
        assert_eq!(
            state.panel_reported(TabId(20), EXT, 433, 9, now),
            vec![reported(true), told()]
        );
        assert_eq!(panels_of(&mut state, EXT), json!([433]));
        assert_eq!(panels_of(&mut state, OTHER), json!([]));

        // A second panel, with the worker and a page listening: both are
        // told every id of the extension's panels. The other extension's
        // listener hears nothing. The worker's delivery carries a token,
        // and the panel's page is not told to send until it is taken.
        listen(&mut state, EXT, PANELS);
        page_listen(&mut state, EXT, 5, PANELS);
        listen(&mut state, OTHER, PANELS);
        state.set_panel_windows(HashMap::from([
            (TabId(20), WindowId(1)),
            (TabId(21), WindowId(1)),
        ]));
        assert_eq!(
            state.panel_reported(TabId(21), EXT, 434, 9, now),
            vec![
                reported(true),
                Effect::EmitTold {
                    extension: EXT.to_string(),
                    event: PANELS,
                    args: vec![json!([433, 434])],
                    token: 1,
                },
                Effect::EmitPage {
                    page: 5,
                    extension: EXT.to_string(),
                    event: PANELS,
                    args: vec![json!([433, 434])],
                },
            ]
        );
        assert!(state.needs_tick());
        // The worker took the delivery: now the page is told, once.
        assert_eq!(state.told(1), vec![told()]);
        assert_eq!(state.told(1), vec![]);
        assert_eq!(state.told(77), vec![]);
        assert!(!state.needs_tick());
        // The same report again (the page reloaded): every context has the
        // id already, and the page may send at once.
        assert_eq!(
            state.panel_reported(TabId(21), EXT, 434, 9, now),
            vec![reported(true), told()]
        );

        // What `sender.tab` is given for a panel's id: the mark, for the
        // panel's own extension only; a tab's strip as before.
        assert_eq!(
            state.call(call(5, EXT, "tabs", "stripOf", json!([433]))),
            vec![Effect::Reply(5, Ok(json!({ "panel": true })))]
        );
        assert_eq!(
            state.call(call(6, OTHER, "tabs", "stripOf", json!([433]))),
            vec![Effect::Reply(6, Ok(Value::Null))]
        );
        let effects = state.call(call(7, EXT, "tabs", "stripOf", json!([101])));
        let [Effect::Reply(7, Ok(strip))] = &effects[..] else {
            panic!("{effects:?}");
        };
        assert_eq!(strip["index"], 0);
        assert_eq!(strip.get("panel"), None);

        // A page cannot make a strip tab a panel: the id of a tab the
        // strip holds is refused.
        state.set_panel_windows(HashMap::from([
            (TabId(20), WindowId(1)),
            (TabId(21), WindowId(1)),
            (TabId(22), WindowId(1)),
        ]));
        assert_eq!(
            state.panel_reported(TabId(22), EXT, 101, 9, now),
            vec![reported(false)]
        );
        assert_eq!(panels_of(&mut state, EXT), json!([433, 434]));

        // The panel in view 20 closed: its id goes. No table row was ever
        // made for a panel: the strip's tabs are as they were.
        state.set_panel_windows(HashMap::from([(TabId(21), WindowId(1))]));
        assert_eq!(panels_of(&mut state, EXT), json!([434]));
        assert_eq!(
            state.call(call(8, EXT, "tabs", "stripOf", json!([433]))),
            vec![Effect::Reply(8, Ok(Value::Null))]
        );
        assert_eq!(queried_ids(&mut state), vec![101]);
    }

    #[test]
    fn a_panels_page_is_told_to_send_only_after_its_worker_took_the_id() {
        const PANELS: &str = "bridge.panels";
        let (mut state, now) = started(vec![tab(1, "a*")]);
        listen(&mut state, EXT, PANELS);
        state.set_panel_windows(HashMap::from([
            (TabId(20), WindowId(1)),
            (TabId(21), WindowId(1)),
        ]));
        // Two panels report one after the other: each waits for its own
        // delivery, and each is answered first that it is a panel's.
        let first = state.panel_reported(TabId(20), EXT, 433, 9, now);
        assert_eq!(first[0], reported(true));
        assert!(matches!(first[1], Effect::EmitTold { token: 1, .. }));
        assert_eq!(
            first.len(),
            2,
            "the page was told before the worker took the id"
        );
        let second = state.panel_reported(TabId(21), EXT, 434, 10, now);
        assert!(matches!(second[1], Effect::EmitTold { token: 2, .. }));
        // The second delivery is taken first: only its page is told.
        assert_eq!(
            state.told(2),
            vec![Effect::EmitPage {
                page: 10,
                extension: EXT.to_string(),
                event: TOLD_EVENT,
                args: vec![],
            }]
        );
        assert!(state.needs_tick(), "the first page still waits");
        assert_eq!(state.told(1), vec![told()]);

        // A delivery nobody ever reports on (its relay went): the wait is
        // forgotten at the bound, with nothing sent: the page has sent its
        // message by then.
        let third = state.panel_reported(TabId(20), EXT, 435, 9, now);
        assert!(matches!(third[1], Effect::EmitTold { token: 3, .. }));
        assert_eq!(state.tick(&live, now + Duration::from_secs(1)), vec![]);
        assert!(state.needs_tick());
        assert_eq!(state.tick(&live, now + PUBLISH_WAIT), vec![]);
        assert!(!state.needs_tick());
        assert_eq!(state.told(3), vec![]);

        // An extension with no worker, and pages that listen: they are
        // told, and the panel's page may send at once.
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), false)]);
        state.strip_changed(
            snapshot(vec![window(1, true, vec![tab(1, "a*")])]),
            &live,
            now,
        );
        listen(&mut state, EXT, PANELS);
        page_listen(&mut state, EXT, 5, PANELS);
        state.set_panel_windows(HashMap::from([(TabId(20), WindowId(1))]));
        assert_eq!(
            state.panel_reported(TabId(20), EXT, 433, 9, now),
            vec![
                reported(true),
                Effect::EmitPage {
                    page: 5,
                    extension: EXT.to_string(),
                    event: PANELS,
                    args: vec![json!([433])],
                },
                told(),
            ]
        );
    }

    #[test]
    fn an_event_of_the_engines_goes_only_to_the_contexts_that_listen_for_it() {
        const OPENED: &str = "sidePanel.onOpened";
        let info = || vec![json!({ "windowId": 1, "path": "panel.html" })];
        let (mut state, _) = started(vec![tab(1, "a*")]);
        // Nobody listens: nothing is sent, so a stopped worker is not started.
        assert_eq!(state.emit(EXT, OPENED, info()), vec![]);
        // The worker listens for another event only.
        listen(&mut state, EXT, "sidePanel.onClosed");
        assert_eq!(state.emit(EXT, OPENED, info()), vec![]);
        // A page of the extension listens: that page, and not the worker.
        page_listen(&mut state, EXT, 5, OPENED);
        page_listen(&mut state, EXT, 6, "sidePanel.onClosed");
        page_listen(&mut state, OTHER, 7, OPENED);
        assert_eq!(
            state.emit(EXT, OPENED, info()),
            vec![Effect::EmitPage {
                page: 5,
                extension: EXT.to_string(),
                event: OPENED,
                args: info(),
            }]
        );
        // The worker listens too: the worker first, then the page.
        listen(&mut state, EXT, OPENED);
        assert_eq!(
            state.emit(EXT, OPENED, info()),
            vec![
                Effect::Emit {
                    extension: EXT.to_string(),
                    event: OPENED,
                    args: info(),
                },
                Effect::EmitPage {
                    page: 5,
                    extension: EXT.to_string(),
                    event: OPENED,
                    args: info(),
                },
            ]
        );
        // The page's frame went: its registration went with it.
        state.page_gone(&[5]);
        assert!(matches!(
            &state.emit(EXT, OPENED, info())[..],
            [Effect::Emit { .. }]
        ));
        // An extension served without a worker has no worker to send to,
        // whatever was registered under its name; its pages still hear.
        state.set_extensions(vec![(EXT.to_string(), all(), false)]);
        page_listen(&mut state, EXT, 8, OPENED);
        assert!(matches!(
            &state.emit(EXT, OPENED, info())[..],
            [Effect::EmitPage { page: 8, .. }]
        ));
        // An extension the bridge does not serve hears nothing.
        state.set_extensions(vec![]);
        assert_eq!(state.emit(EXT, OPENED, info()), vec![]);
    }

    #[test]
    fn a_command_goes_to_the_contexts_that_listen_with_the_tab_as_they_know_it() {
        const COMMAND: &str = "commands.onCommand";
        let (mut state, _) = started(vec![tab(1, "a*"), tab(2, "b")]);
        // Nobody listens: nothing is sent, so a stopped worker stays stopped.
        assert_eq!(state.command(EXT, "toggle-side-panel", TabId(1)), vec![]);
        listen(&mut state, EXT, COMMAND);
        page_listen(&mut state, EXT, 5, COMMAND);
        listen(&mut state, OTHER, COMMAND);
        let effects = state.command(EXT, "toggle-side-panel", TabId(2));
        let [
            Effect::Emit {
                extension,
                event: COMMAND,
                args,
            },
            Effect::EmitPage {
                page: 5,
                event: COMMAND,
                args: page_args,
                ..
            },
        ] = &effects[..]
        else {
            panic!("{effects:?}");
        };
        assert_eq!(extension, EXT);
        assert_eq!(args[0], json!("toggle-side-panel"));
        // The tab is the extension's: Chromium's id, the strip's place.
        assert_eq!(args[1]["id"], json!(102));
        assert_eq!(args[1]["index"], json!(1));
        assert_eq!(args[1]["windowId"], json!(1));
        assert_eq!(args[1]["url"], json!("https://b.example/"));
        assert_eq!(page_args, args);
        // An extension without the tabs permission is not told the address.
        let effects = state.command(OTHER, "toggle", TabId(2));
        let [Effect::Emit { args, .. }] = &effects[..] else {
            panic!("{effects:?}");
        };
        assert_eq!(args[1]["id"], json!(102));
        assert_eq!(args[1].get("url"), None);
        // A tab the strip does not hold: the command alone.
        let effects = state.command(EXT, "toggle-side-panel", TabId(99));
        let [Effect::Emit { args, .. }, Effect::EmitPage { .. }] = &effects[..] else {
            panic!("{effects:?}");
        };
        assert_eq!(args, &[json!("toggle-side-panel")]);
    }

    #[test]
    fn commands_get_all_answers_the_extensions_own_commands() {
        use keyboard_types::{Code, Modifiers};
        use osmena_core::{ExtensionCommand, KeyChord};

        use crate::bridge::ExtensionFacts;
        use crate::extensions::KeyPlatform;

        let (mut state, _) = started(vec![tab(1, "a*")]);
        let commands = vec![ExtensionCommand {
            name: "toggle-side-panel".to_string(),
            description: "Toggle the panel".to_string(),
            chord: Some(KeyChord::new(Modifiers::ALT, Code::KeyE)),
        }];
        state.set_facts(vec![(
            EXT.to_string(),
            ExtensionFacts {
                name: "Probe".to_string(),
                commands: commands.clone(),
            },
        )]);
        assert_eq!(
            state.call(call(1, EXT, "commands", "getAll", json!([]))),
            vec![Effect::Reply(
                1,
                Ok(crate::commands::get_all(&commands, KeyPlatform::CURRENT))
            )]
        );
        // Another extension has none of them.
        assert_eq!(
            state.call(call(2, OTHER, "commands", "getAll", json!([]))),
            vec![Effect::Reply(2, Ok(json!([])))]
        );
        // Nothing else of the namespace is served.
        assert_eq!(
            state.call(call(3, EXT, "commands", "reset", json!(["toggle"]))),
            vec![Effect::Reply(
                3,
                Err("commands.reset is not supported.".to_string())
            )]
        );
    }

    /// What the Claude extension does each time it opens its side panel,
    /// and what it does after, each call made straight after the one
    /// before resolved.
    #[test]
    fn a_panel_openings_group_calls_resolve_with_chromes_shapes_and_in_order() {
        use osmena_core::{Folder, FolderId};

        use crate::bridge::ExtensionFacts;

        let (mut state, now) = started(vec![tab(1, "a*"), tab(2, "b")]);
        state.set_facts(vec![(
            EXT.to_string(),
            ExtensionFacts {
                name: "Claude".to_string(),
                commands: Vec::new(),
            },
        )]);
        for event in [
            "tabGroups.onCreated",
            "tabGroups.onUpdated",
            "tabGroups.onRemoved",
            "tabs.onUpdated",
        ] {
            listen(&mut state, EXT, event);
        }
        let strip = |folder: Option<(&str, bool)>| {
            let mut first = tab(1, "a*");
            let mut one = window(1, true, vec![]);
            if let Some((name, expanded)) = folder {
                first.folder = Some(FolderId(7));
                one.folders = vec![Folder {
                    id: FolderId(7),
                    name: name.to_string(),
                    expanded,
                }];
            }
            one.tabs = vec![first, tab(2, "b")];
            snapshot(vec![one])
        };
        let group = |title: &str| {
            json!({"id": 7, "collapsed": false, "color": "grey", "title": title,
                   "windowId": 1, "shared": false})
        };
        let emitted = |effects: &[Effect]| -> Vec<String> {
            effects
                .iter()
                .map(|effect| match effect {
                    Effect::Emit { event, .. } => event.to_string(),
                    Effect::Reply(call, _) => format!("reply {call}"),
                    other => format!("{other:?}"),
                })
                .collect()
        };

        // tabs.group({tabIds: [tab], createProperties: {windowId}}).
        let effects = state.call(call(
            1,
            EXT,
            "tabs",
            "group",
            json!([{"tabIds": [101], "createProperties": {"windowId": 1}}]),
        ));
        assert_eq!(
            effects,
            vec![Effect::Request {
                token: 1,
                extension: EXT.to_string(),
                request: ExtensionRequest::GroupTabs {
                    tabs: vec![TabId(1)],
                    folder: None,
                },
            }]
        );
        // The model answers before the strip shows the folder, and a
        // republish of the old strip does not show it either.
        assert_eq!(
            state.extension_reply(1, ExtensionReply::Folder(FolderId(7)), now),
            vec![]
        );
        assert_eq!(state.table_changed(&live, now), vec![]);
        // The strip with the folder: the group exists, then the tab is in
        // it, then the promise resolves with the group's id.
        let effects = state.strip_changed(strip(Some(("Claude Session", true))), &live, now);
        assert_eq!(
            emitted(&effects),
            ["tabGroups.onCreated", "tabs.onUpdated", "reply 1"]
        );
        assert_eq!(effects[2], Effect::Reply(1, Ok(json!(7))));
        let Effect::Emit { args, .. } = &effects[0] else {
            panic!("{effects:?}");
        };
        assert_eq!(args, &[group("Claude Session")]);
        let Effect::Emit { args, .. } = &effects[1] else {
            panic!("{effects:?}");
        };
        assert_eq!((&args[0], &args[1]), (&json!(101), &json!({"groupId": 7})));
        assert_eq!(args[2]["groupId"], 7);
        // tabGroups.get(id) straight after sees it.
        assert_eq!(
            state.call(call(2, EXT, "tabGroups", "get", json!([7]))),
            vec![Effect::Reply(2, Ok(group("Claude Session")))]
        );
        // tabGroups.update(id, {title, color, collapsed: false}): the
        // colour is taken without a rejection and the group stays grey.
        let effects = state.call(call(
            3,
            EXT,
            "tabGroups",
            "update",
            json!([7, {"title": "Claude", "color": "orange", "collapsed": false}]),
        ));
        assert_eq!(
            effects,
            vec![Effect::Request {
                token: 2,
                extension: EXT.to_string(),
                request: ExtensionRequest::UpdateFolder {
                    folder: FolderId(7),
                    name: Some("Claude".to_string()),
                    expanded: Some(true),
                },
            }]
        );
        assert_eq!(state.extension_reply(2, ExtensionReply::Done, now), vec![]);
        let effects = state.strip_changed(strip(Some(("Claude", true))), &live, now);
        assert_eq!(emitted(&effects), ["tabGroups.onUpdated", "reply 3"]);
        assert_eq!(effects[1], Effect::Reply(3, Ok(group("Claude"))));
        // tabGroups.query({}) and tabs.query({groupId}).
        assert_eq!(
            state.call(call(4, EXT, "tabGroups", "query", json!([{}]))),
            vec![Effect::Reply(4, Ok(json!([group("Claude")])))]
        );
        let effects = state.call(call(5, EXT, "tabs", "query", json!([{"groupId": 7}])));
        let [Effect::Reply(5, Ok(Value::Array(tabs)))] = &effects[..] else {
            panic!("{effects:?}");
        };
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0]["id"], 101);
        // A blank title is the session name, from the extension's own.
        let effects = state.call(call(
            6,
            EXT,
            "tabGroups",
            "update",
            json!([7, {"title": ""}]),
        ));
        assert!(matches!(effects[..], [Effect::Request { token: 3, .. }]));
        assert_eq!(state.extension_reply(3, ExtensionReply::Done, now), vec![]);
        let effects = state.strip_changed(strip(Some(("Claude Session", true))), &live, now);
        assert_eq!(effects[1], Effect::Reply(6, Ok(group("Claude Session"))));
        // tabs.ungroup([tab]): the tab leaves, the emptied group goes, and
        // the promise resolves with nothing.
        let effects = state.call(call(7, EXT, "tabs", "ungroup", json!([[101]])));
        assert_eq!(
            effects,
            vec![Effect::Request {
                token: 4,
                extension: EXT.to_string(),
                request: ExtensionRequest::UngroupTabs(vec![TabId(1)]),
            }]
        );
        assert_eq!(state.extension_reply(4, ExtensionReply::Done, now), vec![]);
        let effects = state.strip_changed(strip(None), &live, now);
        assert_eq!(
            emitted(&effects),
            ["tabs.onUpdated", "tabGroups.onRemoved", "reply 7"]
        );
        assert_eq!(effects[2], Effect::Reply(7, Ok(Value::Null)));
        let Effect::Emit { args, .. } = &effects[0] else {
            panic!("{effects:?}");
        };
        assert_eq!(args[1], json!({"groupId": -1}));
        assert_eq!(
            state.call(call(8, EXT, "tabGroups", "get", json!([7]))),
            vec![Effect::Reply(8, Err("No group with id: 7.".to_string()))]
        );
        assert!(!state.needs_tick());
    }

    /// A read made straight after a group call resolves sees what the call
    /// changed: the promise waits for the strip that shows the change, not
    /// for any strip (the engine republishes the one it has whenever the
    /// tab-id table changes).
    #[test]
    fn a_group_call_resolves_on_the_strip_that_shows_its_own_change() {
        use osmena_core::{Folder, FolderId};

        let strip = |second_in: bool, name: &str| {
            let mut first = tab(1, "a*");
            first.folder = Some(FolderId(7));
            let mut second = tab(2, "b");
            if second_in {
                second.folder = Some(FolderId(7));
            }
            let mut one = window(1, true, vec![first, second]);
            one.folders = vec![Folder {
                id: FolderId(7),
                name: name.to_string(),
                expanded: true,
            }];
            snapshot(vec![one])
        };
        let mut state = BridgeState::default();
        state.set_extensions(vec![(EXT.to_string(), all(), true)]);
        let now = Instant::now();
        state.strip_changed(strip(false, "Work"), &live, now);
        let replies = |effects: &[Effect]| -> Vec<Effect> {
            effects
                .iter()
                .filter(|effect| matches!(effect, Effect::Reply(..)))
                .cloned()
                .collect()
        };
        let grouped = |state: &mut BridgeState| {
            let effects = state.call(call(9, EXT, "tabs", "query", json!([{"groupId": 7}])));
            let [Effect::Reply(9, Ok(Value::Array(tabs)))] = &effects[..] else {
                panic!("{effects:?}");
            };
            tabs.iter()
                .map(|t| t["id"].as_i64().unwrap())
                .collect::<Vec<_>>()
        };

        // tabs.group({tabIds, groupId}) into a group that exists: the
        // folder is on the strip already, the added tab is not in it yet.
        let effects = state.call(call(
            1,
            EXT,
            "tabs",
            "group",
            json!([{"tabIds": [102], "groupId": 7}]),
        ));
        assert!(matches!(effects[..], [Effect::Request { token: 1, .. }]));
        assert_eq!(
            state.extension_reply(1, ExtensionReply::Folder(FolderId(7)), now),
            vec![]
        );
        assert_eq!(replies(&state.table_changed(&live, now)), vec![]);
        let effects = state.strip_changed(strip(true, "Work"), &live, now);
        assert_eq!(replies(&effects), vec![Effect::Reply(1, Ok(json!(7)))]);
        assert_eq!(grouped(&mut state), [101, 102]);

        // tabGroups.update: a republish of the old strip is not the title.
        let effects = state.call(call(
            2,
            EXT,
            "tabGroups",
            "update",
            json!([7, {"title": "Play"}]),
        ));
        assert!(matches!(effects[..], [Effect::Request { token: 2, .. }]));
        assert_eq!(state.extension_reply(2, ExtensionReply::Done, now), vec![]);
        assert_eq!(replies(&state.table_changed(&live, now)), vec![]);
        let effects = state.strip_changed(strip(true, "Play"), &live, now);
        let [Effect::Reply(2, Ok(group))] = &replies(&effects)[..] else {
            panic!("{effects:?}");
        };
        assert_eq!(group["title"], "Play");
        assert_eq!(
            state.call(call(3, EXT, "tabGroups", "get", json!([7]))),
            vec![Effect::Reply(3, Ok(group.clone()))]
        );

        // tabs.ungroup: nor is it the tab out of its folder.
        let effects = state.call(call(4, EXT, "tabs", "ungroup", json!([102])));
        assert!(matches!(effects[..], [Effect::Request { token: 3, .. }]));
        assert_eq!(state.extension_reply(3, ExtensionReply::Done, now), vec![]);
        assert_eq!(replies(&state.table_changed(&live, now)), vec![]);
        assert_eq!(grouped(&mut state), [101, 102]);
        let effects = state.strip_changed(strip(false, "Play"), &live, now);
        assert_eq!(replies(&effects), vec![Effect::Reply(4, Ok(Value::Null))]);
        assert_eq!(grouped(&mut state), [101]);

        // A request that changes nothing the strip can show is not held
        // for the limit: an update to the title it has resolves at once.
        let effects = state.call(call(
            5,
            EXT,
            "tabGroups",
            "update",
            json!([7, {"title": "Play", "collapsed": false}]),
        ));
        assert!(matches!(effects[..], [Effect::Request { token: 4, .. }]));
        let effects = state.extension_reply(4, ExtensionReply::Done, now);
        assert!(matches!(&replies(&effects)[..], [Effect::Reply(5, Ok(_))]));
        assert!(!state.needs_tick());
    }
}
