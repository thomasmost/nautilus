//! The tab-id table: which Chromium tab id each Nautilus tab has.
//!
//! Chromium's id for a tab is a session id, new on every launch, that
//! CEF does not expose. Each tab's own browser knows its DevTools target
//! id (`Target.getTargetInfo`), and the helper extension's
//! `chrome.debugger.getTargets()` lists every target with Chromium's
//! tab id beside the same target id. The join of the two is the table.
//! Pure: the CEF calls that feed it are in `relay`.

use std::collections::HashMap;

use osmena_core::TabId;
use serde_json::{Value, json};

use super::policy::Session;

/// A worker's `hello` (`bridge.js`): the worker instance it is, and for each
/// Chromium tab its own debugger session's state (`sessions`). A tab the hello
/// does not name, or names with an unknown state, has no such evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hello {
    pub instance: String,
    pub sessions: Vec<(i64, Session)>,
}

/// Read a worker's hello. `None` when it names no instance (a malformed hello
/// is treated as no answer). An entry without a tab or a known state is left
/// out, which is no evidence about its tab.
pub(crate) fn hello_of(hello: &Value) -> Option<Hello> {
    let instance = hello.get("instance")?.as_str()?.to_string();
    let sessions = hello
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let tab = entry.get("tab")?.as_i64()?;
            let state = match entry.get("state")?.as_str()? {
                "attached" => Session::Attached,
                "not-attached" => Session::NotAttached,
                "unknown" => Session::Unknown,
                _ => return None,
            };
            Some((tab, state))
        })
        .collect();
    Some(Hello { instance, sessions })
}

/// The target id of `extension`'s worker in the helper's list: the target of
/// kind `worker` (what `chrome.debugger.getTargets` calls a service worker;
/// `service_worker` is accepted too) whose URL is inside the extension. The
/// first such target is taken; an extension has one service worker. `None`
/// when it is not listed (a stopped worker is not).
pub(crate) fn worker_target_of(targets: &[Target], extension: &str) -> Option<String> {
    let prefix = format!("chrome-extension://{extension}/");
    targets
        .iter()
        .find(|t| {
            matches!(t.kind.as_str(), "worker" | "service_worker") && t.url.starts_with(&prefix)
        })
        .map(|t| t.id.clone())
}

/// One DevTools target as the helper reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    /// The DevTools target id.
    pub id: String,
    /// Chromium's tab id; only pages have one.
    pub tab_id: Option<i64>,
    /// `page`, `worker`, `background_page` or `other`.
    pub kind: String,
    pub url: String,
    pub title: String,
}

/// The helper's answer (`nautilusHelper.targets()`) as targets; an
/// entry without a string `id` is skipped.
pub(crate) fn parse_targets(value: &Value) -> Vec<Target> {
    let text = |item: &Value, key: &str| {
        item.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_string();
            Some(Target {
                id,
                tab_id: item.get("tabId").and_then(|v| v.as_i64()),
                kind: text(item, "type"),
                url: text(item, "url"),
                title: text(item, "title"),
            })
        })
        .collect()
}

/// The target id in a `Target.getTargetInfo` result.
pub(crate) fn target_id_of(result: &Value) -> Option<String> {
    Some(
        result
            .get("targetInfo")?
            .get("targetId")?
            .as_str()?
            .to_string(),
    )
}

/// A Nautilus tab with the id Chromium's extension APIs know it by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub tab: TabId,
    pub chromium: i64,
    pub url: String,
    pub title: String,
}

/// Join the tabs' target ids with the helper's targets: one row per
/// tab whose target is a page with a tab id, in tab order. Targets
/// that are no Nautilus tab (the management page, relay pages, the
/// helper, workers) match nothing and are left out.
pub(crate) fn join(views: &[(TabId, String)], targets: &[Target]) -> Vec<Row> {
    let by_id: HashMap<&str, &Target> = targets
        .iter()
        .filter(|t| t.kind == "page" && t.tab_id.is_some())
        .map(|t| (t.id.as_str(), t))
        .collect();
    let mut rows: Vec<Row> = views
        .iter()
        .filter_map(|(tab, target)| {
            let found = by_id.get(target.as_str())?;
            Some(Row {
                tab: *tab,
                chromium: found.tab_id?,
                url: found.url.clone(),
                title: found.title.clone(),
            })
        })
        .collect();
    rows.sort_by_key(|row| row.tab);
    rows
}

/// The table the engine keeps: each tab's target id, and the rows of
/// the last join.
#[derive(Default)]
pub(crate) struct TabTable {
    targets: HashMap<TabId, String>,
    rows: Vec<Row>,
}

