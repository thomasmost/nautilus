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
use serde_json::Value;

use super::pages::extension_of;
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

/// The outcome of a page's report of its own Chromium tab id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Report {
    /// The id is now the view's.
    Recorded,
    /// The view already has this id.
    Same,
    /// The report is ignored, and why.
    Refused(&'static str),
}

/// The table the engine keeps: each tab's target id, and the rows of
/// the last join.
#[derive(Default)]
pub(crate) struct TabTable {
    targets: HashMap<TabId, String>,
    rows: Vec<Row>,
    /// The listing of the last join: the targets a report is checked against.
    listed: Vec<Target>,
    /// Each extension page's own report of its Chromium tab id, with the
    /// extension it was made from. A report is a row only while its view is
    /// still that extension's page (`prune_reports`).
    reported: HashMap<TabId, (i64, String)>,
}

impl TabTable {
    /// `tab`'s own browser reported its target id.
    pub(crate) fn set_target(&mut self, tab: TabId, target: String) {
        self.targets.insert(tab, target);
    }

    /// The page of `tab` reported its own Chromium tab id. Refused: `tab`
    /// has no target yet, or is not a page of `extension`; it already has an
    /// id from any source, which must be the same; the listing names the id
    /// for another target; or another tab has it.
    pub(crate) fn report(&mut self, tab: TabId, chromium: i64, extension: &str) -> Report {
        if let Some(current) = self.chromium(tab) {
            return if current == chromium {
                Report::Same
            } else {
                Report::Refused("the tab already has a Chromium id")
            };
        }
        let Some(id) = self.targets.get(&tab) else {
            return Report::Refused("the view has no DevTools target yet");
        };
        let own_page = self
            .listed_target(tab)
            .is_some_and(|target| extension_of(&target.url).as_deref() == Some(extension));
        if !own_page {
            return Report::Refused("the view is not a page of that extension");
        }
        if self
            .listed
            .iter()
            .any(|target| target.tab_id == Some(chromium) && &target.id != id)
        {
            return Report::Refused("the listing names the id for another target");
        }
        if self.tab(chromium).is_some() {
            return Report::Refused("another tab has the id");
        }
        self.reported.insert(tab, (chromium, extension.to_string()));
        self.rows = self.rows_of();
        Report::Recorded
    }

    /// `tab` closed.
    pub(crate) fn forget(&mut self, tab: TabId) {
        self.targets.remove(&tab);
        self.reported.remove(&tab);
        self.rows.retain(|row| row.tab != tab);
    }

    /// The helper listed `targets`: the rows are joined afresh.
    pub(crate) fn join(&mut self, targets: &[Target]) {
        self.listed = targets.to_vec();
        self.prune_reports();
        self.rows = self.rows_of();
    }

    /// Drop a report whose view is no longer a page of the extension it was
    /// made from: it was closed, or it navigated elsewhere.
    fn prune_reports(&mut self) {
        let listed = &self.listed;
        let targets = &self.targets;
        self.reported.retain(|tab, (_, extension)| {
            targets
                .get(tab)
                .and_then(|id| listed.iter().find(|target| &target.id == id))
                .is_some_and(|target| {
                    extension_of(&target.url).as_deref() == Some(extension.as_str())
                })
        });
    }

    /// `tab`'s own target in the last listing, if it is listed.
    fn listed_target(&self, tab: TabId) -> Option<&Target> {
        let id = self.targets.get(&tab)?;
        self.listed.iter().find(|target| &target.id == id)
    }

    /// The rows of the last listing, with each report that no row of the
    /// listing already names. A listed row (the helper's) always wins.
    fn rows_of(&self) -> Vec<Row> {
        let views: Vec<(TabId, String)> = self
            .targets
            .iter()
            .map(|(tab, id)| (*tab, id.clone()))
            .collect();
        let mut rows = join(&views, &self.listed);
        for (tab, (chromium, _)) in &self.reported {
            if rows
                .iter()
                .any(|row| row.tab == *tab || row.chromium == *chromium)
            {
                continue;
            }
            let Some(target) = self.listed_target(*tab) else {
                continue;
            };
            rows.push(Row {
                tab: *tab,
                chromium: *chromium,
                url: target.url.clone(),
                title: target.title.clone(),
            });
        }
        rows.sort_by_key(|row| row.tab);
        rows
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}

#[cfg(test)]
mod report_tests {
    use super::*;

    const ID: &str = "abcdefghijklmnopabcdefghijklmnop";
    const EXT: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop/p5page.html";
    const OTHER_EXT: &str = "chrome-extension://ponmlkjihgfedcbaponmlkjihgfedcba/p5page.html";

