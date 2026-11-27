//! Chrome's tab groups over osmena's tab folders, as plain data: the id
//! mapping, the `TabGroup` object, what the bridge's `tabGroups.*`,
//! `tabs.group` and `tabs.ungroup` calls become, and the group events
//! between two strips. A group is a folder: its id is the folder's
//! number, its title the folder's name, collapsed is "not expanded".
//! Folders have no colour, so every group is grey.
//!
//! Nothing here knows the transport or CEF. The router hands in a
//! [`GroupView`] cut from the published strip and the id space.

use osmena_core::{ExtensionRequest, Folder, FolderId, TabId};
use serde_json::{Value, json};

use super::ids::{WINDOW_ID_CURRENT, group_id};
use super::router::{no_tab, no_window};

/// The colour every group reports; `update` accepts any and keeps this.
pub(crate) const COLOR: &str = "grey";

const GROUP_PARAMS: &str = "Cannot specify 'createProperties' along with a 'groupId'.";
const CROSS_WINDOW: &str = "Tabs can only be grouped within their own window.";
const MOVE_UNSUPPORTED: &str = "tabGroups.move is not supported.";

/// The folder a `groupId` names, if it can name one: the reverse of
/// `group_id`. `TAB_GROUP_ID_NONE` and negative ids name none.
pub(crate) fn folder_of(group: i64) -> Option<FolderId> {
    u64::try_from(group).ok().map(FolderId)
}

/// The strip as the group calls need it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GroupView {
    pub windows: Vec<GroupWindow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GroupWindow {
    /// The `windowId` extensions see.
    pub id: i64,
    pub folders: Vec<Folder>,
    /// Strip order.
    pub tabs: Vec<GroupTab>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GroupTab {
    pub tab: TabId,
    /// The tab id extensions see, when the tab has one.
    pub id: Option<i64>,
    pub folder: Option<FolderId>,
}

/// The `TabGroup` object of `folder` in the window extensions know as
/// `window`.
pub(crate) fn tab_group(window: i64, folder: &Folder) -> Value {
    json!({
        "id": group_id(Some(folder.id)),
        "collapsed": !folder.expanded,
        "color": COLOR,
        "title": folder.name,
        "windowId": window,
        "shared": false,
    })
}

impl GroupView {
    /// Every folder that holds a tab, with its window, in strip order.
    fn groups(&self) -> Vec<(i64, &Folder)> {
        let mut found: Vec<(i64, &Folder)> = Vec::new();
        for window in &self.windows {
            for tab in &window.tabs {
                let Some(id) = tab.folder else {
                    continue;
                };
                if found.iter().any(|(_, f)| f.id == id) {
                    continue;
                }
                if let Some(folder) = window.folders.iter().find(|f| f.id == id) {
                    found.push((window.id, folder));
                }
            }
        }
        found
    }

    fn group(&self, group: i64) -> Option<(i64, &Folder)> {
        let id = folder_of(group)?;
        self.groups().into_iter().find(|(_, f)| f.id == id)
    }

    /// Where the model's `tab` is in the strip, with its window.
    fn place(&self, tab: TabId) -> Option<(i64, GroupTab)> {
        self.windows.iter().find_map(|window| {
            window
                .tabs
                .iter()
                .find(|t| t.tab == tab)
                .map(|t| (window.id, *t))
        })
    }

    fn has_window(&self, id: i64) -> bool {
        self.windows.iter().any(|w| w.id == id)
    }
}

/// A bridge call, as much of it as the group entries read.
pub(crate) struct GroupCall<'a> {
    pub namespace: &'a str,
    pub method: &'a str,
    pub args: &'a [Value],
    /// The window the call came from, for `WINDOW_ID_CURRENT`.
    pub current_window: Option<i64>,
    /// "<Extension name> Session": what the model names a folder whose
    /// title is blank.
    pub session_name: &'a str,
    /// The tab an extension's tab id names, current or former
    /// (`IdSpace::tab`): an id a tab had before Chromium's replaced it
    /// keeps naming that tab.
    pub tab_of: &'a dyn Fn(i64) -> Option<TabId>,
}