impl TabTable {
    /// `tab`'s own browser reported its target id.
    pub(crate) fn set_target(&mut self, tab: TabId, target: String) {
        self.targets.insert(tab, target);
    }

    /// `tab` closed.
    pub(crate) fn forget(&mut self, tab: TabId) {
        self.targets.remove(&tab);
        self.rows.retain(|row| row.tab != tab);
    }

    /// The helper listed `targets`: the rows are joined afresh.
    pub(crate) fn join(&mut self, targets: &[Target]) {
        let views: Vec<(TabId, String)> = self
            .targets
            .iter()
            .map(|(tab, id)| (*tab, id.clone()))
            .collect();
        self.rows = join(&views, targets);
    }

    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Chromium's id for `tab`, as of the last join.
    pub(crate) fn chromium(&self, tab: TabId) -> Option<i64> {
        self.rows
            .iter()
            .find(|row| row.tab == tab)
            .map(|row| row.chromium)
    }

    /// The Nautilus tab Chromium calls `chromium`, as of the last join
    /// (the reverse of `chromium`; a debugger report names the tab so).
    pub(crate) fn tab(&self, chromium: i64) -> Option<TabId> {
        self.rows
            .iter()
            .find(|row| row.chromium == chromium)
            .map(|row| row.tab)
    }

    /// The Nautilus tab a debuggee is, or `None` when it is not one or is
    /// not known yet. A tab id needs the last join (its row); a target id
    /// needs only the tab's own report of its target, which comes at the
    /// tab's creation, so a debugger on a new tab resolves before the join.
    pub(crate) fn resolve(&self, debuggee: &Debuggee) -> Option<TabId> {
        match debuggee {
            Debuggee::Tab(chromium) => self.tab(*chromium),
            Debuggee::Target(id) => self
                .targets
                .iter()
                .find(|(_, target)| target.as_str() == id)
                .map(|(tab, _)| *tab),
        }
    }
}

/// What a `chrome.debugger` report names: a tab by Chromium's id (an
/// attach or detach with `{tabId}`), or a DevTools target by its id (with
/// `{targetId}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Debuggee {
    Tab(i64),
    Target(String),
}

/// The debuggee in a report's first argument: a number, or an object with
/// a string `targetId`. Anything else names no debuggee.
pub(crate) fn debuggee_of(arg: Option<&Value>) -> Option<Debuggee> {
    match arg? {
        Value::Number(number) => number.as_i64().map(Debuggee::Tab),
        Value::Object(object) => object
            .get("targetId")?
            .as_str()
            .map(|id| Debuggee::Target(id.to_string())),
        _ => None,
    }
}

