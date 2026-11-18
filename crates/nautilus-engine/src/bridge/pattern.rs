//! Match patterns, title globs, and what an extension's manifest lets it see.

use serde_json::Value;
use url::Url;

/// Whether `text` matches `pattern`, in which `*` stands for any run of
/// characters and everything else for itself, case-sensitively (the rule
/// for `tabs.query`'s `title` and for the path of a match pattern).
pub(crate) fn glob(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let mut parts: Vec<&str> = parts.collect();
    let Some(last) = parts.pop() else {
        // No star at all: the whole text must be the pattern.
        return rest.is_empty();
    };
    for part in parts {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// The schemes `<all_urls>` covers for a tab's address.
const ALL_URLS_SCHEMES: [&str; 6] = ["http", "https", "file", "ftp", "ws", "wss"];

/// Whether `url` falls under the match pattern `pattern`
/// (`<scheme>://<host><path>`, or `<all_urls>`). A scheme of `*` is `http`
/// or `https`; a host of `*` is any, `*.x` is `x` and its subdomains; a
/// port in the pattern is not compared; the path is a glob over the URL's
/// path and query. Anything that is no pattern matches nothing.
pub(crate) fn matches_url(pattern: &str, url: &Url) -> bool {
    if pattern == "<all_urls>" {
        return ALL_URLS_SCHEMES.contains(&url.scheme());
    }
    let Some((scheme, rest)) = pattern.split_once("://") else {
        return false;
    };
    let scheme_matches = match scheme {
        "*" => matches!(url.scheme(), "http" | "https"),
        other => other == url.scheme(),
    };
    if !scheme_matches {
        return false;
    }
    let Some(slash) = rest.find('/') else {
        return false;
    };
    let (host, path) = rest.split_at(slash);
    let host = host.split(':').next().unwrap_or_default();
    let have = url.host_str().unwrap_or_default();
    let host_matches = if host == "*" || (scheme == "file" && host.is_empty()) {
        true
    } else if let Some(domain) = host.strip_prefix("*.") {
        have == domain || have.ends_with(&format!(".{domain}"))
    } else {
        have == host
    };
    if !host_matches {
        return false;
    }
    let mut target = url.path().to_string();
    if let Some(query) = url.query() {
        target.push('?');
        target.push_str(query);
    }
    glob(path, &target)
}

/// What an extension's manifest entitles it to learn about tabs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Grants {
    /// The `tabs` permission: every tab's address and title.
    pub tabs: bool,
    /// `<all_urls>` among its host permissions (`captureVisibleTab`).
    pub all_urls: bool,
    /// Its host permissions, as match patterns.
    pub hosts: Vec<String>,
}

impl Grants {
    /// Read from a parsed `manifest.json`: `permissions` (which in
    /// Manifest V2 also lists host patterns) and `host_permissions`.
    /// Optional permissions are not counted, nor is `activeTab`: neither
    /// is in effect without something the bridge cannot see.
    pub(crate) fn from_manifest(manifest: &Value) -> Grants {
        let list = |key: &str| -> Vec<&str> {
            manifest
                .get(key)
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default()
        };
        let permissions = list("permissions");
        let hosts: Vec<String> = permissions
            .iter()
            .chain(list("host_permissions").iter())
            .filter(|entry| **entry == "<all_urls>" || entry.contains("://"))
            .map(|entry| entry.to_string())
            .collect();
        Grants {
            tabs: permissions.contains(&"tabs"),
            all_urls: hosts.iter().any(|host| host == "<all_urls>"),
            hosts,
        }
    }

    /// Whether the extension may be told the address and title of a tab
    /// at `url`.
    pub(crate) fn sees(&self, url: &str) -> bool {
        self.tabs
            || Url::parse(url)
                .is_ok_and(|url| self.hosts.iter().any(|host| matches_url(host, &url)))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use url::Url;

    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn globs_match_runs_of_any_characters() {
        for (pattern, text, expected) in [
            ("*", "", true),
            ("*", "anything", true),
            ("Inbox*", "Inbox (3)", true),
            ("*Domain", "Example Domain", true),
            ("Ex*Dom*n", "Example Domain", true),
            ("Example", "Example Domain", false),
            ("example*", "Example Domain", false),
            ("", "", true),
            ("", "x", false),
            ("a*b*c", "abc", true),
            ("a*b*c", "acb", false),
        ] {
            assert_eq!(glob(pattern, text), expected, "{pattern:?} on {text:?}");
        }
    }

    #[test]
    fn match_patterns_follow_chromes_rules() {
        for (pattern, page, expected) in [
            ("<all_urls>", "https://example.com/a", true),
            ("<all_urls>", "file:///tmp/x.html", true),
            ("<all_urls>", "chrome://extensions/", false),
            ("<all_urls>", "about:blank", false),
            ("*://*/*", "http://example.com/", true),
            ("*://*/*", "https://example.com/x?y", true),
            ("*://*/*", "file:///tmp/x.html", false),
            ("*://*.example.com/*", "https://example.com/", true),
            ("*://*.example.com/*", "https://www.example.com/a", true),
            ("*://*.example.com/*", "https://notexample.com/", false),
            ("https://example.com/*", "http://example.com/", false),
            (
                "https://example.com/a*",
                "https://example.com/abc?d=1",
                true,
            ),
            ("https://example.com/a", "https://example.com/abc", false),
            ("https://example.com/*", "https://example.com:8443/x", true),
            ("https://*/foo?bar=*", "https://x.test/foo?bar=1", true),
            ("file:///tmp/*", "file:///tmp/x.html", true),
            (
                "chrome-extension://*/*",
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop/p.html",
                true,
            ),
            ("not a pattern", "https://example.com/", false),
            ("https://example.com", "https://example.com/", false),
        ] {
            assert_eq!(
                matches_url(pattern, &url(page)),
                expected,
                "{pattern} on {page}"
            );
        }
    }

    #[test]
    fn grants_come_from_the_manifests_permissions() {
        let v3 = Grants::from_manifest(&json!({
            "manifest_version": 3,
            "permissions": ["storage", "tabs", "activeTab"],
            "host_permissions": ["https://*.example.com/*"],
            "optional_host_permissions": ["<all_urls>"]
        }));
        assert_eq!(
            v3,
            Grants {
                tabs: true,
                all_urls: false,
                hosts: vec!["https://*.example.com/*".to_string()]
            }
        );
        let v2 = Grants::from_manifest(&json!({
            "manifest_version": 2,
            "permissions": ["storage", "<all_urls>", "*://*.example.org/*"]
        }));
        assert_eq!(
            v2,
            Grants {
                tabs: false,
                all_urls: true,
                hosts: vec!["<all_urls>".to_string(), "*://*.example.org/*".to_string()]
            }
        );
        assert_eq!(Grants::from_manifest(&json!("nonsense")), Grants::default());
    }

    #[test]
    fn an_extension_sees_a_page_with_the_tabs_permission_or_a_matching_host() {
        let tabs = Grants {
            tabs: true,
            ..Grants::default()
        };
        let host = Grants {
            hosts: vec!["https://*.example.com/*".to_string()],
            ..Grants::default()
        };
        assert!(tabs.sees("https://anything.test/"));
        assert!(tabs.sees("about:blank"));
        assert!(host.sees("https://www.example.com/x"));
        assert!(!host.sees("https://example.org/"));
        assert!(!host.sees("not a url"));
        assert!(!Grants::default().sees("https://example.com/"));
    }
}
