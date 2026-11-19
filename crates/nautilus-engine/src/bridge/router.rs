//! A bridge call to what the engine does about it: an answer from the
//! snapshot, a request of the model with the shape of its eventual
//! answer, a call of Chromium's own function, or a rejection. Pure: it
//! reads a `View` and returns data, so it is tested without CEF and does
//! not depend on how calls travel.
//!
//! It fails closed: a namespace or method it does not name rejects, so
//! nothing reaches Chromium's own tab-strip code by falling through.

use std::time::Duration;

use osmena_core::{ExtensionReply, ExtensionRequest, TabId, WindowId};
use serde_json::{Map, Value, json};
use url::Url;

use super::ids::{IdSpace, TAB_ID_NONE, WINDOW_ID_CURRENT, group_id, window_from, window_id};
use super::pattern::{glob, matches_url};
use super::shape::{View, status};
use super::snapshot::{SnapTab, SnapWindow};
use super::transport::{Asker, BridgeCall};

/// A continuation settles on its own this long after the model's reply
/// when no new strip arrived (the request changed nothing).
pub(crate) const SETTLE_QUIET: Duration = Duration::from_millis(300);
/// A continuation that is still waiting for the strip after this long
/// answers with what there is, or gives up.
pub(crate) const SETTLE_LIMIT: Duration = Duration::from_secs(5);

const NO_CURRENT_WINDOW: &str = "No current window";
const NO_LAST_FOCUSED_WINDOW: &str = "No last-focused window";
const NO_SELECTED_TAB: &str = "No selected tab";
const UNEXPECTED_REPLY: &str = "The browser answered something else.";
const JAVASCRIPT_URL: &str = "JavaScript URLs are not allowed in API based extension navigations. Use chrome.scripting.executeScript instead.";
const KILL_URL: &str = "I'm sorry. I'm afraid I can't do that.";
/// `chrome://` hosts that crash or stop the browser or a renderer.
const KILL_HOSTS: [&str; 11] = [
    "crash",
    "kill",
    "hang",
    "shorthang",
    "gpucrash",
    "gpuhang",
    "gpuclean",
    "memory-exhaust",
    "inducebrowsercrashforrealz",
    "quit",
    "restart",
];
const QUERY_KEYS: [&str; 18] = [
    "active",
    "audible",
    "autoDiscardable",
    "currentWindow",
    "discarded",
    "frozen",
    "groupId",
    "highlighted",
    "index",
    "lastFocusedWindow",
    "muted",
    "pinned",
    "splitViewId",
    "status",
    "title",
    "url",
    "windowId",
    "windowType",
];

pub(crate) fn no_tab(id: i64) -> String {
    format!("No tab with id: {id}.")
}

pub(crate) fn no_window(id: i64) -> String {
    format!("No window with id: {id}.")
}

/// A refusal of the model's, in the ids the extension uses. osmena words
/// a refusal with its own ids (`No tab with id: 7.` is `TabId(7)`), and an
/// extension knows that tab by Chromium's id or a synthetic one, so the
/// tab's number is put back as the extension's. Window and group ids are
/// the same numbers on both sides and pass through, as does any other
/// sentence. The router checks every id against the published strip
/// before it emits a request, so a refusal is rare: the tab went between
/// the call and the model's answer. The reply comes before the strip
/// that drops the tab, so the id space still holds it; if it does not,
/// the sentence names no id rather than the model's.
pub(crate) fn refusal(message: &str, ids: &IdSpace) -> String {
    let model_id = message
        .strip_prefix("No tab with id: ")
        .and_then(|rest| rest.strip_suffix('.'))
        .and_then(|id| id.parse::<u64>().ok());
    match model_id {
        Some(id) => ids
            .ext(TabId(id))
            .map_or_else(|| "The tab no longer exists.".to_string(), no_tab),
        None => message.to_string(),
    }
}

pub(crate) fn unsupported(namespace: &str, method: &str) -> String {
    format!("{namespace}.{method} is not supported.")
}

fn not_loaded(id: i64) -> String {
    format!("The tab with id {id} is not loaded.")
}

fn invalid(what: &str) -> String {
    format!("Invalid value for {what}.")
}

/// What to do about a call.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Outcome {
    /// Resolve with this (`null` is `undefined`).
    Answer(Value),
    /// Reject with this message.
    Reject(String),
    /// Ask the model, in order; `then` shapes the last reply into the
    /// answer.
    Request {
        requests: Vec<ExtensionRequest>,
        then: Then,
    },
    /// Have the script call Chromium's `method` of the same namespace
    /// with `args` and lay `strip` (by tab id) over the tabs it returns.
    Forward {
        method: String,
        args: Vec<Value>,
        strip: Value,
    },
    /// Screenshot `tab`'s browser and resolve with a data URL.
    Capture {
        tab: TabId,
        format: String,
        quality: i64,
    },
}

/// How a model's reply becomes the value a promise resolves with.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Then {
    /// `undefined`, once the published strip no longer holds `tabs`, so
    /// that a query straight after the promise does not list them.
    Removed { tabs: Vec<TabId> },
    /// The reply names a tab: its object, once the published strip has it.
    CreatedTab,
    /// Once `tab` is active and live, Chromium's `update(id, rest)`, or
    /// `get(id)` when `rest` is empty.
    Native { tab: TabId, rest: Value },
    /// The objects of `tabs` (one object when `single`) from the next
    /// strip.
    Tabs { tabs: Vec<TabId>, single: bool },
    /// The reply names a window: open `more` in it as tabs, then its
    /// populated object.
    CreatedWindow { more: Vec<Url> },
    /// `window`'s object from the next strip.
    Window { window: WindowId, populate: bool },
}

/// Where a continuation stands.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Settled {
    Now(Result<Value, String>),
    /// Ask again after the next publish or tick.
    Wait,
    /// Ask the model for more, then settle `then` on the last reply.
    More {
        requests: Vec<ExtensionRequest>,
        then: Then,
    },
}

/// The value a forward travels as: an ordinary resolved value with one
/// reserved key, so the transport needs no third kind of reply.
pub(crate) fn forward_value(method: &str, args: Vec<Value>, strip: Value) -> Value {
    json!({ "__forward": { "method": method, "args": args, "strip": strip } })
}

/// Decide what to do about `call`.
pub(crate) fn route(call: &BridgeCall, view: &View) -> Outcome {
    let result = match call.namespace.as_str() {
        "tabs" => tabs(call, view),
        "windows" => windows(call, view),
        other => Err(unsupported(other, &call.method)),
    };
    result.unwrap_or_else(Outcome::Reject)
}

/// A tab the model has, as a call named it.
struct Found<'a> {
    window: &'a SnapWindow,
    index: usize,
    tab: &'a SnapTab,
    /// Its id as extensions see it now.
    ext: i64,
}

impl<'a> Found<'a> {
    fn at(view: &View<'a>, window: &'a SnapWindow, index: usize) -> Found<'a> {
        let tab = &window.tabs[index];
        Found {
            window,
            index,
            tab,
            ext: view.ids.ext(tab.id).unwrap_or(TAB_ID_NONE),
        }
    }
}

fn find<'a>(view: &View<'a>, id: i64) -> Result<Found<'a>, String> {
    let tab = view.ids.tab(id).ok_or_else(|| no_tab(id))?;
    let (window, index) = view.snapshot.locate(tab).ok_or_else(|| no_tab(id))?;
    Ok(Found::at(view, window, index))
}

/// The tab a call means: the one it names, else the active tab of the
/// asker's current window.
fn target<'a>(view: &View<'a>, asker: &Asker, id: Option<i64>) -> Result<Found<'a>, String> {
    if let Some(id) = id {
        return find(view, id);
    }
    let window = view.current_window(asker).ok_or(NO_CURRENT_WINDOW)?;
    let index = window
        .tabs
        .iter()
        .position(|tab| tab.active)
        .ok_or(NO_SELECTED_TAB)?;
    Ok(Found::at(view, window, index))
}

fn window_for<'a>(view: &View<'a>, asker: &Asker, id: i64) -> Result<&'a SnapWindow, String> {
    if id == WINDOW_ID_CURRENT {
        return view
            .current_window(asker)
            .ok_or_else(|| NO_CURRENT_WINDOW.to_string());
    }
    window_from(id)
        .and_then(|window| view.snapshot.window(window))
        .ok_or_else(|| no_window(id))
}