/// The answer to the bridge's `tabs.query` in Phase 1: every row, in
/// tab order, with the fields the bridge lays over Chromium's own tab
/// object. `windowId` is 1 for every tab and `index` is the row's
/// place until Phase 5 answers from the shell's strip.
pub(crate) fn query_rows(rows: &[Row], active: Option<TabId>) -> Value {
    Value::Array(
        rows.iter()
            .enumerate()
            .map(|(index, row)| {
                json!({
                    "id": row.chromium,
                    "index": index,
                    "windowId": 1,
                    "active": Some(row.tab) == active,
                    "url": row.url,
                    "title": row.title,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hello_reports_each_tab_as_attached_not_attached_or_unknown() {
        let hello = json!({"instance": "abc", "sessions": [
            {"tab": 5, "state": "attached"},
            {"tab": 9, "state": "not-attached"},
            {"tab": 11, "state": "unknown"},
        ]});
        assert_eq!(
            hello_of(&hello),
            Some(Hello {
                instance: "abc".to_string(),
                sessions: vec![
                    (5, Session::Attached),
                    (9, Session::NotAttached),
                    (11, Session::Unknown),
                ],
            })
        );
    }

    #[test]
    fn a_hello_without_an_instance_is_no_hello_and_a_tab_without_a_state_is_no_evidence() {
        assert_eq!(hello_of(&json!({"sessions": []})), None);
        assert_eq!(
            hello_of(
                &json!({"instance": "abc", "sessions": [{"tab": 5}, {"state": "not-attached"}, {"tab": 6, "state": "maybe"}]})
            ),
            Some(Hello {
                instance: "abc".to_string(),
                sessions: Vec::new(),
            }),
            "an entry missing its tab or its state, or with an unknown state, is left out"
        );
        assert_eq!(
            hello_of(&json!({"instance": "abc"})),
            Some(Hello {
                instance: "abc".to_string(),
                sessions: Vec::new(),
            }),
            "no sessions at all names no tab"
        );
        // An earlier build's hello (`attached` instead of `sessions`) is a hello
        // of its instance with no sessions: the identity counts, nothing is forgotten.
        assert_eq!(
            hello_of(&json!({"instance": "old", "attached": [5]})),
            Some(Hello {
                instance: "old".to_string(),
                sessions: Vec::new(),
            }),
            "the old shape is read as its instance and no sessions (M21)"
        );
    }

    #[test]
    fn the_worker_target_is_the_extensions_worker_as_the_helper_lists_it() {
        // chrome.debugger.getTargets lists an extension's service worker as
        // "worker" (the shape of the probe, `join` above); DevTools calls it
        // "service_worker". Either is the worker; the extension's page is not.
        let targets = parse_targets(&json!([
            {"id": "PAGE", "tabId": 3, "type": "page", "url": "chrome-extension://ext/popup.html"},
            {"id": "W1", "type": "worker", "url": "chrome-extension://ext/background/sw.js"},
            {"id": "W2", "type": "worker", "url": "chrome-extension://other/sw.js"},
        ]));
        assert_eq!(worker_target_of(&targets, "ext").as_deref(), Some("W1"));
        assert_eq!(worker_target_of(&targets, "other").as_deref(), Some("W2"));
        let devtools = parse_targets(&json!([
            {"id": "S1", "type": "service_worker", "url": "chrome-extension://ext/background/sw.js"},
        ]));
        assert_eq!(worker_target_of(&devtools, "ext").as_deref(), Some("S1"));
    }

    #[test]
    fn a_stopped_worker_has_no_target_and_a_page_or_other_is_not_one() {
        let targets = parse_targets(&json!([
            {"id": "P", "tabId": 3, "type": "page", "url": "chrome-extension://ext/background/sw.js"},
            {"id": "R", "type": "other", "url": "chrome-extension://ext/nautilus-bridge/relay.html"},
        ]));
        assert_eq!(worker_target_of(&targets, "ext"), None);
        assert_eq!(
            worker_target_of(&[], "ext"),
            None,
            "an idle worker is not listed"
        );
    }

    fn page(id: &str, tab_id: i64, url: &str) -> Target {
        Target {
            id: id.to_string(),
            tab_id: Some(tab_id),
            kind: "page".to_string(),
            url: url.to_string(),
            title: format!("title of {url}"),
        }
    }

    /// The shape of run I of the probe.
    fn probe_targets() -> Value {
        json!([
            {"id": "77511F4D", "tabId": null, "type": "worker",
             "url": "chrome-extension://llcnklgd/sw.js", "title": "Service Worker"},
            {"id": "0EDDB01E", "tabId": 982885542, "type": "page",
             "url": "https://www.iana.org/help/example-domains", "title": "Example Domains"},
            {"id": "D2C9E333", "tabId": 982885541, "type": "page",
             "url": "https://example.org/", "title": "Example Domain"},
            {"id": "1FFE3731", "tabId": 982885539, "type": "page",
             "url": "chrome://extensions/", "title": "Extensions"},
            {"id": "2504BB17", "type": "other",
             "url": "chrome-extension://llcnklgd/page.html", "title": "Probe"},
            {"tabId": 7, "type": "page", "url": "https://no-id.example/"}
        ])
    }

    #[test]
    fn the_helpers_answer_is_parsed_and_an_entry_without_an_id_is_skipped() {
        let targets = parse_targets(&probe_targets());
        assert_eq!(targets.len(), 5);
        assert_eq!(targets[0].tab_id, None);
        assert_eq!(targets[0].kind, "worker");
        assert_eq!(
            targets[2],
            page("D2C9E333", 982885541, "https://example.org/").with_title("Example Domain")
        );
        assert_eq!(targets[4].tab_id, None);
        assert_eq!(parse_targets(&json!({"not": "a list"})), Vec::new());
    }

    impl Target {
        fn with_title(mut self, title: &str) -> Target {
            self.title = title.to_string();
            self
        }
    }

    #[test]
    fn the_target_id_is_read_from_a_get_target_info_result() {
        let result =
            json!({"targetInfo": {"targetId": "D2C9E333", "type": "page", "attached": true}});
        assert_eq!(target_id_of(&result).as_deref(), Some("D2C9E333"));
        assert_eq!(target_id_of(&json!({})), None);
    }

    #[test]
    fn the_join_keeps_nautilus_tabs_only_in_tab_order() {
        let views = vec![
            (TabId(3), "0EDDB01E".to_string()),
            (TabId(2), "D2C9E333".to_string()),
            // A tab whose target the helper has not listed yet.
            (TabId(4), "FFFFFFFF".to_string()),
        ];
        let rows = join(&views, &parse_targets(&probe_targets()));
        assert_eq!(
            rows.iter().map(|r| (r.tab, r.chromium)).collect::<Vec<_>>(),
            vec![(TabId(2), 982885541), (TabId(3), 982885542)]
        );
        assert_eq!(rows[0].url, "https://example.org/");
        assert_eq!(rows[0].title, "Example Domain");
    }

    #[test]
    fn a_target_that_is_no_page_or_has_no_tab_id_matches_no_tab() {
        let views = vec![
            (TabId(1), "77511F4D".to_string()),
            (TabId(2), "2504BB17".to_string()),
        ];
        assert_eq!(join(&views, &parse_targets(&probe_targets())), Vec::new());
    }

    #[test]
    fn the_table_looks_up_both_ways_and_forgets_a_closed_tab() {
        let mut table = TabTable::default();
        table.set_target(TabId(2), "D2C9E333".to_string());
        table.set_target(TabId(3), "0EDDB01E".to_string());
        assert_eq!(
            table.chromium(TabId(2)),
            None,
            "nothing is known before a join"
        );
        table.join(&parse_targets(&probe_targets()));
        assert_eq!(table.chromium(TabId(2)), Some(982885541));
        assert_eq!(table.tab(982885542), Some(TabId(3)));
        assert_eq!(table.tab(982885539), None, "the management page is no tab");
        table.forget(TabId(2));
        assert_eq!(table.chromium(TabId(2)), None);
        assert_eq!(table.rows().len(), 1);
        // A later join does not bring it back.
        table.join(&parse_targets(&probe_targets()));
        assert_eq!(table.rows().len(), 1);
    }

    #[test]
    fn a_debugger_report_names_a_tab_by_id_or_a_target_by_its_id() {
        assert_eq!(debuggee_of(Some(&json!(41))), Some(Debuggee::Tab(41)));
        assert_eq!(
            debuggee_of(Some(&json!({"targetId": "D2C9E333"}))),
            Some(Debuggee::Target("D2C9E333".to_string()))
        );
        assert_eq!(debuggee_of(Some(&json!({"extensionId": "x"}))), None);
        assert_eq!(debuggee_of(Some(&json!("41"))), None);
        assert_eq!(debuggee_of(None), None);
    }

    #[test]
    fn a_target_that_is_a_nautilus_tab_resolves_to_it_and_one_that_is_not_does_not() {
        let mut table = TabTable::default();
        table.set_target(TabId(2), "D2C9E333".to_string());
        // A Nautilus tab: its own report of the target id is enough.
        assert_eq!(
            table.resolve(&Debuggee::Target("D2C9E333".to_string())),
            Some(TabId(2))
        );
        // The worker and the management page are no tab of the table.
        assert_eq!(
            table.resolve(&Debuggee::Target("77511F4D".to_string())),
            None
        );
        assert_eq!(table.resolve(&Debuggee::Tab(982885539)), None);
    }

    #[test]
    fn a_tab_id_the_table_has_not_joined_yet_resolves_to_nothing_until_the_join() {
        let mut table = TabTable::default();
        table.set_target(TabId(2), "D2C9E333".to_string());
        assert_eq!(
            table.resolve(&Debuggee::Tab(982885541)),
            None,
            "a Chromium id is known only through the helper's list"
        );
        table.join(&parse_targets(&probe_targets()));
        assert_eq!(table.resolve(&Debuggee::Tab(982885541)), Some(TabId(2)));
    }

    #[test]
    fn a_target_whose_tab_has_not_reported_its_id_is_not_a_tab_yet() {
        let table = TabTable::default();
        assert_eq!(
            table.resolve(&Debuggee::Target("D2C9E333".to_string())),
            None
        );
    }

    #[test]
    fn a_query_lists_every_row_with_chromiums_id_and_marks_the_active_tab() {
        let rows = vec![
            Row {
                tab: TabId(2),
                chromium: 41,
                url: "https://a/".into(),
                title: "A".into(),
            },
            Row {
                tab: TabId(5),
                chromium: 42,
                url: "https://b/".into(),
                title: "B".into(),
            },
        ];
        assert_eq!(
            query_rows(&rows, Some(TabId(5))),
            json!([
                {"id": 41, "index": 0, "windowId": 1, "active": false, "url": "https://a/", "title": "A"},
                {"id": 42, "index": 1, "windowId": 1, "active": true, "url": "https://b/", "title": "B"},
            ])
        );
        assert_eq!(query_rows(&[], None), json!([]));
    }
}