/// How a request's reply becomes the promise's value, and what the
/// published strip must show before it does (`router::settle`): a read
/// made straight after the promise resolves sees the call's own change.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GroupThen {
    /// `tabs.group`: `ExtensionReply::Folder` as its group id, once every
    /// one of `tabs` (the tabs the call named) that the strip still holds
    /// is in that folder.
    GroupId { tabs: Vec<TabId> },
    /// `tabGroups.update`: `group`, once the strip's folder has the
    /// `title` and the `collapsed` state that were asked for. A blank
    /// title is the model's to word and is not waited for by name.
    Updated {
        folder: FolderId,
        title: Option<String>,
        collapsed: Option<bool>,
        group: Value,
    },
    /// `tabs.ungroup`: nothing, once every one of `tabs` that the strip
    /// still holds is in no folder.
    Ungrouped { tabs: Vec<TabId> },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GroupRoute {
    /// Answered from the snapshot.
    Answer(Value),
    /// The model decides; `then` shapes its reply.
    Request {
        request: ExtensionRequest,
        then: GroupThen,
    },
    Reject(String),
}

/// What a group call becomes. `None` for a call that is not one of
/// these, which the router then rejects as it rejects any unknown
/// method.
pub(crate) fn route(view: &GroupView, call: &GroupCall<'_>) -> Option<GroupRoute> {
    let routed = match (call.namespace, call.method) {
        ("tabGroups", "get") => get(view, call),
        ("tabGroups", "query") => query(view, call),
        ("tabGroups", "update") => update(view, call),
        ("tabGroups", "move") => Err(MOVE_UNSUPPORTED.to_string()),
        ("tabs", "group") => group(view, call),
        ("tabs", "ungroup") => ungroup(view, call),
        _ => return None,
    };
    Some(routed.unwrap_or_else(GroupRoute::Reject))
}

fn no_group(id: i64) -> String {
    format!("No group with id: {id}.")
}

fn invalid(call: &GroupCall<'_>) -> String {
    format!("Invalid arguments to {}.{}.", call.namespace, call.method)
}

fn get(view: &GroupView, call: &GroupCall<'_>) -> Result<GroupRoute, String> {
    let id = call
        .args
        .first()
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(call))?;
    let (window, folder) = view.group(id).ok_or_else(|| no_group(id))?;
    Ok(GroupRoute::Answer(tab_group(window, folder)))
}

fn query(view: &GroupView, call: &GroupCall<'_>) -> Result<GroupRoute, String> {
    let none = serde_json::Map::new();
    let info = match call.args.first() {
        None | Some(Value::Null) => &none,
        Some(Value::Object(info)) => info,
        Some(_) => return Err(invalid(call)),
    };
    let collapsed = info.get("collapsed").and_then(Value::as_bool);
    let color = info.get("color").and_then(Value::as_str);
    let shared = info.get("shared").and_then(Value::as_bool);
    let title = info.get("title").and_then(Value::as_str);
    // A call from no window has no current one, and matches none.
    let window = info.get("windowId").and_then(Value::as_i64).map(|id| {
        if id == WINDOW_ID_CURRENT {
            call.current_window.unwrap_or(i64::MIN)
        } else {
            id
        }
    });
    let groups = view
        .groups()
        .into_iter()
        .filter(|(in_window, folder)| {
            window.is_none_or(|id| id == *in_window)
                && collapsed.is_none_or(|c| c != folder.expanded)
                && color.is_none_or(|c| c == COLOR)
                && shared.is_none_or(|s| !s)
                && title.is_none_or(|pattern| matches_pattern(pattern, &folder.name))
        })
        .map(|(in_window, folder)| tab_group(in_window, folder))
        .collect();
    Ok(GroupRoute::Answer(Value::Array(groups)))
}

