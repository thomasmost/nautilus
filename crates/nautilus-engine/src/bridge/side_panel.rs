//! `chrome.sidePanel` and the side panel's part of
//! `chrome.runtime.getContexts`, answered by the engine: the options an
//! extension set, the route from a bridged call to an answer or an
//! `ExtensionRequest`, what an action click does when the extension
//! asked for its panel, and the panel's `ExtensionContext`. Pure: the
//! browser's state arrives through [`World`] and [`PanelRegistry`].
//!
//! The rules and the error texts are Chromium's `SidePanelService`
//! (`chrome/browser/extensions/api/side_panel/side_panel_service.cc`).

use std::collections::{BTreeMap, HashMap};

use osmena_core::{ExtensionRequest, TabId, WindowId};
use serde_json::{Value, json};

use super::ids::WINDOW_ID_CURRENT;
use super::router::{self, Then};
use crate::panel::{OpenPanel, PanelNotice, PanelRegistry, normalize_page};

/// `chrome.sidePanel.PanelOptions` without its tab.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PanelOptions {
    pub path: Option<String>,
    pub enabled: Option<bool>,
}

impl PanelOptions {
    /// A panel can be shown: it has a page and is enabled.
    pub(crate) fn active(&self) -> bool {
        self.path.is_some() && self.enabled == Some(true)
    }

    /// The page as the model carries it.
    fn page(&self) -> Option<String> {
        self.path.as_deref().map(normalize_page)
    }
}

#[derive(Debug, Default)]
struct ExtensionPanels {
    /// `side_panel.default_path` of the manifest.
    manifest_path: Option<String>,
    /// What `setOptions` without a tab stored, once it was called.
    default: Option<PanelOptions>,
    /// What `setOptions` stored for single tabs, by the tab id the
    /// extension used.
    tabs: HashMap<i64, PanelOptions>,
}

impl ExtensionPanels {
    fn manifest(&self) -> PanelOptions {
        match &self.manifest_path {
            Some(path) => PanelOptions {
                path: Some(path.clone()),
                enabled: Some(true),
            },
            None => PanelOptions::default(),
        }
    }
}

/// Every extension's side panel options and panel behaviour. The
/// options live for the run, as in Chromium (tab ids do not outlast
/// it); the behaviour is a preference and is written to disk.
#[derive(Debug, Default)]
pub(crate) struct SidePanelStore {
    extensions: HashMap<String, ExtensionPanels>,
    /// Extensions whose action click opens their panel.
    on_action_click: BTreeMap<String, bool>,
}

impl SidePanelStore {
    /// The manifest's default page of `extension`, from the listing.
    pub(crate) fn set_manifest_path(&mut self, extension: &str, path: Option<String>) {
        self.extensions
            .entry(extension.to_string())
            .or_default()
            .manifest_path = path;
    }

    /// The extension list as the manager last read it: an enabled
    /// extension has its manifest's page; one that is disabled or gone
    /// loses what it set, as when Chromium unloads it. The behaviours
    /// are preferences and stay (see [`SidePanelStore::uninstalled`]).
    pub(crate) fn listed(&mut self, listing: &[Listed]) {
        self.extensions
            .retain(|id, _| listing.iter().any(|l| l.enabled && l.id == *id));
        for listed in listing.iter().filter(|l| l.enabled) {
            self.set_manifest_path(&listed.id, listed.side_panel_path.clone());
        }
    }

    /// The user removed the extension: its behaviour goes with it.
    /// Returns whether there was one (and the file wants writing).
    pub(crate) fn uninstalled(&mut self, extension: &str) -> bool {
        self.extensions.remove(extension);
        self.on_action_click.remove(extension).is_some()
    }

    /// What `setOptions` stored for exactly this tab, if anything.
    pub(crate) fn specific(&self, extension: &str, tab: i64) -> Option<&PanelOptions> {
        self.extensions.get(extension)?.tabs.get(&tab)
    }

    /// The options in force for `tab`, or the default ones for `None`:
    /// the tab's own if it has any, else what `setOptions` stored
    /// without a tab, else the manifest's. The flag says the tab's own
    /// were used.
    pub(crate) fn options(&self, extension: &str, tab: Option<i64>) -> (PanelOptions, bool) {
        let Some(panels) = self.extensions.get(extension) else {
            return (PanelOptions::default(), false);
        };
        if let Some(own) = tab.and_then(|tab| panels.tabs.get(&tab)) {
            return (own.clone(), true);
        }
        let options = panels.default.clone().unwrap_or_else(|| panels.manifest());
        (options, false)
    }

    /// `setOptions`: a field that is given replaces the stored one. A
    /// first call for a place enables it unless it says otherwise, and a
    /// first call without a tab starts from the manifest's page.
    pub(crate) fn set_options(
        &mut self,
        extension: &str,
        tab: Option<i64>,
        path: Option<String>,
        enabled: Option<bool>,
    ) {
        let panels = self.extensions.entry(extension.to_string()).or_default();
        let existing = match tab {
            Some(tab) => panels.tabs.get_mut(&tab),
            None => panels.default.as_mut(),
        };
        if let Some(existing) = existing {
            if path.is_some() {
                existing.path = path;
            }
            if enabled.is_some() {
                existing.enabled = enabled;
            }
            return;
        }
        let enabled = Some(enabled.unwrap_or(true));
        match tab {
            Some(tab) => {
                panels.tabs.insert(tab, PanelOptions { path, enabled });
            }
            None => {
                let mut options = panels.manifest();
                if path.is_some() {
                    options.path = path;
                }
                options.enabled = enabled;
                panels.default = Some(options);
            }
        }
    }

    pub(crate) fn opens_on_action_click(&self, extension: &str) -> bool {
        self.on_action_click
            .get(extension)
            .copied()
            .unwrap_or(false)
    }

    /// Returns whether the stored behaviour changed (and wants saving).
    pub(crate) fn set_opens_on_action_click(&mut self, extension: &str, open: bool) -> bool {
        self.on_action_click.insert(extension.to_string(), open) != Some(open)
    }

    /// The behaviours as the profile's file holds them.
    pub(crate) fn behaviors_json(&self) -> String {
        serde_json::to_string_pretty(&self.on_action_click).unwrap_or_else(|_| "{}".to_string())
    }