fn forward_live(view: &View, found: &Found, method: &str, rest: &[Value]) -> Outcome {
    let mut args = vec![json!(found.ext)];
    args.extend(rest.iter().cloned());
    let mut strip = Map::new();
    strip.insert(
        found.ext.to_string(),
        view.strip_json(found.window, found.index),
    );
    Outcome::Forward {
        method: method.to_string(),
        args,
        strip: Value::Object(strip),
    }
}

/// Chromium's `method` on a tab, which must have a browser.
fn forward(view: &View, found: &Found, method: &str, rest: &[Value]) -> Result<Outcome, String> {
    if view.ids.is_live(found.tab.id) {
        Ok(forward_live(view, found, method, rest))
    } else {
        Err(not_loaded(found.ext))
    }
}

/// A tab's object: Chromium's when it has a browser, the strip's
/// otherwise.
fn get(view: &View, found: &Found) -> Outcome {
    if view.ids.is_live(found.tab.id) {
        forward_live(view, found, "get", &[])
    } else {
        Outcome::Answer(view.tab_json(found.window, found.index))
    }
}

fn id_list(value: Option<&Value>) -> Result<Vec<i64>, String> {
    match value {
        Some(Value::Number(one)) => one
            .as_i64()
            .map(|id| vec![id])
            .ok_or_else(|| invalid("tabIds")),
        Some(Value::Array(many)) => many
            .iter()
            .map(|id| id.as_i64().ok_or_else(|| invalid("tabIds")))
            .collect(),
        _ => Err(invalid("tabIds")),
    }
}

/// An address an extension may send a tab to, as Chromium decides it: a
/// relative one is the extension's own page.
pub(crate) fn navigable(extension: &str, text: &str) -> Result<Url, String> {
    let invalid_url = || format!("Invalid url: \"{text}\".");
    let url = match Url::parse(text) {
        Ok(url) => url,
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            Url::parse(&format!("chrome-extension://{extension}/"))
                .and_then(|base| base.join(text))
                .map_err(|_| invalid_url())?
        }
        Err(_) => return Err(invalid_url()),
    };
    match url.scheme() {
        "javascript" => Err(JAVASCRIPT_URL.to_string()),
        "devtools" => Err("Cannot navigate to a devtools:// page.".to_string()),
        "chrome-untrusted" => Err("Cannot navigate to a chrome-untrusted:// page.".to_string()),
        "file" => Err("Cannot navigate to a file URL without local file access.".to_string()),
        "chrome"
            if url
                .host_str()
                .is_some_and(|host| KILL_HOSTS.contains(&host)) =>
        {
            Err(KILL_URL.to_string())
        }
        _ => Ok(url),
    }
}

fn tabs(call: &BridgeCall, view: &View) -> Result<Outcome, String> {
    let args = &call.args;
    let arg = |index: usize| args.get(index).filter(|value| !value.is_null());
    let tab_id = |index: usize| {
        arg(index)
            .and_then(Value::as_i64)
            .ok_or_else(|| invalid("tabId"))
    };
    match call.method.as_str() {
        "query" => query(view, &call.asker, arg(0)),
        "stripOf" => Ok(Outcome::Answer(
            arg(0)
                .and_then(Value::as_i64)
                .and_then(|id| find(view, id).ok())
                .map_or(Value::Null, |found| {
                    view.strip_json(found.window, found.index)
                }),
        )),
        "get" => Ok(get(view, &find(view, tab_id(0)?)?)),
        "getCurrent" => Ok(match &call.asker {
            Asker::View(tab) => match view.snapshot.locate(*tab) {
                Some((window, index)) => get(view, &Found::at(view, window, index)),
                None => Outcome::Answer(Value::Null),
            },
            Asker::Worker | Asker::Popup { .. } => Outcome::Answer(Value::Null),
        }),
        "create" => create(call, view, arg(0)),
        "duplicate" => {
            let found = find(view, tab_id(0)?)?;
            let url = Url::parse(&found.tab.url)
                .map_err(|_| format!("Invalid url: \"{}\".", found.tab.url))?;
            Ok(Outcome::Request {
                requests: vec![ExtensionRequest::OpenTab {
                    window: Some(found.window.id),
                    url,
                    active: true,
                }],
                then: Then::CreatedTab,
            })
        }
        "remove" => {
            let mut closing = Vec::new();
            for id in id_list(arg(0))? {
                closing.push(find(view, id)?.tab.id);
            }
            if closing.is_empty() {
                return Ok(Outcome::Answer(Value::Null));
            }
            Ok(Outcome::Request {
                requests: vec![ExtensionRequest::CloseTabs(closing.clone())],
                then: Then::Removed { tabs: closing },
            })
        }
        "update" => update(call, view),
        "move" => move_tabs(call, view),
        "highlight" => highlight(call, view, arg(0)),
        // Chromium's discard crashes on these tabs. Answer as Chrome does
        // for a tab it chose not to discard.
        "discard" => {
            if let Some(id) = arg(0).and_then(Value::as_i64) {
                find(view, id)?;
            }
            Ok(Outcome::Answer(Value::Null))
        }
        "captureVisibleTab" => capture(call, view),
        // `setZoom(zoomFactor)` or `setZoom(tabId, zoomFactor)`: one
        // number alone is the factor.
        "setZoom" => {
            let (id, rest) = if args.len() >= 2 {
                (args[0].as_i64(), &args[1..])
            } else {
                (None, &args[..])
            };
            forward(view, &target(view, &call.asker, id)?, "setZoom", rest)
        }
        method @ ("reload" | "goBack" | "goForward" | "getZoom" | "getZoomSettings"
        | "setZoomSettings" | "detectLanguage" | "executeScript" | "insertCSS"
        | "removeCSS") => {
            let (id, rest) = match args.first() {
                Some(first) if first.is_number() => (first.as_i64(), &args[1..]),
                Some(first) if first.is_null() => (None, &args[1..]),
                _ => (None, &args[..]),
            };
            forward(view, &target(view, &call.asker, id)?, method, rest)
        }
        // `group` and `ungroup` (which crash in Chromium on these tabs)
        // land here until the tab-groups change gives them rows above.
        method => Err(unsupported("tabs", method)),
    }
}

fn query(view: &View, asker: &Asker, info: Option<&Value>) -> Result<Outcome, String> {
    let empty = Map::new();
    let info = match info {
        None => &empty,
        Some(Value::Object(info)) => info,
        Some(_) => return Err(invalid("queryInfo")),
    };
    if let Some(key) = info.keys().find(|key| !QUERY_KEYS.contains(&key.as_str())) {
        return Err(format!("Unexpected property: '{key}'."));
    }
    let flag = |key: &str| info.get(key).and_then(Value::as_bool);
    let number = |key: &str| info.get(key).and_then(Value::as_i64);
    let text = |key: &str| info.get(key).and_then(Value::as_str);
    let patterns: Option<Vec<&str>> = match info.get("url") {
        Some(Value::String(one)) => Some(vec![one.as_str()]),
        Some(Value::Array(many)) => Some(many.iter().filter_map(Value::as_str).collect()),
        _ => None,
    };
    let current = view.current_window(asker).map(|window| window.id);
    let last = view.last_focused_window().map(|window| window.id);
    let is = |want: Option<bool>, have: bool| want.is_none_or(|want| want == have);
    let mut found = Vec::new();
    for window in &view.snapshot.windows {
        for (index, tab) in window.tabs.iter().enumerate() {
            let seen = view.grants.sees(&tab.url);
            let keep = is(flag("active"), tab.active)
                && is(flag("highlighted"), tab.active)
                && is(flag("pinned"), false)
                && is(flag("audible"), false)
                && is(flag("muted"), false)
                && is(flag("frozen"), false)
                && is(flag("autoDiscardable"), true)
                && is(flag("discarded"), tab.deferred)
                && is(flag("currentWindow"), Some(window.id) == current)
                && is(flag("lastFocusedWindow"), Some(window.id) == last)
                && number("windowId").is_none_or(|id| {
                    if id == WINDOW_ID_CURRENT {
                        Some(window.id) == current
                    } else {
                        id == window_id(window.id)
                    }
                })
                && number("index").is_none_or(|want| want == index as i64)
                && number("groupId").is_none_or(|want| want == group_id(tab.folder))
                && number("splitViewId").is_none_or(|want| want == -1)
                && text("status").is_none_or(|want| want == status(tab))
                && text("windowType").is_none_or(|want| want == "normal")
                && text("title").is_none_or(|want| seen && glob(want, &tab.title))
                && patterns.as_ref().is_none_or(|patterns| {
                    seen && Url::parse(&tab.url)
                        .is_ok_and(|url| patterns.iter().any(|pattern| matches_url(pattern, &url)))
                });
            if keep {
                found.push(view.tab_json(window, index));
            }
        }
    }
    Ok(Outcome::Answer(Value::Array(found)))
}