/// Chromium's `base::MatchPattern`, as `tabGroups.query` uses it for
/// titles: `*` is any run of characters, `?` any one.
fn matches_pattern(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    // The last `*` and how much of the text it has swallowed.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if let Some((at, swallowed)) = star {
            p = at + 1;
            t = swallowed + 1;
            star = Some((at, swallowed + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

fn update(view: &GroupView, call: &GroupCall<'_>) -> Result<GroupRoute, String> {
    let id = call
        .args
        .first()
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(call))?;
    let properties = call
        .args
        .get(1)
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(call))?;
    let (window, folder) = view.group(id).ok_or_else(|| no_group(id))?;
    let title = properties.get("title").and_then(Value::as_str);
    let collapsed = properties.get("collapsed").and_then(Value::as_bool);
    if title.is_none() && collapsed.is_none() {
        // Colour alone: folders have none.
        return Ok(GroupRoute::Answer(tab_group(window, folder)));
    }
    let mut after = folder.clone();
    if let Some(title) = title {
        after.name = if title.trim().is_empty() {
            call.session_name.to_string()
        } else {
            title.to_string()
        };
    }
    if let Some(collapsed) = collapsed {
        after.expanded = !collapsed;
    }
    Ok(GroupRoute::Request {
        request: ExtensionRequest::UpdateFolder {
            folder: folder.id,
            name: title.map(str::to_string),
            expanded: collapsed.map(|c| !c),
        },
        then: GroupThen::Updated {
            folder: folder.id,
            title: title
                .filter(|title| !title.trim().is_empty())
                .map(str::to_string),
            collapsed,
            group: tab_group(window, &after),
        },
    })
}

/// `tabIds`: one id or a list with at least one.
fn tab_ids(value: Option<&Value>) -> Option<Vec<i64>> {
    let ids = match value? {
        Value::Number(id) => vec![id.as_i64()?],
        Value::Array(ids) => ids.iter().map(Value::as_i64).collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    (!ids.is_empty()).then_some(ids)
}

/// The model's tabs for extension tab ids, each once, in the order
/// given, and the windows they are in.
fn tabs_named(
    view: &GroupView,
    call: &GroupCall<'_>,
    ids: &[i64],
) -> Result<(Vec<TabId>, Vec<i64>), String> {
    let mut tabs = Vec::new();
    let mut windows = Vec::new();
    for id in ids {
        let tab = (call.tab_of)(*id).ok_or_else(|| no_tab(*id))?;
        let (window, tab) = view.place(tab).ok_or_else(|| no_tab(*id))?;
        if !tabs.contains(&tab.tab) {
            tabs.push(tab.tab);
        }
        if !windows.contains(&window) {
            windows.push(window);
        }
    }
    Ok((tabs, windows))
}

fn group(view: &GroupView, call: &GroupCall<'_>) -> Result<GroupRoute, String> {
    let options = call
        .args
        .first()
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(call))?;
    let ids = tab_ids(options.get("tabIds")).ok_or_else(|| invalid(call))?;
    let group = options.get("groupId").and_then(Value::as_i64);
    let create = options.get("createProperties").filter(|v| !v.is_null());
    if group.is_some() && create.is_some() {
        return Err(GROUP_PARAMS.to_string());
    }
    // The window the group is or shall be in, and the folder if it
    // exists. Resolved before the tabs, as Chromium does.
    let target: Option<(i64, Option<FolderId>)> = match group {
        Some(id) => {
            let (window, folder) = view.group(id).ok_or_else(|| no_group(id))?;
            Some((window, Some(folder.id)))
        }
        None => match create
            .and_then(|c| c.get("windowId"))
            .and_then(Value::as_i64)
        {
            None | Some(WINDOW_ID_CURRENT) => None,
            Some(id) if view.has_window(id) => Some((id, None)),
            Some(id) => return Err(no_window(id)),
        },
    };
    let (tabs, windows) = tabs_named(view, call, &ids)?;
    // A folder gathers tabs of one window where they are; nothing
    // moves a tab between windows for it.
    if windows.len() != 1 || target.is_some_and(|(window, _)| window != windows[0]) {
        return Err(CROSS_WINDOW.to_string());
    }
    Ok(GroupRoute::Request {
        request: ExtensionRequest::GroupTabs {
            tabs: tabs.clone(),
            folder: target.and_then(|(_, folder)| folder),
        },
        then: GroupThen::GroupId { tabs },
    })
}

fn ungroup(view: &GroupView, call: &GroupCall<'_>) -> Result<GroupRoute, String> {
    let ids = tab_ids(call.args.first()).ok_or_else(|| invalid(call))?;
    let (tabs, _) = tabs_named(view, call, &ids)?;
    Ok(GroupRoute::Request {
        request: ExtensionRequest::UngroupTabs(tabs.clone()),
        then: GroupThen::Ungrouped { tabs },
    })
}

/// One group event of a strip change. A tab's own `groupId` change is
/// not one of these: it is the differ's `tabs.onUpdated`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GroupEvent {
    /// `tabGroups.onCreated(group)`.
    Created(Value),
    /// `tabGroups.onUpdated(group)`: its title or collapsed state changed.
    Updated(Value),
    /// `tabGroups.onRemoved(group)`, the group as it last was.
    Removed(Value),
}

impl GroupEvent {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            GroupEvent::Created(_) => "tabGroups.onCreated",
            GroupEvent::Updated(_) => "tabGroups.onUpdated",
            GroupEvent::Removed(_) => "tabGroups.onRemoved",
        }
    }
}

/// The group events of one strip change, in two parts: `before` goes
/// ahead of the tab events of the same change (a group exists before
/// tabs are said to be in it), `after` follows them (a group is gone
/// after the tab event that emptied it).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GroupEvents {
    pub before: Vec<GroupEvent>,
    pub after: Vec<GroupEvent>,
}