    /// Read the file's text; anything unreadable is no behaviour.
    pub(crate) fn load_behaviors(&mut self, text: &str) {
        self.on_action_click = serde_json::from_str(text).unwrap_or_default();
    }
}

/// One extension of the manager's listing, as the store needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Listed {
    pub id: String,
    pub enabled: bool,
    /// The manifest's `side_panel.default_path`.
    pub side_panel_path: Option<String>,
}

/// The requests that close the panels of extensions that are no longer
/// enabled, each with the extension it is made for.
pub(crate) fn orphaned(
    panels: &PanelRegistry,
    listing: &[Listed],
) -> Vec<(String, ExtensionRequest)> {
    panels
        .all()
        .filter(|open| !listing.iter().any(|l| l.enabled && l.id == open.extension))
        .map(|open| {
            (
                open.extension.clone(),
                ExtensionRequest::CloseSidePanel {
                    window: open.window,
                    tab: open.tab,
                },
            )
        })
        .collect()
}

/// A tab as the model knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TabRef {
    pub window: WindowId,
    pub tab: TabId,
}

/// What the router needs of the browser: the last strip snapshot and
/// the tab-id table, behind the ids extensions use.
pub(crate) trait World {
    /// The tab an extension calls `id`.
    fn tab(&self, id: i64) -> Option<TabRef>;
    /// The id extensions use for `tab`; `None` while it has none (a
    /// restored tab not yet shown).
    fn tab_id(&self, tab: TabId) -> Option<i64>;
    /// The window an extension calls `id`.
    fn window(&self, id: i64) -> Option<WindowId>;
    /// The id extensions use for `window`.
    fn window_id(&self, window: WindowId) -> i64;
    fn window_of(&self, tab: TabId) -> Option<WindowId>;
    fn focused_window(&self) -> Option<WindowId>;
    /// The view of the panel `window` shows now, if it shows one.
    fn shown_panel(&self, window: WindowId) -> Option<TabId>;
}

/// One bridged call.
pub(crate) struct Call<'a> {
    pub extension: &'a str,
    /// The window the calling context is in: its tab's or its panel's.
    /// For the service worker, the window focused last
    /// (`View::current_window`), `None` only when no window exists.
    pub origin_window: Option<WindowId>,
    /// The view the call came from, when it came from a page.
    pub origin_view: Option<TabId>,
    pub method: &'a str,
    pub args: &'a [Value],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Outcome {
    /// Resolve the call with this value.
    Answer(Value),
    /// Ask the model; `Done` resolves the call with nothing, `Refused`
    /// rejects it with its text.
    Request(ExtensionRequest),
    Reject(String),
}

impl Outcome {
    /// The router's outcome for this one. A request's answer is
    /// `undefined` once the model is done (`Then::Undefined`).
    pub(crate) fn into_router(self) -> router::Outcome {
        match self {
            Outcome::Answer(value) => router::Outcome::Answer(value),
            Outcome::Reject(message) => router::Outcome::Reject(message),
            Outcome::Request(request) => router::Outcome::Request {
                requests: vec![request],
                then: Then::Undefined,
            },
        }
    }
}

/// What a call comes to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Routed {
    pub outcome: Outcome,
    /// Requests the call causes besides its own answer (a disabled
    /// panel closing); nobody waits for their replies.
    pub also: Vec<ExtensionRequest>,
    /// The panel behaviours changed and want writing.
    pub save: bool,
}

impl Routed {
    /// A call that comes to `value` and nothing else.
    pub(crate) fn answer(value: Value) -> Routed {
        Routed::only(Outcome::Answer(value))
    }

    fn only(outcome: Outcome) -> Routed {
        Routed {
            outcome,
            also: Vec::new(),
            save: false,
        }
    }

    fn reject(message: impl Into<String>) -> Routed {
        Routed::only(Outcome::Reject(message.into()))
    }
}

const NEITHER: &str = "At least one of `tabId` and `windowId` must be provided";
const OTHER_WINDOW: &str = "The specified tab does not belong to the specified window.";

fn no_tab(id: i64) -> String {
    format!("No tab with id: {id}.")
}

fn no_window(id: i64) -> String {
    format!("No window with id: {id}.")
}

fn int(options: &Value, key: &str) -> Option<i64> {
    options.get(key).and_then(Value::as_i64)
}

fn options_json(options: &PanelOptions, tab: Option<i64>) -> Value {
    let mut object = serde_json::Map::new();
    if let Some(enabled) = options.enabled {
        object.insert("enabled".to_string(), json!(enabled));
    }
    if let Some(path) = &options.path {
        object.insert("path".to_string(), json!(path));
    }
    if let Some(tab) = tab {
        object.insert("tabId".to_string(), json!(tab));
    }
    Value::Object(object)
}

/// The place an `open` or `close` names.
struct Place {
    window: WindowId,
    /// The tab, with the id the extension used for it.
    tab: Option<(i64, TabId)>,
}

fn place(world: &dyn World, call: &Call<'_>, options: &Value) -> Result<Place, String> {
    let tab = int(options, "tabId");
    let window = int(options, "windowId");
    if tab.is_none() && window.is_none() {
        return Err(NEITHER.to_string());
    }
    let window = match window {
        Some(WINDOW_ID_CURRENT) => Some(
            call.origin_window
                .or_else(|| world.focused_window())
                .ok_or_else(|| no_window(WINDOW_ID_CURRENT))?,
        ),
        Some(id) => Some(world.window(id).ok_or_else(|| no_window(id))?),
        None => None,
    };
    match tab {
        Some(id) => {
            let found = world.tab(id).ok_or_else(|| no_tab(id))?;
            if window.is_some_and(|window| window != found.window) {
                return Err(OTHER_WINDOW.to_string());
            }
            Ok(Place {
                window: found.window,
                tab: Some((id, found.tab)),
            })
        }
        None => Ok(Place {
            window: window.expect("a window when no tab was named"),
            tab: None,
        }),
    }
}

/// Whether `name` is a function of `chrome.sidePanel` the engine answers
/// (the script's `bridged` list; a test in `bridge/mod.rs` reads both).
#[cfg(test)]
pub(crate) fn is_method(name: &str) -> bool {
    matches!(
        name,
        "open"
            | "close"
            | "getOptions"
            | "setOptions"
            | "getPanelBehavior"
            | "setPanelBehavior"
            | "getLayout"
    )
}

