//! What the bridge remembers between calls, with no CEF in it: the strip
//! as extensions were last told of it, the one waiting to be told, the
//! ids, each extension's grants and listeners, and the calls waiting on
//! the model. Every entry point returns the `Effect`s to carry out.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use osmena_core::{ExtensionReply, ExtensionRequest, TabId, WindowId};
use serde_json::Value;

use super::differ;
use super::ids::IdSpace;
use super::pattern::Grants;
use super::router::{self, Outcome, Settled, Then};
use super::shape::View;
use super::snapshot::Snapshot;
use super::transport::{Asker, BridgeCall, CallId};

/// How long a strip waits for Chromium's id of a new live tab before it
/// is published with a synthetic one. A page's call waits for the same
/// bound, for the tab it is in or for its extension to be served.
pub(crate) const PUBLISH_WAIT: Duration = Duration::from_secs(2);

/// The error a page's call gets when its extension is still not bridged
/// when the call's wait is over.
pub(crate) const UNBRIDGED: &str = "the extension is not bridged.";

/// What a page's call waits for before the engine answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hold {
    /// The tab the page is in is not in the published strip, and a strip is
    /// still pending.
    Tab(TabId),
    /// The page's extension is not served yet.
    Served,
    /// An earlier held call must go first, so that calls keep their order.
    Queued,
}

/// A page's call held until its `Hold` is met or the bound has passed.
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
    published: Snapshot,
    /// The newest strip, when it is not published yet, and since when.
    held: Option<(Snapshot, Instant)>,
    ids: IdSpace,
    grants: HashMap<String, Grants>,
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
    /// Page calls held for their tab or their extension, in arrival order.
    held_calls: Vec<HeldCall>,
    last_token: u64,
    last_focused: Option<WindowId>,
    seq: u64,
}

impl BridgeState {
    fn view(&self, extension: &str) -> View<'_> {
        View {
            snapshot: &self.published,
            ids: &self.ids,
            grants: self.grants.get(extension).unwrap_or(&NO_GRANTS),
            last_focused: self.last_focused,
        }
    }

    /// Whether `tick` has anything to wait for.
    pub(crate) fn needs_tick(&self) -> bool {
        self.held.is_some() || !self.waiting.is_empty() || !self.held_calls.is_empty()
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
                _ => Err(router::unsupported("bridge", &call.method)),
            };
            return vec![Effect::Reply(call.id, result)];
        }
        let outcome = router::route(&call, &self.view(&call.extension));
        match outcome {
            Outcome::Answer(value) => vec![Effect::Reply(call.id, Ok(value))],
            Outcome::Reject(message) => vec![Effect::Reply(call.id, Err(message))],
            Outcome::Forward {
                method,
                args,
                strip,
            } => vec![Effect::Reply(
                call.id,
                Ok(router::forward_value(&method, args, strip)),
            )],
            Outcome::Capture {
                tab,
                format,
                quality,
            } => vec![Effect::Capture {
                call: call.id,
                tab,
                format,
                quality,
            }],
            Outcome::Request { requests, then } => {
                self.request(call.id, &call.extension, requests, then)
            }
        }
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
            .any(|held| held.call.extension == call.extension)
        {
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
        // Held page calls, in arrival order within each extension: a call
        // goes once its hold is met or its bound has passed, and a later call
        // of the same extension never goes before it. Another extension's
        // held calls do not wait for it.
        let mut blocked: BTreeSet<String> = BTreeSet::new();
        let mut kept = Vec::new();
        for held in std::mem::take(&mut self.held_calls) {
            if blocked.contains(&held.call.extension) {
                kept.push(held);
                continue;
            }
            let ready = match held.hold {
                Hold::Queued => true,
                Hold::Served => self.serves(&held.call.extension),
                Hold::Tab(tab) => self.published.locate(tab).is_some() || self.held.is_none(),
            };
            let waited = now.saturating_duration_since(held.since);
            if !ready && waited < PUBLISH_WAIT {
                blocked.insert(held.call.extension.clone());
                kept.push(held);
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
    use serde_json::{Value, json};
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
            state.call(call(4, EXT, "tabs", "group", json!([{ "tabIds": [101] }]))),
            vec![Effect::Reply(
                4,
                Err("tabs.group is not supported.".to_string())
            )]
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
}