fn create(call: &BridgeCall, view: &View, props: Option<&Value>) -> Result<Outcome, String> {
    let prop = |key: &str| {
        props
            .and_then(|props| props.get(key))
            .filter(|v| !v.is_null())
    };
    let window = match prop("windowId").and_then(Value::as_i64) {
        None => None,
        Some(id) => Some(window_for(view, &call.asker, id)?.id),
    };
    let url = match prop("url").and_then(Value::as_str) {
        Some(text) => navigable(&call.extension, text)?,
        None => Url::parse("about:blank").expect("about:blank is a URL"),
    };
    let active = prop("active")
        .or(prop("selected"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    Ok(Outcome::Request {
        requests: vec![ExtensionRequest::OpenTab {
            window,
            url,
            active,
        }],
        then: Then::CreatedTab,
    })
}

fn update(call: &BridgeCall, view: &View) -> Result<Outcome, String> {
    let args = &call.args;
    let (id, props) = match args.first() {
        Some(first) if first.is_object() => (None, Some(first)),
        Some(first) => (first.as_i64(), args.get(1)),
        None => (None, None),
    };
    let mut rest = props
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| invalid("updateProperties"))?;
    let found = target(view, &call.asker, id)?;
    let activate = ["active", "highlighted", "selected"]
        .iter()
        .any(|key| rest.get(*key).and_then(Value::as_bool) == Some(true));
    // The strip is osmena's: these never reach Chromium, whose tab-strip
    // code does not know these tabs.
    for key in ["active", "highlighted", "selected", "pinned"] {
        rest.remove(key);
    }
    if let Some(text) = rest.get("url").and_then(Value::as_str) {
        let url = navigable(&call.extension, text)?;
        rest.insert("url".to_string(), json!(url.as_str()));
    }
    if activate && !found.tab.active {
        return Ok(Outcome::Request {
            requests: vec![ExtensionRequest::ActivateTab(found.tab.id)],
            then: Then::Native {
                tab: found.tab.id,
                rest: Value::Object(rest),
            },
        });
    }
    if !view.ids.is_live(found.tab.id) {
        return if rest.contains_key("url") {
            Err(not_loaded(found.ext))
        } else {
            Ok(Outcome::Answer(view.tab_json(found.window, found.index)))
        };
    }
    Ok(if rest.is_empty() {
        forward_live(view, &found, "get", &[])
    } else {
        forward_live(view, &found, "update", &[Value::Object(rest)])
    })
}

fn move_tabs(call: &BridgeCall, view: &View) -> Result<Outcome, String> {
    let first = call.args.first().filter(|value| !value.is_null());
    let ids = id_list(first)?;
    let single = first.is_some_and(Value::is_number);
    let props = call
        .args
        .get(1)
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("moveProperties"))?;
    let index = props
        .get("index")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid("index"))?;
    let to_window = match props.get("windowId").and_then(Value::as_i64) {
        Some(id) => Some(window_for(view, &call.asker, id)?.id),
        None => None,
    };
    let mut requests = Vec::new();
    let mut moved = Vec::new();
    for (offset, id) in ids.into_iter().enumerate() {
        let found = find(view, id)?;
        if to_window.is_some_and(|window| window != found.window.id) {
            return Err("Moving a tab to another window is not supported.".to_string());
        }
        let last = found.window.tabs.len() - 1;
        let to = if index < 0 {
            last
        } else {
            (index as usize + offset).min(last)
        };
        requests.push(ExtensionRequest::MoveTab {
            tab: found.tab.id,
            index: to,
        });
        moved.push(found.tab.id);
    }
    if requests.is_empty() {
        return Ok(Outcome::Answer(json!([])));
    }
    Ok(Outcome::Request {
        requests,
        then: Then::Tabs {
            tabs: moved,
            single,
        },
    })
}

fn highlight(call: &BridgeCall, view: &View, info: Option<&Value>) -> Result<Outcome, String> {
    let info = info
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("highlightInfo"))?;
    let window = match info.get("windowId").and_then(Value::as_i64) {
        Some(id) => window_for(view, &call.asker, id)?,
        None => view.current_window(&call.asker).ok_or(NO_CURRENT_WINDOW)?,
    };
    let first = match info.get("tabs") {
        Some(Value::Number(one)) => one.as_i64(),
        Some(Value::Array(many)) => many.first().and_then(Value::as_i64),
        _ => None,
    }
    .ok_or_else(|| invalid("tabs"))?;
    let tab = usize::try_from(first)
        .ok()
        .and_then(|index| window.tabs.get(index))
        .ok_or_else(|| format!("No tab at index: {first}."))?;
    if tab.active {
        return Ok(Outcome::Answer(view.window_json(window, true)));
    }
    Ok(Outcome::Request {
        requests: vec![ExtensionRequest::ActivateTab(tab.id)],
        then: Then::Window {
            window: window.id,
            populate: true,
        },
    })
}

fn capture(call: &BridgeCall, view: &View) -> Result<Outcome, String> {
    let window = match call.args.first().and_then(Value::as_i64) {
        Some(id) => window_for(view, &call.asker, id)?,
        None => view.current_window(&call.asker).ok_or(NO_CURRENT_WINDOW)?,
    };
    let options = call.args.iter().find_map(Value::as_object);
    let option = |key: &str| options.and_then(|options| options.get(key));
    let format = match option("format").and_then(Value::as_str) {
        None | Some("jpeg") => "jpeg",
        Some("png") => "png",
        Some(_) => return Err(invalid("format")),
    };
    let quality = option("quality")
        .and_then(Value::as_i64)
        .unwrap_or(90)
        .clamp(0, 100);
    // `activeTab` is never in effect here (a delivered click carries no
    // gesture), which leaves the other permission Chromium accepts.
    if !view.grants.all_urls {
        return Err("Either the '<all_urls>' or 'activeTab' permission is required.".to_string());
    }
    let tab = window.active().ok_or(NO_SELECTED_TAB)?;
    if !view.ids.is_live(tab.id) {
        return Err(not_loaded(view.ids.ext(tab.id).unwrap_or(TAB_ID_NONE)));
    }
    Ok(Outcome::Capture {
        tab: tab.id,
        format: format.to_string(),
        quality,
    })
}