/// `chrome.sidePanel.<method>` from an extension context.
pub(crate) fn route(
    store: &mut SidePanelStore,
    panels: &PanelRegistry,
    world: &dyn World,
    call: &Call<'_>,
) -> Routed {
    let extension = call.extension;
    let empty = json!({});
    let options = call.args.first().unwrap_or(&empty);
    match call.method {
        "getOptions" => {
            let tab = int(options, "tabId");
            let (found, own) = store.options(extension, tab);
            Routed::only(Outcome::Answer(options_json(&found, tab.filter(|_| own))))
        }
        "setOptions" => {
            let tab = int(options, "tabId");
            let path = options
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_string);
            let enabled = options.get("enabled").and_then(Value::as_bool);
            store.set_options(extension, tab, path, enabled);
            Routed {
                outcome: Outcome::Answer(Value::Null),
                also: after_set_options(store, panels, world, extension, tab),
                save: false,
            }
        }
        "getPanelBehavior" => Routed::only(Outcome::Answer(json!({
            "openPanelOnActionClick": store.opens_on_action_click(extension),
        }))),
        "setPanelBehavior" => {
            let save = match options
                .get("openPanelOnActionClick")
                .and_then(Value::as_bool)
            {
                Some(open) => store.set_opens_on_action_click(extension, open),
                None => false,
            };
            Routed {
                outcome: Outcome::Answer(Value::Null),
                also: Vec::new(),
                save,
            }
        }
        "open" => {
            let place = match place(world, call, options) {
                Ok(place) => place,
                Err(message) => return Routed::reject(message),
            };
            let request = match place.tab {
                Some((id, tab)) => {
                    let (found, own) = store.options(extension, Some(id));
                    let Some(page) = found.page().filter(|_| found.active()) else {
                        return Routed::reject(format!("No active side panel for tabId: {id}"));
                    };
                    ExtensionRequest::OpenSidePanel {
                        window: place.window,
                        // The tab's own panel when the tab has options
                        // of its own; the window's panel otherwise.
                        tab: own.then_some(tab),
                        page,
                    }
                }
                None => {
                    let (found, _) = store.options(extension, None);
                    let Some(page) = found.page().filter(|_| found.active()) else {
                        return Routed::reject(format!(
                            "No active side panel for windowId: {}",
                            world.window_id(place.window)
                        ));
                    };
                    ExtensionRequest::OpenSidePanel {
                        window: place.window,
                        tab: None,
                        page,
                    }
                }
            };
            Routed::only(Outcome::Request(request))
        }
        "close" => {
            let place = match place(world, call, options) {
                Ok(place) => place,
                Err(message) => return Routed::reject(message),
            };
            let tab = match place.tab {
                Some((id, tab)) => {
                    if !store
                        .specific(extension, id)
                        .is_some_and(PanelOptions::active)
                    {
                        return Routed::reject(format!(
                            "No active tab-specific side panel for tabId: {id}"
                        ));
                    }
                    Some(tab)
                }
                None => {
                    if !store.options(extension, None).0.active() {
                        return Routed::reject(format!(
                            "No active side panel for windowId: {}",
                            world.window_id(place.window)
                        ));
                    }
                    None
                }
            };
            // Only this extension's own panel is closed; with none open
            // there the call succeeds and changes nothing.
            match panels.at(extension, place.window, tab) {
                Some(_) => Routed::only(Outcome::Request(ExtensionRequest::CloseSidePanel {
                    window: place.window,
                    tab,
                })),
                None => Routed::only(Outcome::Answer(Value::Null)),
            }
        }
        // `window.close()` in an extension page, asked through the
        // bridge: a panel's page closes its panel; any other page is
        // told to close itself (`false`).
        "closeSelf" => {
            let own = call
                .origin_view
                .and_then(|view| panels.get(view))
                .filter(|open| open.extension == extension);
            match own {
                Some(open) => Routed::only(Outcome::Request(ExtensionRequest::CloseSidePanel {
                    window: open.window,
                    tab: open.tab,
                })),
                None => Routed::only(Outcome::Answer(json!(false))),
            }
        }
        // The panel docks on the right, always.
        "getLayout" => Routed::only(Outcome::Answer(json!({ "side": "right" }))),
        other => Routed::reject(format!(
            "chrome.sidePanel.{other} is not supported in this browser."
        )),
    }
}

/// What open panels do when their options change: a panel whose place
/// is no longer active closes, one whose page changed shows the new
/// page.
fn after_set_options(
    store: &SidePanelStore,
    panels: &PanelRegistry,
    world: &dyn World,
    extension: &str,
    tab: Option<i64>,
) -> Vec<ExtensionRequest> {
    let follow = |open: &OpenPanel, options: &PanelOptions| match options.page() {
        Some(page) if options.active() => {
            (page != open.page).then_some(ExtensionRequest::OpenSidePanel {
                window: open.window,
                tab: open.tab,
                page,
            })
        }
        _ => Some(ExtensionRequest::CloseSidePanel {
            window: open.window,
            tab: open.tab,
        }),
    };
    match tab {
        Some(id) => {
            let Some(found) = world.tab(id) else {
                return Vec::new();
            };
            let Some(own) = store.specific(extension, id) else {
                return Vec::new();
            };
            panels
                .at(extension, found.window, Some(found.tab))
                .and_then(|open| follow(open, own))
                .into_iter()
                .collect()
        }
        None => {
            let (default, _) = store.options(extension, None);
            panels
                .of_extension(extension)
                .filter(|open| open.tab.is_none())
                .filter_map(|open| follow(open, &default))
                .collect()
        }
    }
}

/// A click on `extension`'s toolbar action with `tab` active. `Some`
/// when the extension asked for its panel on a click
/// (`setPanelBehavior({openPanelOnActionClick: true})`) and has an
/// active panel for the tab: the request that opens it, or closes it
/// when the window already shows it. `None`: the click is the
/// extension's `action.onClicked`.
pub(crate) fn action_click(
    store: &SidePanelStore,
    panels: &PanelRegistry,
    world: &dyn World,
    extension: &str,
    tab: TabId,
) -> Option<ExtensionRequest> {
    if !store.opens_on_action_click(extension) {
        return None;
    }
    let window = world.window_of(tab)?;
    let (options, own) = store.options(extension, world.tab_id(tab));
    let page = options.page().filter(|_| options.active())?;
    let shown = world
        .shown_panel(window)
        .and_then(|view| panels.get(view))
        .filter(|open| open.extension == extension);
    Some(match shown {
        Some(open) => ExtensionRequest::CloseSidePanel {
            window,
            tab: open.tab,
        },
        None => ExtensionRequest::OpenSidePanel {
            window,
            tab: own.then_some(tab),
            page,
        },
    })
}

