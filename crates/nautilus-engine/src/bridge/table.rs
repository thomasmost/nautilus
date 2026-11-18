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