/// What changed about groups between two snapshots, whoever changed it.
pub(crate) fn group_events(prev: &GroupView, next: &GroupView) -> GroupEvents {
    let was = prev.groups();
    let now = next.groups();
    let mut events = GroupEvents::default();
    for (window, folder) in &now {
        match was.iter().find(|(_, f)| f.id == folder.id) {
            None => events
                .before
                .push(GroupEvent::Created(tab_group(*window, folder))),
            // Chrome has no event for a group changing windows: it is
            // removed there and created here.
            Some((old_window, old)) if old_window != window => {
                events
                    .before
                    .push(GroupEvent::Removed(tab_group(*old_window, old)));
                events
                    .before
                    .push(GroupEvent::Created(tab_group(*window, folder)));
            }
            Some(_) => {}
        }
    }
    for (window, folder) in &now {
        if let Some((old_window, old)) = was.iter().find(|(_, f)| f.id == folder.id)
            && old_window == window
            && *old != *folder
        {
            events
                .after
                .push(GroupEvent::Updated(tab_group(*window, folder)));
        }
    }
    for (window, folder) in &was {
        if !now.iter().any(|(_, f)| f.id == folder.id) {
            events
                .after
                .push(GroupEvent::Removed(tab_group(*window, folder)));
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use osmena_core::{ExtensionRequest, Folder, FolderId, TabId};
    use serde_json::{Value, json};

    use super::*;
    use crate::bridge::ids::TAB_GROUP_ID_NONE;

    fn folder(id: u64, name: &str, expanded: bool) -> Folder {
        Folder {
            id: FolderId(id),
            name: name.to_string(),
            expanded,
        }
    }

    /// Tab `n` of the model is Chromium's tab `100 + n`.
    fn tab(n: u64, folder: Option<u64>) -> GroupTab {
        GroupTab {
            tab: TabId(n),
            id: Some(100 + n as i64),
            folder: folder.map(FolderId),
        }
    }

    fn window(id: i64, folders: Vec<Folder>, tabs: Vec<GroupTab>) -> GroupWindow {
        GroupWindow { id, folders, tabs }
    }

    /// Window 1: tab 1 loose, tabs 2 and 3 in "Research" (7, expanded).
    /// Window 2: tab 4 in "Later" (8, collapsed), tab 5 loose.
    fn two_windows() -> GroupView {
        GroupView {
            windows: vec![
                window(
                    1,
                    vec![folder(7, "Research", true)],
                    vec![tab(1, None), tab(2, Some(7)), tab(3, Some(7))],
                ),
                window(
                    2,
                    vec![folder(8, "Later", false)],
                    vec![tab(4, Some(8)), tab(5, None)],
                ),
            ],
        }
    }

    /// A call from window 1, each tab named by the id it has now.
    fn ask(view: &GroupView, namespace: &str, method: &str, args: Value) -> Option<GroupRoute> {
        let args = args.as_array().cloned().unwrap_or_default();
        route(
            view,
            &GroupCall {
                namespace,
                method,
                args: &args,
                current_window: Some(1),
                session_name: "Probe Session",
                tab_of: &|id| {
                    view.windows
                        .iter()
                        .flat_map(|w| &w.tabs)
                        .find(|t| t.id == Some(id))
                        .map(|t| t.tab)
                },
            },
        )
    }

    fn research() -> Value {
        json!({"id": 7, "collapsed": false, "color": "grey", "title": "Research",
               "windowId": 1, "shared": false})
    }

    fn later() -> Value {
        json!({"id": 8, "collapsed": true, "color": "grey", "title": "Later",
               "windowId": 2, "shared": false})
    }

    #[test]
    fn a_group_id_is_the_folders_number_and_minus_one_is_no_group() {
        assert_eq!(group_id(Some(FolderId(7))), 7);
        assert_eq!(group_id(None), TAB_GROUP_ID_NONE);
        assert_eq!(folder_of(7), Some(FolderId(7)));
        assert_eq!(folder_of(TAB_GROUP_ID_NONE), None);
        assert_eq!(folder_of(-5), None);
    }

    #[test]
    fn a_folder_reads_as_a_grey_unshared_tab_group() {
        assert_eq!(tab_group(1, &folder(7, "Research", true)), research());
        assert_eq!(tab_group(2, &folder(8, "Later", false)), later());
    }

    #[test]
    fn get_answers_from_the_snapshot_or_rejects_with_chromiums_words() {
        let view = two_windows();
        assert_eq!(
            ask(&view, "tabGroups", "get", json!([7])),
            Some(GroupRoute::Answer(research()))
        );
        assert_eq!(
            ask(&view, "tabGroups", "get", json!([99])),
            Some(GroupRoute::Reject("No group with id: 99.".to_string()))
        );
        assert_eq!(
            ask(&view, "tabGroups", "get", json!(["seven"])),
            Some(GroupRoute::Reject(
                "Invalid arguments to tabGroups.get.".to_string()
            ))
        );
    }

    #[test]
    fn query_filters_by_window_state_colour_and_title() {
        let view = two_windows();
        let both = json!([research(), later()]);
        for (filter, expected) in [
            (json!({}), both.clone()),
            (json!({"windowId": 2}), json!([later()])),
            // The window the call came from.
            (json!({"windowId": -2}), json!([research()])),
            (json!({"windowId": 44}), json!([])),
            (json!({"collapsed": true}), json!([later()])),
            (json!({"collapsed": false}), json!([research()])),
            (json!({"color": "grey"}), both.clone()),
            (json!({"color": "blue"}), json!([])),
            (json!({"shared": false}), both.clone()),
            (json!({"shared": true}), json!([])),
            (json!({"title": "Later"}), json!([later()])),
            (json!({"title": "Re*"}), json!([research()])),
            (json!({"title": "?ater"}), json!([later()])),
            (json!({"title": "later"}), json!([])),
            (json!({"title": "Re*", "windowId": 2}), json!([])),
        ] {
            assert_eq!(
                ask(&view, "tabGroups", "query", json!([filter.clone()])),
                Some(GroupRoute::Answer(expected)),
                "{filter}"
            );
        }
    }

    #[test]
    fn a_folder_without_tabs_is_no_group() {
        let view = GroupView {
            windows: vec![window(
                1,
                vec![folder(7, "Empty", true)],
                vec![tab(1, None)],
            )],
        };
        assert_eq!(
            ask(&view, "tabGroups", "query", json!([{}])),
            Some(GroupRoute::Answer(json!([])))
        );
    }

    #[test]
    fn update_becomes_update_folder_and_resolves_the_group_as_it_will_be() {
        let view = two_windows();
        assert_eq!(
            ask(
                &view,
                "tabGroups",
                "update",
                json!([7, {"title": "Trip", "color": "blue"}])
            ),
            Some(GroupRoute::Request {
                request: ExtensionRequest::UpdateFolder {
                    folder: FolderId(7),
                    name: Some("Trip".to_string()),
                    expanded: None,
                },
                then: GroupThen::Updated {
                    folder: FolderId(7),
                    title: Some("Trip".to_string()),
                    collapsed: None,
                    group: json!({"id": 7, "collapsed": false, "color": "grey",
                        "title": "Trip", "windowId": 1, "shared": false}),
                },
            })
        );
        assert_eq!(
            ask(
                &view,
                "tabGroups",
                "update",
                json!([7, {"collapsed": true}])
            ),
            Some(GroupRoute::Request {
                request: ExtensionRequest::UpdateFolder {
                    folder: FolderId(7),
                    name: None,
                    expanded: Some(false),
                },
                then: GroupThen::Updated {
                    folder: FolderId(7),
                    title: None,
                    collapsed: Some(true),
                    group: json!({"id": 7, "collapsed": true, "color": "grey",
                        "title": "Research", "windowId": 1, "shared": false}),
                },
            })
        );
    }

    #[test]
    fn a_blank_title_resolves_as_the_session_name_the_model_will_set() {
        let view = two_windows();
        let Some(GroupRoute::Request { request, then }) =
            ask(&view, "tabGroups", "update", json!([8, {"title": "  "}]))
        else {
            panic!("not a request");
        };
        assert_eq!(
            request,
            ExtensionRequest::UpdateFolder {
                folder: FolderId(8),
                name: Some("  ".to_string()),
                expanded: None,
            }
        );
        assert_eq!(
            then,
            // The name is the model's to give: the strip is not waited
            // for by a title.
            GroupThen::Updated {
                folder: FolderId(8),
                title: None,
                collapsed: None,
                group: json!({"id": 8, "collapsed": true, "color": "grey",
                    "title": "Probe Session", "windowId": 2, "shared": false}),
            }
        );
    }

    #[test]
    fn an_update_of_colour_alone_changes_nothing_and_answers_the_group() {
        let view = two_windows();
        assert_eq!(
            ask(&view, "tabGroups", "update", json!([7, {"color": "red"}])),
            Some(GroupRoute::Answer(research()))
        );
        assert_eq!(
            ask(&view, "tabGroups", "update", json!([99, {"title": "x"}])),
            Some(GroupRoute::Reject("No group with id: 99.".to_string()))
        );
    }

    #[test]
    fn move_is_refused_and_other_methods_are_left_to_the_router() {
        let view = two_windows();
        assert_eq!(
            ask(&view, "tabGroups", "move", json!([7, {"index": 0}])),
            Some(GroupRoute::Reject(
                "tabGroups.move is not supported.".to_string()
            ))
        );
        assert_eq!(ask(&view, "tabGroups", "frobnicate", json!([])), None);
        assert_eq!(ask(&view, "tabs", "query", json!([{}])), None);
    }

    #[test]
    fn tabs_group_becomes_group_tabs_and_resolves_the_group_id() {
        let view = two_windows();
        // A new group: Chromium's ids 101 and 102 are tabs 1 and 2.
        assert_eq!(
            ask(&view, "tabs", "group", json!([{"tabIds": [101, 102, 101]}])),
            Some(GroupRoute::Request {
                request: ExtensionRequest::GroupTabs {
                    tabs: vec![TabId(1), TabId(2)],
                    folder: None,
                },
                then: GroupThen::GroupId {
                    tabs: vec![TabId(1), TabId(2)],
                },
            })
        );
        // One id, not in a list, into an existing group.
        assert_eq!(
            ask(
                &view,
                "tabs",
                "group",
                json!([{"tabIds": 101, "groupId": 7}])
            ),
            Some(GroupRoute::Request {
                request: ExtensionRequest::GroupTabs {
                    tabs: vec![TabId(1)],
                    folder: Some(FolderId(7)),
                },
                then: GroupThen::GroupId {
                    tabs: vec![TabId(1)],
                },
            })
        );
        // The tabs' own window, named.
        assert!(matches!(
            ask(
                &view,
                "tabs",
                "group",
                json!([{"tabIds": [104], "createProperties": {"windowId": 2}}])
            ),
            Some(GroupRoute::Request { .. })
        ));
    }

    #[test]
    fn tabs_group_rejects_what_chromium_rejects_and_what_folders_cannot_do() {
        let view = two_windows();
        for (options, message) in [
            (json!({"tabIds": [999]}), "No tab with id: 999."),
            (
                json!({"tabIds": [101], "groupId": 99}),
                "No group with id: 99.",
            ),
            // The group is looked up before the tabs, as in Chromium.
            (
                json!({"tabIds": [999], "groupId": 99}),
                "No group with id: 99.",
            ),
            (
                json!({"tabIds": [101], "createProperties": {"windowId": 44}}),
                "No window with id: 44.",
            ),
            (
                json!({"tabIds": [101], "groupId": 7, "createProperties": {}}),
                "Cannot specify 'createProperties' along with a 'groupId'.",
            ),
            (
                json!({"tabIds": [101, 104]}),
                "Tabs can only be grouped within their own window.",
            ),
            (
                json!({"tabIds": [104], "groupId": 7}),
                "Tabs can only be grouped within their own window.",
            ),
            (
                json!({"tabIds": [101], "createProperties": {"windowId": 2}}),
                "Tabs can only be grouped within their own window.",
            ),
            (json!({"tabIds": []}), "Invalid arguments to tabs.group."),
            (json!({}), "Invalid arguments to tabs.group."),
        ] {
            assert_eq!(
                ask(&view, "tabs", "group", json!([options.clone()])),
                Some(GroupRoute::Reject(message.to_string())),
                "{options}"
            );
        }
    }

    #[test]
    fn ungroup_becomes_ungroup_tabs() {
        let view = two_windows();
        assert_eq!(
            ask(&view, "tabs", "ungroup", json!([[102, 103]])),
            Some(GroupRoute::Request {
                request: ExtensionRequest::UngroupTabs(vec![TabId(2), TabId(3)]),
                then: GroupThen::Ungrouped {
                    tabs: vec![TabId(2), TabId(3)],
                },
            })
        );
        assert_eq!(
            ask(&view, "tabs", "ungroup", json!([999])),
            Some(GroupRoute::Reject("No tab with id: 999.".to_string()))
        );
    }

    #[test]
    fn a_tab_without_a_chromium_id_cannot_be_named() {
        let mut view = two_windows();
        view.windows[0].tabs[0].id = None;
        assert_eq!(
            ask(&view, "tabs", "ungroup", json!([101])),
            Some(GroupRoute::Reject("No tab with id: 101.".to_string()))
        );
    }

    #[test]
    fn a_former_tab_id_still_names_its_tab() {
        let view = two_windows();
        let args = [json!({"tabIds": [900, 101]})];
        // 900 is the id tab 1 had before Chromium's replaced it.
        let routed = route(
            &view,
            &GroupCall {
                namespace: "tabs",
                method: "group",
                args: &args,
                current_window: Some(1),
                session_name: "Probe Session",
                tab_of: &|id| match id {
                    900 | 101 => Some(TabId(1)),
                    // A tab the ids still know and the strip no longer has.
                    901 => Some(TabId(77)),
                    _ => None,
                },
            },
        );
        assert_eq!(
            routed,
            Some(GroupRoute::Request {
                request: ExtensionRequest::GroupTabs {
                    tabs: vec![TabId(1)],
                    folder: None,
                },
                then: GroupThen::GroupId {
                    tabs: vec![TabId(1)],
                },
            })
        );
        let gone = [json!([901])];
        assert_eq!(
            route(
                &view,
                &GroupCall {
                    namespace: "tabs",
                    method: "ungroup",
                    args: &gone,
                    current_window: Some(1),
                    session_name: "Probe Session",
                    tab_of: &|_| Some(TabId(77)),
                },
            ),
            Some(GroupRoute::Reject("No tab with id: 901.".to_string()))
        );
    }

    /// What the Claude extension calls each time it opens its side panel,
    /// with its argument shapes.
    #[test]
    fn the_calls_of_a_panel_opening_resolve_with_chromes_shapes() {
        let view = two_windows();
        // tabs.group({tabIds: [tab], createProperties: {windowId}}), the
        // window being the tab's own.
        assert_eq!(
            ask(
                &view,
                "tabs",
                "group",
                json!([{"tabIds": [101], "createProperties": {"windowId": 1}}])
            ),
            Some(GroupRoute::Request {
                request: ExtensionRequest::GroupTabs {
                    tabs: vec![TabId(1)],
                    folder: None,
                },
                then: GroupThen::GroupId {
                    tabs: vec![TabId(1)],
                },
            })
        );
        // tabGroups.update(id, {title, color, collapsed: false}): the
        // colour is taken and the group stays grey.
        assert_eq!(
            ask(
                &view,
                "tabGroups",
                "update",
                json!([8, {"title": "Claude", "color": "orange", "collapsed": false}])
            ),
            Some(GroupRoute::Request {
                request: ExtensionRequest::UpdateFolder {
                    folder: FolderId(8),
                    name: Some("Claude".to_string()),
                    expanded: Some(true),
                },
                then: GroupThen::Updated {
                    folder: FolderId(8),
                    title: Some("Claude".to_string()),
                    collapsed: Some(false),
                    group: json!({"id": 8, "collapsed": false, "color": "grey",
                        "title": "Claude", "windowId": 2, "shared": false}),
                },
            })
        );
        // A tab the assistant opens joins the group: one id, not a list.
        assert_eq!(
            ask(
                &view,
                "tabs",
                "group",
                json!([{"tabIds": 101, "groupId": 7}])
            ),
            Some(GroupRoute::Request {
                request: ExtensionRequest::GroupTabs {
                    tabs: vec![TabId(1)],
                    folder: Some(FolderId(7)),
                },
                then: GroupThen::GroupId {
                    tabs: vec![TabId(1)],
                },
            })
        );
        // tabs.ungroup([tab]) for a tab in someone else's group.
        assert_eq!(
            ask(&view, "tabs", "ungroup", json!([[102]])),
            Some(GroupRoute::Request {
                request: ExtensionRequest::UngroupTabs(vec![TabId(2)]),
                then: GroupThen::Ungrouped {
                    tabs: vec![TabId(2)],
                },
            })
        );
        // tabGroups.get(id) and tabGroups.query({}).
        let Some(GroupRoute::Answer(group)) = ask(&view, "tabGroups", "get", json!([7])) else {
            panic!("not an answer");
        };
        let mut keys: Vec<&str> = group
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["collapsed", "color", "id", "shared", "title", "windowId"]
        );
        assert!(group["id"].is_i64() && group["windowId"].is_i64());
        assert_eq!(
            ask(&view, "tabGroups", "query", json!([{}])),
            Some(GroupRoute::Answer(json!([research(), later()])))
        );
    }

    use GroupEvent::{Created, Removed, Updated};

    fn one_window(folders: Vec<Folder>, tabs: Vec<GroupTab>) -> GroupView {
        GroupView {
            windows: vec![window(1, folders, tabs)],
        }
    }

    fn group_json(id: u64, title: &str, collapsed: bool, window: i64) -> Value {
        json!({"id": id, "collapsed": collapsed, "color": "grey", "title": title,
               "windowId": window, "shared": false})
    }

    /// A name, the strip before and after, and the events `before` and
    /// `after` the tab events.
    type Case = (
        &'static str,
        GroupView,
        GroupView,
        Vec<GroupEvent>,
        Vec<GroupEvent>,
    );

    #[test]
    fn the_group_events_of_a_change() {
        let loose = || one_window(vec![], vec![tab(1, None), tab(2, None), tab(3, None)]);
        let grouped = || {
            one_window(
                vec![folder(7, "Probe Session", true)],
                vec![tab(1, None), tab(2, Some(7)), tab(3, Some(7))],
            )
        };
        let session = || group_json(7, "Probe Session", false, 1);
        let cases: Vec<Case> = vec![
            ("nothing changed", grouped(), grouped(), vec![], vec![]),
            (
                "two tabs grouped",
                loose(),
                grouped(),
                vec![Created(session())],
                vec![],
            ),
            (
                "renamed",
                grouped(),
                one_window(
                    vec![folder(7, "Trip", true)],
                    vec![tab(1, None), tab(2, Some(7)), tab(3, Some(7))],
                ),
                vec![],
                vec![Updated(group_json(7, "Trip", false, 1))],
            ),
            (
                "collapsed",
                grouped(),
                one_window(
                    vec![folder(7, "Probe Session", false)],
                    vec![tab(1, None), tab(2, Some(7)), tab(3, Some(7))],
                ),
                vec![],
                vec![Updated(group_json(7, "Probe Session", true, 1))],
            ),
            (
                "a tab moved into a folder",
                grouped(),
                one_window(
                    vec![folder(7, "Probe Session", true)],
                    vec![tab(1, Some(7)), tab(2, Some(7)), tab(3, Some(7))],
                ),
                vec![],
                vec![],
            ),
            (
                "a tab moved out",
                grouped(),
                one_window(
                    vec![folder(7, "Probe Session", true)],
                    vec![tab(1, None), tab(2, Some(7)), tab(3, None)],
                ),
                vec![],
                vec![],
            ),
            (
                "the last tab ungrouped",
                grouped(),
                loose(),
                vec![],
                vec![Removed(session())],
            ),
            (
                "the last tab closed",
                one_window(
                    vec![folder(7, "Probe Session", true)],
                    vec![tab(1, None), tab(2, Some(7))],
                ),
                one_window(vec![], vec![tab(1, None)]),
                vec![],
                vec![Removed(session())],
            ),
            (
                "a tab opened inside a folder",
                grouped(),
                one_window(
                    vec![folder(7, "Probe Session", true)],
                    vec![
                        tab(1, None),
                        tab(2, Some(7)),
                        tab(3, Some(7)),
                        tab(4, Some(7)),
                    ],
                ),
                vec![],
                vec![],
            ),
            (
                "a tab moved from one folder to another, emptying the first",
                one_window(
                    vec![folder(7, "A", true), folder(8, "B", true)],
                    vec![tab(1, Some(7)), tab(2, Some(8))],
                ),
                one_window(
                    vec![folder(8, "B", true)],
                    vec![tab(1, Some(8)), tab(2, Some(8))],
                ),
                vec![],
                vec![Removed(group_json(7, "A", false, 1))],
            ),
            (
                "a folder carried to another window is removed and created",
                GroupView {
                    windows: vec![
                        window(1, vec![folder(7, "A", true)], vec![tab(1, Some(7))]),
                        window(2, vec![], vec![tab(2, None)]),
                    ],
                },
                GroupView {
                    windows: vec![window(
                        2,
                        vec![folder(7, "A", true)],
                        vec![tab(2, None), tab(1, Some(7))],
                    )],
                },
                vec![
                    Removed(group_json(7, "A", false, 1)),
                    Created(group_json(7, "A", false, 2)),
                ],
                vec![],
            ),
        ];
        for (name, prev, next, before, after) in cases {
            assert_eq!(
                group_events(&prev, &next),
                GroupEvents { before, after },
                "{name}"
            );
        }
    }

    #[test]
    fn events_name_the_chrome_event_they_fire() {
        assert_eq!(Created(json!({})).name(), "tabGroups.onCreated");
        assert_eq!(Updated(json!({})).name(), "tabGroups.onUpdated");
        assert_eq!(Removed(json!({})).name(), "tabGroups.onRemoved");
    }
}