    fn view_page(id: &str, url: &str) -> Target {
        Target {
            id: id.into(),
            tab_id: None,
            kind: "other".into(),
            url: url.into(),
            title: "t".into(),
        }
    }

    fn tab_page(id: &str, tab_id: i64, url: &str) -> Target {
        Target {
            id: id.into(),
            tab_id: Some(tab_id),
            kind: "page".into(),
            url: url.into(),
            title: "t".into(),
        }
    }

    /// View 5 on target "V" at `url`, joined against the listing.
    fn table_with(url: &str, extra: Vec<Target>) -> (TabTable, Vec<Target>) {
        let mut targets = vec![view_page("V", url)];
        targets.extend(extra);
        let mut table = TabTable::default();
        table.set_target(TabId(5), "V".to_string());
        table.join(&targets);
        (table, targets)
    }

    #[test]
    fn a_report_names_the_view_of_an_extension_page_and_a_listing_keeps_it() {
        let (mut table, targets) = table_with(EXT, vec![]);
        assert_eq!(table.report(TabId(5), 433, ID), Report::Recorded);
        assert_eq!(table.chromium(TabId(5)), Some(433));
        assert_eq!(table.tab(433), Some(TabId(5)));
        assert_eq!(table.rows().len(), 1);
        table.join(&targets);
        assert_eq!(table.chromium(TabId(5)), Some(433));
    }

    #[test]
    fn a_second_report_of_the_same_id_is_the_same_and_a_different_one_is_refused() {
        let (mut table, _) = table_with(EXT, vec![]);
        table.report(TabId(5), 433, ID);
        assert_eq!(table.report(TabId(5), 433, ID), Report::Same);
        assert!(matches!(
            table.report(TabId(5), 434, ID),
            Report::Refused(_)
        ));
        assert_eq!(table.chromium(TabId(5)), Some(433));
    }

    #[test]
    fn an_id_the_helper_lists_for_another_target_is_refused() {
        let (mut free, _) = table_with(EXT, vec![]);
        assert_eq!(
            free.report(TabId(5), 433, ID),
            Report::Recorded,
            "the same id, not listed"
        );
        let (mut table, _) = table_with(EXT, vec![tab_page("W", 433, "https://example.com/")]);
        assert!(matches!(
            table.report(TabId(5), 433, ID),
            Report::Refused(_)
        ));
        assert_eq!(table.chromium(TabId(5)), None);
    }

    #[test]
    fn a_view_with_no_target_is_refused_and_the_helpers_row_wins_over_a_report() {
        let mut table = TabTable::default();
        assert!(
            matches!(table.report(TabId(5), 433, ID), Report::Refused(_)),
            "no target yet"
        );

        let mut table = TabTable::default();
        table.set_target(TabId(7), "W".to_string());
        table.join(&[tab_page("W", 433, "https://example.com/")]);
        assert_eq!(table.chromium(TabId(7)), Some(433));
        assert!(matches!(
            table.report(TabId(7), 999, ID),
            Report::Refused(_)
        ));
        assert_eq!(table.report(TabId(7), 433, ID), Report::Same);
        assert_eq!(table.chromium(TabId(7)), Some(433));
    }

    #[test]
    fn a_report_ends_when_its_view_leaves_the_extension_page_and_a_web_page_joins_as_before() {
        let (mut table, mut targets) = table_with(EXT, vec![]);
        table.report(TabId(5), 433, ID);

        targets[0] = view_page("V", "https://example.com/");
        table.join(&targets);
        assert_eq!(
            table.chromium(TabId(5)),
            None,
            "the report does not outlive the page"
        );

        targets[0] = view_page("V", OTHER_EXT);
        table.join(&targets);
        assert_eq!(
            table.chromium(TabId(5)),
            None,
            "another extension's page is not this one"
        );

        targets[0] = view_page("V", EXT);
        table.join(&targets);
        assert_eq!(
            table.report(TabId(5), 433, ID),
            Report::Recorded,
            "a new report after navigation back"
        );

        targets[0] = tab_page("V", 433, "https://example.com/");
        table.join(&targets);
        assert_eq!(
            table.chromium(TabId(5)),
            Some(433),
            "the helper's row joins the web page"
        );
    }

    #[test]
    fn closing_the_view_drops_its_report() {
        let (mut table, targets) = table_with(EXT, vec![]);
        assert_eq!(table.report(TabId(5), 433, ID), Report::Recorded);
        assert_eq!(table.chromium(TabId(5)), Some(433));
        table.forget(TabId(5));
        table.join(&targets);
        assert_eq!(table.chromium(TabId(5)), None);
        assert_eq!(table.tab(433), None);
    }
}