fn windows(call: &BridgeCall, view: &View) -> Result<Outcome, String> {
    let args = &call.args;
    let arg = |index: usize| args.get(index).filter(|value| !value.is_null());
    let window_id_at = |index: usize| {
        arg(index)
            .and_then(Value::as_i64)
            .ok_or_else(|| invalid("windowId"))
    };
    let populate = |index: usize| {
        arg(index)
            .and_then(|options| options.get("populate"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    match call.method.as_str() {
        "get" => {
            let window = window_for(view, &call.asker, window_id_at(0)?)?;
            Ok(Outcome::Answer(view.window_json(window, populate(1))))
        }
        "getCurrent" => {
            let window = view.current_window(&call.asker).ok_or(NO_CURRENT_WINDOW)?;
            Ok(Outcome::Answer(view.window_json(window, populate(0))))
        }
        "getLastFocused" => {
            let window = view.last_focused_window().ok_or(NO_LAST_FOCUSED_WINDOW)?;
            Ok(Outcome::Answer(view.window_json(window, populate(0))))
        }
        "getAll" => {
            // Every window here is a normal one.
            let normal = arg(0)
                .and_then(|options| options.get("windowTypes"))
                .and_then(Value::as_array)
                .is_none_or(|types| types.iter().any(|t| t.as_str() == Some("normal")));
            let list = if normal {
                view.snapshot
                    .windows
                    .iter()
                    .map(|window| view.window_json(window, populate(0)))
                    .collect()
            } else {
                Vec::new()
            };
            Ok(Outcome::Answer(Value::Array(list)))
        }
        "create" => create_window(call, arg(0)),
        // Nothing the model can be asked: focus, state and bounds are the
        // user's. Answer the window as it is.
        "update" => {
            let window = window_for(view, &call.asker, window_id_at(0)?)?;
            Ok(Outcome::Answer(view.window_json(window, false)))
        }
        "remove" => {
            let window = window_for(view, &call.asker, window_id_at(0)?)?;
            let closing: Vec<TabId> = window.tabs.iter().map(|tab| tab.id).collect();
            Ok(Outcome::Request {
                requests: vec![ExtensionRequest::CloseTabs(closing.clone())],
                then: Then::Removed { tabs: closing },
            })
        }
        method => Err(unsupported("windows", method)),
    }
}

fn create_window(call: &BridgeCall, data: Option<&Value>) -> Result<Outcome, String> {
    let field = |key: &str| data.and_then(|data| data.get(key)).filter(|v| !v.is_null());
    if field("incognito").and_then(Value::as_bool) == Some(true) {
        return Err("Incognito mode is disabled.".to_string());
    }
    if field("tabId").is_some() {
        return Err("windows.create with a tabId is not supported.".to_string());
    }
    let texts: Vec<&str> = match field("url") {
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let mut urls = Vec::new();
    for text in texts {
        urls.push(navigable(&call.extension, text)?);
    }
    let mut urls = urls.into_iter();
    Ok(Outcome::Request {
        requests: vec![ExtensionRequest::OpenWindow { url: urls.next() }],
        then: Then::CreatedWindow {
            more: urls.collect(),
        },
    })
}

/// Shape the model's `reply` into an answer, as far as the published
/// strip allows yet. `fresh`: a strip was published since the reply.
/// `waited`: how long ago the reply came.
pub(crate) fn settle(
    then: &Then,
    reply: &ExtensionReply,
    view: &View,
    fresh: bool,
    waited: Duration,
) -> Settled {
    if let ExtensionReply::Refused(message) = reply {
        return Settled::Now(Err(refusal(message, view.ids)));
    }
    let ready = fresh || waited >= SETTLE_QUIET;
    let last_chance = waited >= SETTLE_LIMIT;
    let window_now =
        |window: WindowId, populate: bool, ready: bool| match view.snapshot.window(window) {
            Some(found) if ready => Settled::Now(Ok(view.window_json(found, populate))),
            None if last_chance => Settled::Now(Err(no_window(window_id(window)))),
            _ => Settled::Wait,
        };
    match then {
        Then::Removed { tabs } => {
            let gone = tabs.iter().all(|tab| view.snapshot.locate(*tab).is_none());
            if gone || last_chance {
                Settled::Now(Ok(Value::Null))
            } else {
                Settled::Wait
            }
        }
        Then::CreatedTab => match reply {
            ExtensionReply::Tab(tab) => match view.snapshot.locate(*tab) {
                Some((window, index)) => Settled::Now(Ok(view.tab_json(window, index))),
                None if last_chance => Settled::Now(Err(
                    "The tab was closed before it could be reported.".to_string(),
                )),
                None => Settled::Wait,
            },
            _ => Settled::Now(Err(UNEXPECTED_REPLY.to_string())),
        },
        Then::Native { tab, rest } => {
            let Some((window, index)) = view.snapshot.locate(*tab) else {
                return if ready {
                    Settled::Now(Err(no_tab(view.ids.ext(*tab).unwrap_or(TAB_ID_NONE))))
                } else {
                    Settled::Wait
                };
            };
            let found = Found::at(view, window, index);
            if found.tab.active && view.ids.is_live(*tab) {
                let rest = rest.as_object().filter(|rest| !rest.is_empty());
                let outcome = match rest {
                    Some(rest) => {
                        forward_live(view, &found, "update", &[Value::Object(rest.clone())])
                    }
                    None => forward_live(view, &found, "get", &[]),
                };
                let Outcome::Forward {
                    method,
                    args,
                    strip,
                } = outcome
                else {
                    unreachable!("forward_live returns a forward");
                };
                Settled::Now(Ok(forward_value(&method, args, strip)))
            } else if last_chance {
                Settled::Now(Ok(view.tab_json(window, index)))
            } else {
                Settled::Wait
            }
        }
        Then::Tabs { tabs, single } => {
            if !ready {
                return Settled::Wait;
            }
            let mut objects = Vec::new();
            for tab in tabs {
                match view.snapshot.locate(*tab) {
                    Some((window, index)) => objects.push(view.tab_json(window, index)),
                    None => {
                        return Settled::Now(Err(no_tab(
                            view.ids.ext(*tab).unwrap_or(TAB_ID_NONE),
                        )));
                    }
                }
            }
            Settled::Now(Ok(if *single {
                objects.into_iter().next().unwrap_or(Value::Null)
            } else {
                Value::Array(objects)
            }))
        }
        Then::CreatedWindow { more } => match reply {
            ExtensionReply::Window(window) if more.is_empty() => window_now(*window, true, true),
            ExtensionReply::Window(window) => Settled::More {
                requests: more
                    .iter()
                    .map(|url| ExtensionRequest::OpenTab {
                        window: Some(*window),
                        url: url.clone(),
                        active: false,
                    })
                    .collect(),
                then: Then::Window {
                    window: *window,
                    populate: true,
                },
            },
            _ => Settled::Now(Err(UNEXPECTED_REPLY.to_string())),
        },
        Then::Window { window, populate } => window_now(*window, *populate, ready),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use osmena_core::{ExtensionReply, ExtensionRequest, FolderId, TabId, WindowId};
    use serde_json::{Value, json};
    use url::Url;

    use super::*;
    use crate::bridge::ids::{IdSpace, SYNTHETIC_BASE};
    use crate::bridge::pattern::Grants;
    use crate::bridge::shape::View;
    use crate::bridge::snapshot::Snapshot;
    use crate::bridge::snapshot::tests::{snapshot, tab, window};
    use crate::bridge::transport::{Asker, BridgeCall};

    const EXT: &str = "abcdefghijklmnopabcdefghijklmnop";

    /// A model for the router: live tabs are known to Chromium as
    /// 100 + their id.
    struct World {
        snapshot: Snapshot,
        ids: IdSpace,
        grants: Grants,
        last_focused: Option<WindowId>,
    }

    impl World {
        fn new(snapshot: Snapshot) -> World {
            let mut ids = IdSpace::default();
            let pairs: Vec<(TabId, Option<i64>)> = snapshot
                .windows
                .iter()
                .flat_map(|w| &w.tabs)
                .map(|t| (t.id, (!t.deferred).then_some(100 + t.id.0 as i64)))
                .collect();
            ids.sync(&pairs);
            World {
                snapshot,
                ids,
                grants: Grants {
                    tabs: true,
                    all_urls: true,
                    hosts: vec!["<all_urls>".to_string()],
                },
                last_focused: Some(WindowId(1)),
            }
        }

        /// Two windows: 1 (focused) holds tabs 1 (active), 2, and 3
        /// (deferred); 2 holds tabs 4 and 5 (active).
        fn two_windows() -> World {
            World::new(snapshot(vec![
                window(1, true, vec![tab(1, "a*"), tab(2, "b"), tab(3, "c~")]),
                window(2, false, vec![tab(4, "d"), tab(5, "e*")]),
            ]))
        }

        fn view(&self) -> View<'_> {
            View {
                snapshot: &self.snapshot,
                ids: &self.ids,
                grants: &self.grants,
                last_focused: self.last_focused,
            }
        }

        fn ask(&self, asker: Asker, namespace: &str, method: &str, args: Value) -> Outcome {
            let call = BridgeCall {
                id: 1,
                extension: EXT.to_string(),
                asker,
                namespace: namespace.to_string(),
                method: method.to_string(),
                args: args.as_array().cloned().unwrap_or_default(),
                page: None,
            };
            route(&call, &self.view())
        }

        fn tabs(&self, method: &str, args: Value) -> Outcome {
            self.ask(Asker::Worker, "tabs", method, args)
        }
    }

    /// (id, index, windowId, active) of each tab object in an answer.
    fn brief(outcome: &Outcome) -> Vec<(i64, i64, i64, bool)> {
        let Outcome::Answer(Value::Array(tabs)) = outcome else {
            panic!("not a list of tabs: {outcome:?}");
        };
        tabs.iter()
            .map(|t| {
                (
                    t["id"].as_i64().unwrap(),
                    t["index"].as_i64().unwrap(),
                    t["windowId"].as_i64().unwrap(),
                    t["active"].as_bool().unwrap(),
                )
            })
            .collect()
    }

    fn rejected(outcome: Outcome) -> String {
        match outcome {
            Outcome::Reject(message) => message,
            other => panic!("not a rejection: {other:?}"),
        }
    }

    #[test]
    fn query_lists_every_tab_of_every_window_in_strip_order() {
        let world = World::two_windows();
        assert_eq!(
            brief(&world.tabs("query", json!([{}]))),
            vec![
                (101, 0, 1, true),
                (102, 1, 1, false),
                (SYNTHETIC_BASE, 2, 1, false),
                (104, 0, 2, false),
                (105, 1, 2, true),
            ]
        );
        let Outcome::Answer(Value::Array(tabs)) = world.tabs("query", json!([{}])) else {
            panic!();
        };
        assert_eq!(
            tabs[0],
            json!({
                "id": 101, "index": 0, "windowId": 1, "groupId": -1,
                "active": true, "highlighted": true, "selected": true,
                "pinned": false, "incognito": false, "audible": false,
                "mutedInfo": { "muted": false }, "discarded": false,
                "autoDiscardable": true, "frozen": false, "lastAccessed": 0,
                "splitViewId": -1, "status": "complete",
                "url": "https://a.example/", "title": "A"
            })
        );
        // A deferred tab is listed with its address and title.
        assert_eq!(tabs[2]["url"], "https://c.example/");
        assert_eq!(tabs[2]["title"], "C");
        assert_eq!(tabs[2]["discarded"], true);
        assert_eq!(tabs[2]["status"], "unloaded");
    }

    #[test]
    fn the_current_window_is_the_askers() {
        let world = World::two_windows();
        let active_current = json!([{ "active": true, "currentWindow": true }]);
        for (asker, expected) in [
            // The worker: the window focused last.
            (Asker::Worker, 101),
            // A page shown as tab 4, which is in window 2.
            (Asker::View(TabId(4)), 105),
            // A popup opened over tab 5.
            (Asker::Popup { over: TabId(5) }, 105),
            // A view nobody knows falls back to the last focused window.
            (Asker::View(TabId(77)), 101),
        ] {
            let outcome = world.ask(asker.clone(), "tabs", "query", active_current.clone());
            assert_eq!(
                brief(&outcome).iter().map(|t| t.0).collect::<Vec<_>>(),
                vec![expected],
                "{asker:?}"
            );
        }
        // A panel's view names its window (Phase 6 shows one).
        let mut world = World::two_windows();
        world.snapshot.windows[1].panel_view = Some(TabId(9));
        let outcome = world.ask(Asker::View(TabId(9)), "tabs", "query", active_current);
        assert_eq!(brief(&outcome)[0].0, 105);
    }

    #[test]
    fn query_filters_by_every_key_and_refuses_an_unknown_one() {
        let mut world = World::two_windows();
        world.snapshot.windows[0].tabs[1].folder = Some(FolderId(7));
        world.snapshot.windows[1].tabs[0].load = osmena_core::LoadState::Loading;
        for (info, expected) in [
            (json!({ "active": true }), vec![101, 105]),
            (
                json!({ "active": false, "windowId": 1 }),
                vec![102, SYNTHETIC_BASE],
            ),
            (json!({ "windowId": 2 }), vec![104, 105]),
            (json!({ "windowId": -2 }), vec![101, 102, SYNTHETIC_BASE]),
            (json!({ "windowId": 9 }), vec![]),
            (
                json!({ "lastFocusedWindow": true, "highlighted": true }),
                vec![101],
            ),
            (json!({ "currentWindow": false }), vec![104, 105]),
            (json!({ "index": 1 }), vec![102, 105]),
            (json!({ "groupId": 7 }), vec![102]),
            (
                json!({ "groupId": -1, "windowId": 1 }),
                vec![101, SYNTHETIC_BASE],
            ),
            (json!({ "discarded": true }), vec![SYNTHETIC_BASE]),
            (json!({ "status": "loading" }), vec![104]),
            (json!({ "status": "unloaded" }), vec![SYNTHETIC_BASE]),
            (json!({ "pinned": true }), vec![]),
            (
                json!({ "audible": false, "muted": false, "frozen": false, "autoDiscardable": true, "windowId": 2 }),
                vec![104, 105],
            ),
            (
                json!({ "windowType": "normal", "windowId": 2 }),
                vec![104, 105],
            ),
            (json!({ "windowType": "popup" }), vec![]),
            (json!({ "splitViewId": -1, "windowId": 2 }), vec![104, 105]),
            (json!({ "title": "B*" }), vec![102]),
            (json!({ "url": "https://d.example/*" }), vec![104]),
            (
                json!({ "url": ["https://a.example/*", "*://e.example/*"] }),
                vec![101, 105],
            ),
            (
                json!({ "url": "<all_urls>", "windowId": 2 }),
                vec![104, 105],
            ),
        ] {
            let outcome = world.tabs("query", json!([info]));
            assert_eq!(
                brief(&outcome).iter().map(|t| t.0).collect::<Vec<_>>(),
                expected,
                "{info}"
            );
        }
        assert_eq!(brief(&world.tabs("query", json!([]))).len(), 5);
        assert_eq!(
            rejected(world.tabs("query", json!([{ "colour": "red" }]))),
            "Unexpected property: 'colour'."
        );
    }

    #[test]
    fn an_extension_without_the_tabs_permission_gets_no_addresses() {
        let mut world = World::two_windows();
        world.grants = Grants {
            tabs: false,
            all_urls: false,
            hosts: vec!["https://b.example/*".to_string()],
        };
        let Outcome::Answer(Value::Array(tabs)) = world.tabs("query", json!([{}])) else {
            panic!();
        };
        assert_eq!(tabs.len(), 5);
        assert_eq!(tabs[0].get("url"), None);
        assert_eq!(tabs[0].get("title"), None);
        assert_eq!(tabs[1]["url"], "https://b.example/");
        // A filter on the address finds only what it may see.
        assert_eq!(
            brief(&world.tabs("query", json!([{ "url": "*://*/*" }]))).len(),
            1
        );
        assert_eq!(
            brief(&world.tabs("query", json!([{ "title": "*" }]))).len(),
            1
        );
    }

    #[test]
    fn get_forwards_a_live_tab_with_its_strip_fields_and_answers_a_deferred_one() {
        let world = World::two_windows();
        assert_eq!(
            world.tabs("get", json!([105])),
            Outcome::Forward {
                method: "get".to_string(),
                args: vec![json!(105)],
                strip: json!({ "105": {
                    "index": 1, "windowId": 2, "groupId": -1, "active": true,
                    "highlighted": true, "selected": true, "pinned": false
                } }),
            }
        );
        let Outcome::Answer(deferred) = world.tabs("get", json!([SYNTHETIC_BASE])) else {
            panic!();
        };
        assert_eq!(deferred["id"], SYNTHETIC_BASE);
        assert_eq!(deferred["url"], "https://c.example/");
        // The page shown as tab 4 asks which tab it is.
        assert!(matches!(
            world.ask(Asker::View(TabId(4)), "tabs", "getCurrent", json!([])),
            Outcome::Forward { ref method, ref args, .. } if method == "get" && args == &vec![json!(104)]
        ));
        assert_eq!(
            world.tabs("getCurrent", json!([])),
            Outcome::Answer(Value::Null)
        );
        assert_eq!(
            world.ask(
                Asker::Popup { over: TabId(1) },
                "tabs",
                "getCurrent",
                json!([])
            ),
            Outcome::Answer(Value::Null)
        );
        // What `sender.tab` is given.
        assert_eq!(
            world.tabs("stripOf", json!([102])),
            Outcome::Answer(json!({
                "index": 1, "windowId": 1, "groupId": -1, "active": false,
                "highlighted": false, "selected": false, "pinned": false
            }))
        );
        assert_eq!(
            world.tabs("stripOf", json!([999])),
            Outcome::Answer(Value::Null)
        );
    }

    #[test]
    fn an_unknown_tab_or_window_rejects_with_chromiums_words() {
        let world = World::two_windows();
        for (method, args) in [
            ("get", json!([999])),
            ("remove", json!([[101, 999]])),
            ("update", json!([999, { "active": true }])),
            ("duplicate", json!([999])),
            ("move", json!([999, { "index": 0 }])),
            ("discard", json!([999])),
            ("reload", json!([999])),
        ] {
            assert_eq!(
                rejected(world.tabs(method, args)),
                "No tab with id: 999.",
                "{method}"
            );
        }
        for (namespace, method, args) in [
            ("windows", "get", json!([9])),
            ("windows", "remove", json!([9])),
            ("windows", "update", json!([9, { "focused": true }])),
            ("tabs", "create", json!([{ "windowId": 9 }])),
            ("tabs", "captureVisibleTab", json!([9])),
            ("tabs", "highlight", json!([{ "windowId": 9, "tabs": 0 }])),
        ] {
            assert_eq!(
                rejected(world.ask(Asker::Worker, namespace, method, args)),
                "No window with id: 9.",
                "{namespace}.{method}"
            );
        }
        assert_eq!(
            rejected(world.tabs("highlight", json!([{ "tabs": [7] }]))),
            "No tab at index: 7."
        );
    }

    #[test]
    fn create_becomes_open_tab_and_resolves_with_the_tab_once_published() {
        let world = World::two_windows();
        let outcome = world.tabs(
            "create",
            json!([{ "url": "https://example.net/", "active": false, "index": 0, "pinned": true }]),
        );
        assert_eq!(
            outcome,
            Outcome::Request {
                requests: vec![ExtensionRequest::OpenTab {
                    window: None,
                    url: Url::parse("https://example.net/").unwrap(),
                    active: false,
                }],
                then: Then::CreatedTab,
            }
        );
        // Defaults: active, a blank page; a relative address is the
        // extension's own page; a window may be named or "current".
        assert_eq!(
            world.ask(
                Asker::View(TabId(4)),
                "tabs",
                "create",
                json!([{ "url": "page.html", "windowId": -2 }])
            ),
            Outcome::Request {
                requests: vec![ExtensionRequest::OpenTab {
                    window: Some(WindowId(2)),
                    url: Url::parse(&format!("chrome-extension://{EXT}/page.html")).unwrap(),
                    active: true,
                }],
                then: Then::CreatedTab,
            }
        );
        assert!(matches!(
            world.tabs("create", json!([])),
            Outcome::Request { ref requests, .. }
                if requests == &vec![ExtensionRequest::OpenTab {
                    window: None,
                    url: Url::parse("about:blank").unwrap(),
                    active: true,
                }]
        ));
        // The reply names tab 9, which the published strip does not hold
        // yet: wait. Once it does, the promise gets that tab's object.
        let reply = ExtensionReply::Tab(TabId(9));
        assert_eq!(
            settle(
                &Then::CreatedTab,
                &reply,
                &world.view(),
                false,
                Duration::ZERO
            ),
            Settled::Wait
        );
        let mut after = World::new(snapshot(vec![window(
            1,
            true,
            vec![tab(1, "a*"), tab(2, "b"), tab(9, "new")],
        )]));
        after.last_focused = Some(WindowId(1));
        let Settled::Now(Ok(created)) = settle(
            &Then::CreatedTab,
            &reply,
            &after.view(),
            true,
            Duration::ZERO,
        ) else {
            panic!();
        };
        assert_eq!(created["id"], 109);
        assert_eq!(created["index"], 2);
        assert_eq!(created["windowId"], 1);
        // A tab that never appears is given up on.
        assert_eq!(
            settle(&Then::CreatedTab, &reply, &world.view(), true, SETTLE_LIMIT),
            Settled::Now(Err(
                "The tab was closed before it could be reported.".to_string()
            ))
        );
        // What the model refuses, the promise rejects with.
        assert_eq!(
            settle(
                &Then::CreatedTab,
                &ExtensionReply::Refused("No window with id: 3.".to_string()),
                &world.view(),
                false,
                Duration::ZERO
            ),
            Settled::Now(Err("No window with id: 3.".to_string()))
        );
        // A refusal names a tab by the model's id; the extension is told
        // the id it knows the tab by. Windows and groups keep their
        // numbers, and a tab nobody can name any more is named by no id.
        let first = world.ids.ext(TabId(1)).expect("tab 1 has an id here");
        assert_ne!(first, 1);
        assert_eq!(
            settle(
                &Then::Removed {
                    tabs: vec![TabId(1)]
                },
                &ExtensionReply::Refused("No tab with id: 1.".to_string()),
                &world.view(),
                false,
                Duration::ZERO
            ),
            Settled::Now(Err(no_tab(first)))
        );
        assert_eq!(
            refusal("No group with id: 7.", &world.ids),
            "No group with id: 7."
        );
        assert_eq!(
            refusal("No tabs to group.", &world.ids),
            "No tabs to group."
        );
        assert_eq!(
            refusal("No tab with id: 999.", &world.ids),
            "The tab no longer exists."
        );
        // Duplicating is opening the same address beside it.
        assert_eq!(
            world.tabs("duplicate", json!([104])),
            Outcome::Request {
                requests: vec![ExtensionRequest::OpenTab {
                    window: Some(WindowId(2)),
                    url: Url::parse("https://d.example/").unwrap(),
                    active: true,
                }],
                then: Then::CreatedTab,
            }
        );
    }

    #[test]
    fn urls_are_checked_as_chromium_checks_them() {
        for (text, expected) in [
            (
                "javascript:alert(1)",
                "JavaScript URLs are not allowed in API based extension navigations. Use chrome.scripting.executeScript instead.",
            ),
            ("chrome://crash", "I'm sorry. I'm afraid I can't do that."),
            ("chrome://kill/", "I'm sorry. I'm afraid I can't do that."),
            (
                "devtools://devtools/bundled/inspector.html",
                "Cannot navigate to a devtools:// page.",
            ),
            (
                "chrome-untrusted://x/",
                "Cannot navigate to a chrome-untrusted:// page.",
            ),
            (
                "file:///etc/passwd",
                "Cannot navigate to a file URL without local file access.",
            ),
            ("http://[bad", "Invalid url: \"http://[bad\"."),
        ] {
            assert_eq!(navigable(EXT, text), Err(expected.to_string()), "{text}");
        }
        assert_eq!(
            navigable(EXT, "chrome://extensions/").unwrap().as_str(),
            "chrome://extensions/"
        );
        assert_eq!(
            navigable(EXT, "/options.html?a=1").unwrap().as_str(),
            format!("chrome-extension://{EXT}/options.html?a=1")
        );
    }

    #[test]
    fn remove_becomes_close_tabs() {
        let world = World::two_windows();
        assert_eq!(
            world.tabs("remove", json!([102])),
            Outcome::Request {
                requests: vec![ExtensionRequest::CloseTabs(vec![TabId(2)])],
                then: Then::Removed {
                    tabs: vec![TabId(2)]
                },
            }
        );
        // Several, a deferred one among them.
        assert_eq!(
            world.tabs("remove", json!([[104, SYNTHETIC_BASE]])),
            Outcome::Request {
                requests: vec![ExtensionRequest::CloseTabs(vec![TabId(4), TabId(3)])],
                then: Then::Removed {
                    tabs: vec![TabId(4), TabId(3)]
                },
            }
        );
        assert_eq!(
            world.tabs("remove", json!([[]])),
            Outcome::Answer(Value::Null)
        );
        // The promise resolves when the published strip no longer shows
        // the tabs, so a query straight after does not list them.
        let gone = Then::Removed {
            tabs: vec![TabId(2)],
        };
        assert_eq!(
            settle(
                &gone,
                &ExtensionReply::Done,
                &world.view(),
                true,
                Duration::ZERO
            ),
            Settled::Wait
        );
        let after = World::new(snapshot(vec![window(1, true, vec![tab(1, "a*")])]));
        assert_eq!(
            settle(
                &gone,
                &ExtensionReply::Done,
                &after.view(),
                true,
                Duration::ZERO
            ),
            Settled::Now(Ok(Value::Null))
        );
        assert_eq!(
            settle(
                &gone,
                &ExtensionReply::Done,
                &world.view(),
                false,
                SETTLE_LIMIT
            ),
            Settled::Now(Ok(Value::Null))
        );
    }

    #[test]
    fn update_active_becomes_activate_tab() {
        let world = World::two_windows();
        let outcome = world.tabs("update", json!([102, { "active": true }]));
        let then = Then::Native {
            tab: TabId(2),
            rest: json!({}),
        };
        assert_eq!(
            outcome,
            Outcome::Request {
                requests: vec![ExtensionRequest::ActivateTab(TabId(2))],
                then: then.clone(),
            }
        );
        // Not active in the published strip yet: wait.
        assert_eq!(
            settle(
                &then,
                &ExtensionReply::Done,
                &world.view(),
                false,
                Duration::ZERO
            ),
            Settled::Wait
        );
        // Active: Chromium is asked for the tab, with the strip's fields.
        let after = World::new(snapshot(vec![window(
            1,
            true,
            vec![tab(1, "a"), tab(2, "b*")],
        )]));
        let Settled::Now(Ok(value)) = settle(
            &then,
            &ExtensionReply::Done,
            &after.view(),
            true,
            Duration::ZERO,
        ) else {
            panic!();
        };
        assert_eq!(value["__forward"]["method"], "get");
        assert_eq!(value["__forward"]["args"], json!([102]));
        assert_eq!(value["__forward"]["strip"]["102"]["active"], true);
        // Activating a deferred tab with an address to load: the request
        // first, then Chromium's update under the tab's new id.
        let outcome = world.tabs("update", json!([SYNTHETIC_BASE, { "selected": true, "url": "https://example.net/", "pinned": true }]));
        let then = Then::Native {
            tab: TabId(3),
            rest: json!({ "url": "https://example.net/" }),
        };
        assert_eq!(
            outcome,
            Outcome::Request {
                requests: vec![ExtensionRequest::ActivateTab(TabId(3))],
                then: then.clone(),
            }
        );
        let mut shown = World::new(snapshot(vec![window(
            1,
            true,
            vec![tab(1, "a"), tab(3, "c*")],
        )]));
        shown.ids.sync(&[(TabId(3), Some(555))]);
        let Settled::Now(Ok(value)) = settle(
            &then,
            &ExtensionReply::Done,
            &shown.view(),
            true,
            Duration::ZERO,
        ) else {
            panic!();
        };
        assert_eq!(value["__forward"]["method"], "update");
        assert_eq!(
            value["__forward"]["args"],
            json!([555, { "url": "https://example.net/" }])
        );
        // Already active: nothing to ask of the model.
        assert!(matches!(
            world.tabs("update", json!([101, { "active": true }])),
            Outcome::Forward { ref method, .. } if method == "get"
        ));
    }

    #[test]
    fn update_forwards_what_chromium_can_do_and_never_the_strip_keys() {
        let world = World::two_windows();
        assert_eq!(
            world.tabs("update", json!([102, { "url": "https://example.net/", "muted": true, "pinned": true, "active": false, "highlighted": false }])),
            Outcome::Forward {
                method: "update".to_string(),
                args: vec![json!(102), json!({ "url": "https://example.net/", "muted": true })],
                strip: json!({ "102": {
                    "index": 1, "windowId": 1, "groupId": -1, "active": false,
                    "highlighted": false, "selected": false, "pinned": false
                } }),
            }
        );
        // No id: the asker's window's active tab.
        assert!(matches!(
            world.ask(Asker::View(TabId(4)), "tabs", "update", json!([{ "muted": true }])),
            Outcome::Forward { ref args, .. } if args[0] == json!(105)
        ));
        assert!(matches!(
            world.tabs("update", json!([null, { "muted": true }])),
            Outcome::Forward { ref args, .. } if args[0] == json!(101)
        ));
        // A deferred tab has no browser to navigate.
        assert_eq!(
            rejected(world.tabs(
                "update",
                json!([SYNTHETIC_BASE, { "url": "https://example.net/" }])
            )),
            format!("The tab with id {SYNTHETIC_BASE} is not loaded.")
        );
        assert!(matches!(
            world.tabs("update", json!([SYNTHETIC_BASE, { "muted": true }])),
            Outcome::Answer(ref tab) if tab["id"] == SYNTHETIC_BASE
        ));
        assert_eq!(
            rejected(world.tabs("update", json!([102, { "url": "javascript:1" }]))),
            "JavaScript URLs are not allowed in API based extension navigations. Use chrome.scripting.executeScript instead."
        );
    }

    #[test]
    fn move_becomes_one_move_per_tab() {
        let world = World::two_windows();
        assert_eq!(
            world.tabs("move", json!([101, { "index": -1 }])),
            Outcome::Request {
                requests: vec![ExtensionRequest::MoveTab {
                    tab: TabId(1),
                    index: 2
                }],
                then: Then::Tabs {
                    tabs: vec![TabId(1)],
                    single: true
                },
            }
        );
        assert_eq!(
            world.tabs("move", json!([[102, 101], { "index": 1, "windowId": 1 }])),
            Outcome::Request {
                requests: vec![
                    ExtensionRequest::MoveTab {
                        tab: TabId(2),
                        index: 1
                    },
                    ExtensionRequest::MoveTab {
                        tab: TabId(1),
                        index: 2
                    },
                ],
                then: Then::Tabs {
                    tabs: vec![TabId(2), TabId(1)],
                    single: false
                },
            }
        );
        assert_eq!(
            rejected(world.tabs("move", json!([101, { "index": 0, "windowId": 2 }]))),
            "Moving a tab to another window is not supported."
        );
        // The answer waits for the strip that shows the move, or for a
        // moment when nothing changed.
        let then = Then::Tabs {
            tabs: vec![TabId(1)],
            single: true,
        };
        assert_eq!(
            settle(
                &then,
                &ExtensionReply::Done,
                &world.view(),
                false,
                Duration::ZERO
            ),
            Settled::Wait
        );
        let Settled::Now(Ok(moved)) = settle(
            &then,
            &ExtensionReply::Done,
            &world.view(),
            false,
            SETTLE_QUIET,
        ) else {
            panic!();
        };
        assert_eq!(moved["id"], 101);
        let many = Then::Tabs {
            tabs: vec![TabId(2), TabId(1)],
            single: false,
        };
        let Settled::Now(Ok(Value::Array(list))) = settle(
            &many,
            &ExtensionReply::Done,
            &world.view(),
            true,
            Duration::ZERO,
        ) else {
            panic!();
        };
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn highlight_activates_the_first_index_and_answers_the_window() {
        let world = World::two_windows();
        assert_eq!(
            world.tabs("highlight", json!([{ "tabs": [1, 0] }])),
            Outcome::Request {
                requests: vec![ExtensionRequest::ActivateTab(TabId(2))],
                then: Then::Window {
                    window: WindowId(1),
                    populate: true
                },
            }
        );
        assert!(matches!(
            world.tabs("highlight", json!([{ "tabs": 1, "windowId": 2 }])),
            Outcome::Answer(ref w) if w["id"] == 2 && w["tabs"].as_array().unwrap().len() == 2
        ));
    }

    #[test]
    fn discard_answers_without_a_request() {
        let world = World::two_windows();
        assert_eq!(
            world.tabs("discard", json!([102])),
            Outcome::Answer(Value::Null)
        );
        assert_eq!(
            world.tabs("discard", json!([])),
            Outcome::Answer(Value::Null)
        );
    }

    #[test]
    fn unknown_methods_and_namespaces_reject() {
        let world = World::two_windows();
        assert_eq!(
            rejected(world.tabs("group", json!([{ "tabIds": [101] }]))),
            "tabs.group is not supported."
        );
        assert_eq!(
            rejected(world.tabs("ungroup", json!([101]))),
            "tabs.ungroup is not supported."
        );
        assert_eq!(
            rejected(world.tabs("createSplit", json!([[101, 102]]))),
            "tabs.createSplit is not supported."
        );
        assert_eq!(
            rejected(world.ask(Asker::Worker, "windows", "teleport", json!([]))),
            "windows.teleport is not supported."
        );
        assert_eq!(
            rejected(world.ask(Asker::Worker, "tabGroups", "query", json!([{}]))),
            "tabGroups.query is not supported."
        );
    }

    #[test]
    fn optional_tab_methods_fill_in_the_active_tab() {
        let world = World::two_windows();
        for (asker, method, args, expected) in [
            (Asker::Worker, "reload", json!([]), json!([101])),
            (
                Asker::Worker,
                "reload",
                json!([102, { "bypassCache": true }]),
                json!([102, { "bypassCache": true }]),
            ),
            (
                Asker::Worker,
                "reload",
                json!([null, { "bypassCache": true }]),
                json!([101, { "bypassCache": true }]),
            ),
            (Asker::View(TabId(4)), "goBack", json!([]), json!([105])),
            (Asker::Worker, "goForward", json!([104]), json!([104])),
            (Asker::Worker, "getZoom", json!([]), json!([101])),
            (Asker::Worker, "setZoom", json!([1.5]), json!([101, 1.5])),
            (
                Asker::Worker,
                "setZoom",
                json!([104, 1.5]),
                json!([104, 1.5]),
            ),
            (
                Asker::Worker,
                "setZoomSettings",
                json!([{ "mode": "manual" }]),
                json!([101, { "mode": "manual" }]),
            ),
            (Asker::Worker, "detectLanguage", json!([]), json!([101])),
            (
                Asker::Worker,
                "executeScript",
                json!([{ "code": "1" }]),
                json!([101, { "code": "1" }]),
            ),
        ] {
            let outcome = world.ask(asker, "tabs", method, args.clone());
            assert!(
                matches!(outcome, Outcome::Forward { method: ref m, args: ref a, .. } if m == method && json!(a) == expected),
                "{method} {args}: {outcome:?}"
            );
        }
        assert_eq!(
            rejected(world.tabs("reload", json!([SYNTHETIC_BASE]))),
            format!("The tab with id {SYNTHETIC_BASE} is not loaded.")
        );
    }

    #[test]
    fn capture_needs_all_urls_and_names_the_active_tab() {
        let mut world = World::two_windows();
        assert_eq!(
            world.tabs("captureVisibleTab", json!([])),
            Outcome::Capture {
                tab: TabId(1),
                format: "jpeg".to_string(),
                quality: 90
            }
        );
        assert_eq!(
            world.tabs("captureVisibleTab", json!([2, { "format": "png" }])),
            Outcome::Capture {
                tab: TabId(5),
                format: "png".to_string(),
                quality: 90
            }
        );
        assert_eq!(
            world.ask(
                Asker::View(TabId(4)),
                "tabs",
                "captureVisibleTab",
                json!([{ "quality": 50 }])
            ),
            Outcome::Capture {
                tab: TabId(5),
                format: "jpeg".to_string(),
                quality: 50
            }
        );
        world.grants.all_urls = false;
        assert_eq!(
            rejected(world.tabs("captureVisibleTab", json!([]))),
            "Either the '<all_urls>' or 'activeTab' permission is required."
        );
    }

    #[test]
    fn windows_are_read_from_the_strip() {
        let world = World::two_windows();
        assert_eq!(
            world.ask(Asker::Worker, "windows", "getAll", json!([])),
            Outcome::Answer(json!([
                { "id": 1, "focused": true, "incognito": false, "alwaysOnTop": false, "type": "normal", "state": "normal" },
                { "id": 2, "focused": false, "incognito": false, "alwaysOnTop": false, "type": "normal", "state": "normal" },
            ]))
        );
        let Outcome::Answer(populated) = world.ask(
            Asker::Worker,
            "windows",
            "get",
            json!([2, { "populate": true }]),
        ) else {
            panic!();
        };
        assert_eq!(populated["tabs"].as_array().unwrap().len(), 2);
        assert_eq!(populated["tabs"][1]["id"], 105);
        for (asker, method, expected) in [
            (Asker::Worker, "getCurrent", 1),
            (Asker::View(TabId(5)), "getCurrent", 2),
            (Asker::View(TabId(5)), "getLastFocused", 1),
        ] {
            assert!(matches!(
                world.ask(asker, "windows", method, json!([])),
                Outcome::Answer(ref w) if w["id"] == expected
            ));
        }
        assert!(matches!(
            world.ask(Asker::View(TabId(5)), "windows", "get", json!([-2])),
            Outcome::Answer(ref w) if w["id"] == 2
        ));
        assert_eq!(
            world.ask(
                Asker::Worker,
                "windows",
                "getAll",
                json!([{ "windowTypes": ["popup"] }])
            ),
            Outcome::Answer(json!([]))
        );
        assert!(matches!(
            world.ask(Asker::Worker, "windows", "update", json!([2, { "focused": true }])),
            Outcome::Answer(ref w) if w["id"] == 2
        ));
        let empty = World::new(snapshot(vec![]));
        assert_eq!(
            rejected(empty.ask(Asker::Worker, "windows", "getCurrent", json!([]))),
            "No current window"
        );
        assert_eq!(
            rejected(empty.ask(Asker::Worker, "windows", "getLastFocused", json!([]))),
            "No last-focused window"
        );
    }

    #[test]
    fn windows_create_opens_the_rest_as_tabs_and_remove_closes_every_tab() {
        let world = World::two_windows();
        let first = Url::parse("https://example.net/").unwrap();
        let second = Url::parse("https://example.org/").unwrap();
        let outcome = world.ask(
            Asker::Worker,
            "windows",
            "create",
            json!([{ "url": ["https://example.net/", "https://example.org/"], "type": "popup", "width": 400 }]),
        );
        let then = Then::CreatedWindow {
            more: vec![second.clone()],
        };
        assert_eq!(
            outcome,
            Outcome::Request {
                requests: vec![ExtensionRequest::OpenWindow { url: Some(first) }],
                then: then.clone(),
            }
        );
        assert_eq!(
            settle(
                &then,
                &ExtensionReply::Window(WindowId(3)),
                &world.view(),
                false,
                Duration::ZERO
            ),
            Settled::More {
                requests: vec![ExtensionRequest::OpenTab {
                    window: Some(WindowId(3)),
                    url: second,
                    active: false
                }],
                then: Then::Window {
                    window: WindowId(3),
                    populate: true
                },
            }
        );
        // The window is answered once the published strip holds it.
        let waiting = Then::Window {
            window: WindowId(3),
            populate: true,
        };
        assert_eq!(
            settle(
                &waiting,
                &ExtensionReply::Tab(TabId(8)),
                &world.view(),
                true,
                Duration::ZERO
            ),
            Settled::Wait
        );
        assert_eq!(
            settle(
                &waiting,
                &ExtensionReply::Tab(TabId(8)),
                &world.view(),
                true,
                SETTLE_LIMIT
            ),
            Settled::Now(Err("No window with id: 3.".to_string()))
        );
        assert!(matches!(
            world.ask(Asker::Worker, "windows", "create", json!([])),
            Outcome::Request { ref requests, then: Then::CreatedWindow { ref more } }
                if requests == &vec![ExtensionRequest::OpenWindow { url: None }] && more.is_empty()
        ));
        assert_eq!(
            rejected(world.ask(
                Asker::Worker,
                "windows",
                "create",
                json!([{ "incognito": true }])
            )),
            "Incognito mode is disabled."
        );
        assert_eq!(
            rejected(world.ask(
                Asker::Worker,
                "windows",
                "create",
                json!([{ "tabId": 101 }])
            )),
            "windows.create with a tabId is not supported."
        );
        assert_eq!(
            world.ask(Asker::Worker, "windows", "remove", json!([2])),
            Outcome::Request {
                requests: vec![ExtensionRequest::CloseTabs(vec![TabId(4), TabId(5)])],
                then: Then::Removed {
                    tabs: vec![TabId(4), TabId(5)]
                },
            }
        );
    }
}