/// What a click on an extension's toolbar action does, in the order
/// Chromium decides it: the popup under the button if the action has one,
/// else the side panel when the extension asked for that and the tab has
/// an active panel, else `action.onClicked`, else nothing.
#[derive(Debug, PartialEq)]
pub(crate) enum ActionClick {
    Popup(String),
    Panel(ExtensionRequest),
    OnClicked,
    NotBridged,
}

pub(crate) fn decide_action_click(
    popup: Option<String>,
    panel: Option<ExtensionRequest>,
    bridged: bool,
) -> ActionClick {
    match (popup, panel, bridged) {
        (Some(url), _, _) => ActionClick::Popup(url),
        (None, Some(request), _) => ActionClick::Panel(request),
        (None, None, true) => ActionClick::OnClicked,
        (None, None, false) => ActionClick::NotBridged,
    }
}

/// A stable id for a panel's context, shaped like Chromium's (a UUID).
fn context_id(view: TabId) -> String {
    format!("00000000-0000-4000-8000-{:012x}", view.0)
}

/// `chrome.runtime.getContexts(filter)` for `extension`: `originals` is
/// what Chromium answered for no filter at all. Chromium knows nothing
/// of the panel as a side panel: it lists the panel's page as some
/// other kind of context, or not at all. Each open panel takes over the
/// entry Chromium has for its document (same URL, no strip tab behind
/// it, not the worker or an offscreen document) or gets one made; then
/// the filter is applied.
pub(crate) fn get_contexts(
    panels: &PanelRegistry,
    world: &dyn World,
    extension: &str,
    filter: &Value,
    originals: Vec<Value>,
) -> Vec<Value> {
    let mut contexts = originals;
    for open in panels.of_extension(extension) {
        let url = format!("chrome-extension://{extension}/{}", open.page);
        let window = world.window_id(open.window);
        let theirs = contexts.iter_mut().find(|context| {
            let kind = context.get("contextType").and_then(Value::as_str);
            let tab = context.get("tabId").and_then(Value::as_i64).unwrap_or(-1);
            context.get("documentUrl").and_then(Value::as_str) == Some(url.as_str())
                && !matches!(
                    kind,
                    Some("BACKGROUND" | "OFFSCREEN_DOCUMENT" | "SIDE_PANEL")
                )
                && world.tab(tab).is_none()
        });
        match theirs {
            Some(context) => {
                context["contextType"] = json!("SIDE_PANEL");
                context["tabId"] = json!(-1);
                context["windowId"] = json!(window);
            }
            None => contexts.push(json!({
                "contextId": context_id(open.view),
                "contextType": "SIDE_PANEL",
                "documentOrigin": format!("chrome-extension://{extension}"),
                "documentUrl": url,
                "frameId": 0,
                "incognito": false,
                "tabId": -1,
                "windowId": window,
            })),
        }
    }
    contexts.retain(|context| passes(context, filter));
    contexts
}

/// `runtime.ContextFilter`: every list that is given must hold the
/// context's value for its field.
fn passes(context: &Value, filter: &Value) -> bool {
    const LISTS: [(&str, &str); 8] = [
        ("contextIds", "contextId"),
        ("contextTypes", "contextType"),
        ("documentIds", "documentId"),
        ("documentOrigins", "documentOrigin"),
        ("documentUrls", "documentUrl"),
        ("frameIds", "frameId"),
        ("tabIds", "tabId"),
        ("windowIds", "windowId"),
    ];
    let lists =
        LISTS.iter().all(
            |(list, field)| match filter.get(*list).and_then(Value::as_array) {
                Some(wanted) => context.get(*field).is_some_and(|v| wanted.contains(v)),
                None => true,
            },
        );
    let incognito = match filter.get("incognito").and_then(Value::as_bool) {
        Some(wanted) => context.get("incognito").and_then(Value::as_bool) == Some(wanted),
        None => true,
    };
    lists && incognito
}

/// The `chrome.sidePanel` event a notice is: the extension it goes to,
/// the event's name and its one argument (`PanelOpenedInfo`,
/// `PanelClosedInfo`).
pub(crate) fn notice_event(
    world: &dyn World,
    notice: &PanelNotice,
) -> (String, &'static str, Value) {
    let (name, open) = match notice {
        PanelNotice::Opened(open) => ("sidePanel.onOpened", open),
        PanelNotice::Closed(open) => ("sidePanel.onClosed", open),
    };
    let mut info = serde_json::Map::new();
    info.insert("windowId".to_string(), json!(world.window_id(open.window)));
    info.insert("path".to_string(), json!(open.page));
    if let Some(tab) = open.tab.and_then(|tab| world.tab_id(tab)) {
        info.insert("tabId".to_string(), json!(tab));
    }
    (open.extension.clone(), name, Value::Object(info))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXT: &str = "abcdefghijklmnopabcdefghijklmnop";

    /// Two windows. Window 1 (extension id 11) holds tabs 1 and 2
    /// (extension ids 101, 102); window 2 (id 12) holds tab 3 (103) and
    /// tab 4, restored and not yet shown, which has no extension id.
    #[derive(Default)]
    struct Fake {
        shown: HashMap<WindowId, TabId>,
    }

    impl World for Fake {
        fn tab(&self, id: i64) -> Option<TabRef> {
            let (window, tab) = match id {
                101 => (1, 1),
                102 => (1, 2),
                103 => (2, 3),
                _ => return None,
            };
            Some(TabRef {
                window: WindowId(window),
                tab: TabId(tab),
            })
        }
        fn tab_id(&self, tab: TabId) -> Option<i64> {
            (1..=3).contains(&tab.0).then_some(100 + tab.0 as i64)
        }
        fn window(&self, id: i64) -> Option<WindowId> {
            matches!(id, 11 | 12).then_some(WindowId(id as u64 - 10))
        }
        fn window_id(&self, window: WindowId) -> i64 {
            window.0 as i64 + 10
        }
        fn window_of(&self, tab: TabId) -> Option<WindowId> {
            match tab.0 {
                1 | 2 => Some(WindowId(1)),
                3 | 4 => Some(WindowId(2)),
                _ => None,
            }
        }
        fn focused_window(&self) -> Option<WindowId> {
            Some(WindowId(2))
        }
        fn shown_panel(&self, window: WindowId) -> Option<TabId> {
            self.shown.get(&window).copied()
        }
    }

    fn store_with_manifest() -> SidePanelStore {
        let mut store = SidePanelStore::default();
        store.set_manifest_path(EXT, Some("panel.html".to_string()));
        store
    }

    fn call(
        store: &mut SidePanelStore,
        panels: &PanelRegistry,
        method: &str,
        options: Value,
    ) -> Routed {
        route(
            store,
            panels,
            &Fake::default(),
            &Call {
                extension: EXT,
                origin_window: None,
                origin_view: None,
                method,
                args: &[options],
            },
        )
    }

    fn answer(routed: Routed) -> Value {
        match routed.outcome {
            Outcome::Answer(value) => value,
            other => panic!("not an answer: {other:?}"),
        }
    }

    fn rejection(routed: Routed) -> String {
        match routed.outcome {
            Outcome::Reject(message) => message,
            other => panic!("not a rejection: {other:?}"),
        }
    }

    fn open_panel(
        panels: &mut PanelRegistry,
        view: u64,
        window: u64,
        tab: Option<u64>,
        page: &str,
    ) {
        let request = ExtensionRequest::OpenSidePanel {
            window: WindowId(window),
            tab: tab.map(TabId),
            page: page.to_string(),
        };
        panels.expect(view, EXT, &request);
        panels.bind(TabId(view), EXT, page).unwrap();
    }

    #[test]
    fn a_click_is_a_popup_then_a_panel_then_onclicked() {
        let url = "chrome-extension://abcdefghijklmnopabcdefghijklmnop/popup.html".to_string();
        let request = ExtensionRequest::OpenSidePanel {
            window: WindowId(1),
            tab: None,
            page: "panel.html".into(),
        };
        assert_eq!(
            decide_action_click(Some(url.clone()), Some(request.clone()), true),
            ActionClick::Popup(url)
        );
        assert_eq!(
            decide_action_click(None, Some(request.clone()), true),
            ActionClick::Panel(request)
        );
        assert_eq!(
            decide_action_click(None, None, true),
            ActionClick::OnClicked
        );
        assert_eq!(
            decide_action_click(None, None, false),
            ActionClick::NotBridged
        );
    }

    #[test]
    fn a_routed_request_is_the_routers_request_answered_with_nothing() {
        let request = ExtensionRequest::CloseSidePanel {
            window: WindowId(1),
            tab: None,
        };
        assert_eq!(
            Outcome::Request(request.clone()).into_router(),
            router::Outcome::Request {
                requests: vec![request],
                then: Then::Undefined,
            }
        );
        assert_eq!(
            Outcome::Answer(json!(false)).into_router(),
            router::Outcome::Answer(json!(false))
        );
        assert_eq!(
            Outcome::Reject("no".into()).into_router(),
            router::Outcome::Reject("no".into())
        );
        assert_eq!(
            Routed::answer(json!([])),
            Routed {
                outcome: Outcome::Answer(json!([])),
                also: Vec::new(),
                save: false,
            }
        );
    }

    #[test]
    fn options_fall_back_from_the_tab_to_the_default_to_the_manifest() {
        let mut store = store_with_manifest();
        let panels = PanelRegistry::default();
        // Nothing set: the manifest's page, enabled, for any tab.
        assert_eq!(
            answer(call(&mut store, &panels, "getOptions", json!({}))),
            json!({ "enabled": true, "path": "panel.html" })
        );
        assert_eq!(
            answer(call(
                &mut store,
                &panels,
                "getOptions",
                json!({ "tabId": 101 })
            )),
            json!({ "enabled": true, "path": "panel.html" })
        );
        // A tab's own options answer with its id; other tabs still fall
        // back.
        let set = call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "tabId": 101, "path": "panel.html?tab=101", "enabled": true }),
        );
        assert_eq!(answer(set), Value::Null);
        assert_eq!(
            answer(call(
                &mut store,
                &panels,
                "getOptions",
                json!({ "tabId": 101 })
            )),
            json!({ "enabled": true, "path": "panel.html?tab=101", "tabId": 101 })
        );
        assert_eq!(
            answer(call(
                &mut store,
                &panels,
                "getOptions",
                json!({ "tabId": 102 })
            )),
            json!({ "enabled": true, "path": "panel.html" })
        );
        // A default set later is what other tabs fall back to; a field
        // not given keeps what was there (the manifest's page).
        call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "enabled": false }),
        );
        assert_eq!(
            answer(call(
                &mut store,
                &panels,
                "getOptions",
                json!({ "tabId": 102 })
            )),
            json!({ "enabled": false, "path": "panel.html" })
        );
        call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "path": "other.html" }),
        );
        assert_eq!(
            answer(call(&mut store, &panels, "getOptions", json!({}))),
            json!({ "enabled": false, "path": "other.html" })
        );
    }

    #[test]
    fn a_first_set_for_a_tab_is_enabled_and_keeps_only_what_it_was_given() {
        // cookbook.sidepanel-site-specific: no manifest page at all.
        let mut store = SidePanelStore::default();
        store.set_manifest_path(EXT, None);
        let panels = PanelRegistry::default();
        assert_eq!(
            answer(call(
                &mut store,
                &panels,
                "getOptions",
                json!({ "tabId": 101 })
            )),
            json!({})
        );
        call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "tabId": 101, "enabled": false }),
        );
        assert_eq!(
            answer(call(
                &mut store,
                &panels,
                "getOptions",
                json!({ "tabId": 101 })
            )),
            json!({ "enabled": false, "tabId": 101 })
        );
        // The tab reaches the site: a page and enabled.
        call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "tabId": 101, "path": "sidepanel.html", "enabled": true }),
        );
        assert!(store.specific(EXT, 101).unwrap().active());
        // A first set with a page alone is enabled.
        call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "tabId": 102, "path": "sidepanel.html" }),
        );
        assert!(store.specific(EXT, 102).unwrap().active());
        // An extension nobody listed has no options.
        assert_eq!(
            store.options("unknown", Some(101)),
            (PanelOptions::default(), false)
        );
    }

    #[test]
    fn open_for_a_tab_with_its_own_options_is_that_tabs_panel() {
        let mut store = store_with_manifest();
        let panels = PanelRegistry::default();
        call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "tabId": 101, "path": "/panel.html?tab=101", "enabled": true }),
        );
        assert_eq!(
            call(&mut store, &panels, "open", json!({ "tabId": 101 })).outcome,
            Outcome::Request(ExtensionRequest::OpenSidePanel {
                window: WindowId(1),
                tab: Some(TabId(1)),
                page: "panel.html?tab=101".to_string(),
            })
        );
        // A tab without options of its own opens the window's panel.
        assert_eq!(
            call(&mut store, &panels, "open", json!({ "tabId": 102 })).outcome,
            Outcome::Request(ExtensionRequest::OpenSidePanel {
                window: WindowId(1),
                tab: None,
                page: "panel.html".to_string(),
            })
        );
    }

    #[test]
    fn open_for_a_window_is_the_default_panel() {
        let mut store = store_with_manifest();
        let panels = PanelRegistry::default();
        assert_eq!(
            call(&mut store, &panels, "open", json!({ "windowId": 12 })).outcome,
            Outcome::Request(ExtensionRequest::OpenSidePanel {
                window: WindowId(2),
                tab: None,
                page: "panel.html".to_string(),
            })
        );
        // The current window of the service worker is the focused one;
        // of a panel or a tab, its own.
        assert_eq!(
            call(&mut store, &panels, "open", json!({ "windowId": -2 })).outcome,
            Outcome::Request(ExtensionRequest::OpenSidePanel {
                window: WindowId(2),
                tab: None,
                page: "panel.html".to_string(),
            })
        );
        let from_window_1 = route(
            &mut store,
            &panels,
            &Fake::default(),
            &Call {
                extension: EXT,
                origin_window: Some(WindowId(1)),
                origin_view: Some(TabId(1)),
                method: "open",
                args: &[json!({ "windowId": -2 })],
            },
        );
        assert_eq!(
            from_window_1.outcome,
            Outcome::Request(ExtensionRequest::OpenSidePanel {
                window: WindowId(1),
                tab: None,
                page: "panel.html".to_string(),
            })
        );
    }

    #[test]
    fn open_rejects_with_chromiums_words() {
        let mut store = store_with_manifest();
        let panels = PanelRegistry::default();
        assert_eq!(
            rejection(call(&mut store, &panels, "open", json!({}))),
            "At least one of `tabId` and `windowId` must be provided"
        );
        assert_eq!(
            rejection(call(&mut store, &panels, "open", json!({ "tabId": 999 }))),
            "No tab with id: 999."
        );
        assert_eq!(
            rejection(call(
                &mut store,
                &panels,
                "open",
                json!({ "windowId": 999 })
            )),
            "No window with id: 999."
        );
        assert_eq!(
            rejection(call(
                &mut store,
                &panels,
                "open",
                json!({ "tabId": 101, "windowId": 12 })
            )),
            "The specified tab does not belong to the specified window."
        );
        call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "tabId": 101, "enabled": false }),
        );
        assert_eq!(
            rejection(call(&mut store, &panels, "open", json!({ "tabId": 101 }))),
            "No active side panel for tabId: 101"
        );
        call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "enabled": false }),
        );
        assert_eq!(
            rejection(call(&mut store, &panels, "open", json!({ "windowId": 11 }))),
            "No active side panel for windowId: 11"
        );
        assert_eq!(
            rejection(call(&mut store, &panels, "frobnicate", json!({}))),
            "chrome.sidePanel.frobnicate is not supported in this browser."
        );
    }

    #[test]
    fn disabling_an_open_panel_closes_it_and_a_new_page_replaces_it() {
        let mut store = store_with_manifest();
        let mut panels = PanelRegistry::default();
        call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "tabId": 101, "path": "panel.html?tab=101" }),
        );
        open_panel(&mut panels, 20, 1, None, "panel.html");
        open_panel(&mut panels, 21, 1, Some(1), "panel.html?tab=101");
        open_panel(&mut panels, 22, 2, None, "panel.html");

        // The same options again change nothing.
        let same = call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "tabId": 101, "enabled": true }),
        );
        assert!(same.also.is_empty());

        // The tab's panel follows its page, then closes when disabled;
        // the window's panel is not touched by a tab's options.
        let moved = call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "tabId": 101, "path": "panel.html?tab=101&v=2" }),
        );
        assert_eq!(
            moved.also,
            [ExtensionRequest::OpenSidePanel {
                window: WindowId(1),
                tab: Some(TabId(1)),
                page: "panel.html?tab=101&v=2".to_string(),
            }]
        );
        let disabled = call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "tabId": 101, "enabled": false }),
        );
        assert_eq!(answer(disabled.clone()), Value::Null);
        assert_eq!(
            disabled.also,
            [ExtensionRequest::CloseSidePanel {
                window: WindowId(1),
                tab: Some(TabId(1)),
            }]
        );

        // Disabling the default closes the window panel of every
        // window, and no tab's.
        let all = call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "enabled": false }),
        );
        assert_eq!(
            all.also,
            [
                ExtensionRequest::CloseSidePanel {
                    window: WindowId(1),
                    tab: None,
                },
                ExtensionRequest::CloseSidePanel {
                    window: WindowId(2),
                    tab: None,
                },
            ]
        );
        // Options for a tab nobody knows are kept and close nothing.
        let unknown = call(
            &mut store,
            &panels,
            "setOptions",
            json!({ "tabId": 999, "enabled": false }),
        );
        assert!(unknown.also.is_empty());
    }

    #[test]
    fn close_asks_only_for_this_extensions_open_panel() {
        let mut store = store_with_manifest();
        let mut panels = PanelRegistry::default();
        // Nothing open: the call succeeds and asks for nothing.
        assert_eq!(
            answer(call(
                &mut store,
                &panels,
                "close",
                json!({ "windowId": 11 })
            )),
            Value::Null
        );
        open_panel(&mut panels, 20, 1, None, "panel.html");
        assert_eq!(
            call(&mut store, &panels, "close", json!({ "windowId": 11 })).outcome,
            Outcome::Request(ExtensionRequest::CloseSidePanel {
                window: WindowId(1),
                tab: None,
            })
        );
        assert_eq!(
            rejection(call(&mut store, &panels, "close", json!({ "tabId": 101 }))),
            "No active tab-specific side panel for tabId: 101"
        );
        assert_eq!(
            rejection(call(&mut store, &panels, "close", json!({}))),
            "At least one of `tabId` and `windowId` must be provided"
        );
        assert_eq!(
            rejection(call(&mut store, &panels, "close", json!({ "tabId": 999 }))),
            "No tab with id: 999."
        );
    }

    #[test]
    fn window_close_in_a_panel_closes_that_panel_and_nothing_else() {
        let mut store = store_with_manifest();
        let mut panels = PanelRegistry::default();
        open_panel(&mut panels, 21, 1, Some(1), "panel.html");
        let from = |view: Option<u64>, extension: &'static str| Call {
            extension,
            origin_window: Some(WindowId(1)),
            origin_view: view.map(TabId),
            method: "closeSelf",
            args: &[],
        };
        let world = Fake::default();
        assert_eq!(
            route(&mut store, &panels, &world, &from(Some(21), EXT)).outcome,
            Outcome::Request(ExtensionRequest::CloseSidePanel {
                window: WindowId(1),
                tab: Some(TabId(1)),
            })
        );
        // An extension page in a tab, the worker, and another
        // extension's page close themselves.
        for call in [from(Some(1), EXT), from(None, EXT), from(Some(21), "other")] {
            assert_eq!(
                route(&mut store, &panels, &world, &call).outcome,
                Outcome::Answer(json!(false))
            );
        }
    }

    #[test]
    fn panels_of_extensions_that_are_off_or_gone_are_closed() {
        let mut panels = PanelRegistry::default();
        open_panel(&mut panels, 20, 1, None, "panel.html");
        open_panel(&mut panels, 21, 2, Some(3), "panel.html");
        let mut listing = vec![Listed {
            id: EXT.to_string(),
            enabled: true,
            side_panel_path: None,
        }];
        assert!(orphaned(&panels, &listing).is_empty());
        listing[0].enabled = false;
        let closes = vec![
            (
                EXT.to_string(),
                ExtensionRequest::CloseSidePanel {
                    window: WindowId(1),
                    tab: None,
                },
            ),
            (
                EXT.to_string(),
                ExtensionRequest::CloseSidePanel {
                    window: WindowId(2),
                    tab: Some(TabId(3)),
                },
            ),
        ];
        assert_eq!(orphaned(&panels, &listing), closes);
        assert_eq!(orphaned(&panels, &[]), closes);
    }

    #[test]
    fn the_panel_behaviour_is_kept_and_written() {
        let mut store = store_with_manifest();
        let panels = PanelRegistry::default();
        assert_eq!(
            answer(call(&mut store, &panels, "getPanelBehavior", json!({}))),
            json!({ "openPanelOnActionClick": false })
        );
        let set = call(
            &mut store,
            &panels,
            "setPanelBehavior",
            json!({ "openPanelOnActionClick": true }),
        );
        assert!(set.save);
        // The worker says it again at every start: nothing to write.
        let again = call(
            &mut store,
            &panels,
            "setPanelBehavior",
            json!({ "openPanelOnActionClick": true }),
        );
        assert!(!again.save);
        assert!(!call(&mut store, &panels, "setPanelBehavior", json!({})).save);

        let text = store.behaviors_json();
        let mut next_run = SidePanelStore::default();
        next_run.load_behaviors(&text);
        assert!(next_run.opens_on_action_click(EXT));
        next_run.load_behaviors("not json");
        assert!(!next_run.opens_on_action_click(EXT));

        // Disabling the extension drops its options, not the behaviour;
        // removing it drops both.
        let mut listing = [Listed {
            id: EXT.to_string(),
            enabled: false,
            side_panel_path: Some("panel.html".to_string()),
        }];
        store.listed(&listing);
        assert!(store.opens_on_action_click(EXT));
        assert_eq!(store.options(EXT, None), (PanelOptions::default(), false));
        listing[0].enabled = true;
        store.listed(&listing);
        assert!(store.options(EXT, None).0.active());
        assert!(store.uninstalled(EXT));
        assert!(!store.uninstalled(EXT));
        assert!(!store.opens_on_action_click(EXT));
        assert_eq!(store.options(EXT, None), (PanelOptions::default(), false));
        assert_eq!(
            answer(call(&mut store, &panels, "getLayout", json!({}))),
            json!({ "side": "right" })
        );
    }

    #[test]
    fn an_action_click_opens_the_panel_only_when_the_extension_asked() {
        let mut store = store_with_manifest();
        let mut panels = PanelRegistry::default();
        let world = Fake::default();
        assert_eq!(action_click(&store, &panels, &world, EXT, TabId(1)), None);

        store.set_opens_on_action_click(EXT, true);
        assert_eq!(
            action_click(&store, &panels, &world, EXT, TabId(1)),
            Some(ExtensionRequest::OpenSidePanel {
                window: WindowId(1),
                tab: None,
                page: "panel.html".to_string(),
            })
        );
        // A restored tab without an extension id has the default panel.
        assert_eq!(
            action_click(&store, &panels, &world, EXT, TabId(4)),
            Some(ExtensionRequest::OpenSidePanel {
                window: WindowId(2),
                tab: None,
                page: "panel.html".to_string(),
            })
        );

        // The window shows this extension's panel: the click closes it.
        open_panel(&mut panels, 20, 1, None, "panel.html");
        let showing = Fake {
            shown: HashMap::from([(WindowId(1), TabId(20))]),
        };
        assert_eq!(
            action_click(&store, &panels, &showing, EXT, TabId(1)),
            Some(ExtensionRequest::CloseSidePanel {
                window: WindowId(1),
                tab: None,
            })
        );
        // Another extension's click, with this one's panel showing,
        // opens its own.
        store.set_manifest_path("other", Some("o.html".to_string()));
        store.set_opens_on_action_click("other", true);
        assert_eq!(
            action_click(&store, &panels, &showing, "other", TabId(1)),
            Some(ExtensionRequest::OpenSidePanel {
                window: WindowId(1),
                tab: None,
                page: "o.html".to_string(),
            })
        );
    }

    #[test]
    fn a_click_on_a_tab_with_its_own_options_follows_them() {
        // cookbook.sidepanel-site-specific.
        let mut store = SidePanelStore::default();
        store.set_manifest_path(EXT, None);
        store.set_opens_on_action_click(EXT, true);
        store.set_options(
            EXT,
            Some(101),
            Some("sidepanel.html".to_string()),
            Some(true),
        );
        store.set_options(EXT, Some(102), None, Some(false));
        let panels = PanelRegistry::default();
        let world = Fake::default();
        assert_eq!(
            action_click(&store, &panels, &world, EXT, TabId(1)),
            Some(ExtensionRequest::OpenSidePanel {
                window: WindowId(1),
                tab: Some(TabId(1)),
                page: "sidepanel.html".to_string(),
            })
        );
        // Disabled for this tab, and no page for a tab never set: the
        // click is the extension's `onClicked`.
        assert_eq!(action_click(&store, &panels, &world, EXT, TabId(2)), None);
        assert_eq!(action_click(&store, &panels, &world, EXT, TabId(3)), None);
    }

    #[test]
    fn get_contexts_reports_an_open_panel_as_a_side_panel() {
        let mut panels = PanelRegistry::default();
        open_panel(&mut panels, 20, 1, None, "panel.html");
        let world = Fake::default();
        let worker = json!({
            "contextId": "712f4315-10fb-4d66-83dc-032ae525dc64",
            "contextType": "BACKGROUND",
            "frameId": -1, "incognito": false, "tabId": -1, "windowId": -1
        });
        let panel_url = format!("chrome-extension://{EXT}/panel.html");

        // Chromium does not list the panel's page: one is made.
        let made = get_contexts(&panels, &world, EXT, &json!({}), vec![worker.clone()]);
        assert_eq!(
            made,
            [
                worker.clone(),
                json!({
                    "contextId": "00000000-0000-4000-8000-000000000014",
                    "contextType": "SIDE_PANEL",
                    "documentOrigin": format!("chrome-extension://{EXT}"),
                    "documentUrl": panel_url,
                    "frameId": 0, "incognito": false, "tabId": -1, "windowId": 11
                }),
            ]
        );

        // Chromium lists it as a tab nobody has in a strip: that entry
        // becomes the panel's and keeps its ids. The same page open in
        // a real tab (101) is left alone.
        let as_tab = json!({
            "contextId": "aaaaaaaa-0000-4000-8000-000000000001",
            "contextType": "TAB", "documentId": "D1",
            "documentOrigin": format!("chrome-extension://{EXT}"),
            "documentUrl": panel_url,
            "frameId": 0, "incognito": false, "tabId": 555, "windowId": -1
        });
        let mut real_tab = as_tab.clone();
        real_tab["tabId"] = json!(101);
        real_tab["contextId"] = json!("bbbbbbbb-0000-4000-8000-000000000002");
        let taken = get_contexts(
            &panels,
            &world,
            EXT,
            &json!({ "contextTypes": ["SIDE_PANEL"] }),
            vec![worker.clone(), real_tab.clone(), as_tab],
        );
        assert_eq!(
            taken,
            [json!({
                "contextId": "aaaaaaaa-0000-4000-8000-000000000001",
                "contextType": "SIDE_PANEL", "documentId": "D1",
                "documentOrigin": format!("chrome-extension://{EXT}"),
                "documentUrl": panel_url,
                "frameId": 0, "incognito": false, "tabId": -1, "windowId": 11
            })]
        );

        // No panel open: Chromium's answer, filtered.
        let none = get_contexts(
            &PanelRegistry::default(),
            &world,
            EXT,
            &json!({ "contextTypes": ["SIDE_PANEL"] }),
            vec![worker.clone()],
        );
        assert!(none.is_empty());
    }

    #[test]
    fn the_context_filter_narrows_by_every_field_given() {
        let mut panels = PanelRegistry::default();
        open_panel(&mut panels, 20, 1, None, "panel.html");
        open_panel(&mut panels, 21, 2, None, "panel.html");
        let world = Fake::default();
        let count = |filter: Value| get_contexts(&panels, &world, EXT, &filter, Vec::new()).len();
        assert_eq!(count(json!({})), 2);
        assert_eq!(count(json!({ "windowIds": [12] })), 1);
        assert_eq!(count(json!({ "windowIds": [11, 12], "tabIds": [-1] })), 2);
        assert_eq!(count(json!({ "tabIds": [101] })), 0);
        assert_eq!(count(json!({ "contextTypes": ["TAB", "POPUP"] })), 0);
        assert_eq!(count(json!({ "frameIds": [0], "incognito": false })), 2);
        assert_eq!(count(json!({ "incognito": true })), 0);
        assert_eq!(
            count(json!({ "documentUrls": [format!("chrome-extension://{EXT}/panel.html")] })),
            2
        );
        assert_eq!(
            count(json!({ "contextIds": ["00000000-0000-4000-8000-000000000015"] })),
            1
        );
        // The made entry has no document id to match.
        assert_eq!(count(json!({ "documentIds": ["D1"] })), 0);
    }

    #[test]
    fn a_notice_is_the_side_panel_event_for_its_extension() {
        let world = Fake::default();
        let tab_panel = OpenPanel {
            view: TabId(21),
            extension: EXT.to_string(),
            page: "panel.html?tab=101".to_string(),
            window: WindowId(1),
            tab: Some(TabId(1)),
        };
        assert_eq!(
            notice_event(&world, &PanelNotice::Opened(tab_panel.clone())),
            (
                EXT.to_string(),
                "sidePanel.onOpened",
                json!({ "windowId": 11, "tabId": 101, "path": "panel.html?tab=101" })
            )
        );
        let window_panel = OpenPanel {
            tab: None,
            page: "panel.html".to_string(),
            ..tab_panel
        };
        assert_eq!(
            notice_event(&world, &PanelNotice::Closed(window_panel)),
            (
                EXT.to_string(),
                "sidePanel.onClosed",
                json!({ "windowId": 11, "path": "panel.html" })
            )
        );
    }
}
