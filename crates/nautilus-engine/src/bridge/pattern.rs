//! Match patterns, title globs, and what an extension's manifest lets it see.

use serde_json::Value;
use url::Url;

/// Whether a manifest entry is a host pattern: `<all_urls>`, or a match
/// pattern with a scheme.
fn is_host(entry: &str) -> bool {
    entry == "<all_urls>" || entry.contains("://")
}

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

/// The schemes `<all_urls>` covers for a tab's address, besides `file`
/// (`matches_url`).
const ALL_URLS_SCHEMES: [&str; 5] = ["http", "https", "ftp", "ws", "wss"];

/// Whether `url` falls under the match pattern `pattern`
/// (`<scheme>://<host><path>`, or `<all_urls>`). A scheme of `*` is `http`
/// or `https`; a host of `*` is any, `*.x` is `x` and its subdomains; a
/// port in the pattern is not compared; the path is a glob over the URL's
/// path and query; the scheme and the host compare case-insensitively. Anything
/// that is no pattern matches nothing. A file page is named by a pattern that
/// names the `file` scheme, and by `<all_urls>`; this is the rule of the
/// `tabs.query` url filter, which lists the file pages the extension may see.
/// Whether the extension may see a file page at all is `grants_url`'s rule.
pub(crate) fn matches_url(pattern: &str, url: &Url) -> bool {
    if pattern == "<all_urls>" {
        return ALL_URLS_SCHEMES.contains(&url.scheme()) || url.scheme() == "file";
    }
    let Some((scheme, rest)) = pattern.split_once("://") else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    let scheme_matches = match scheme.as_str() {
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
    let host = host
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let have = url.host_str().unwrap_or_default();
    let host_matches = if host == "*" {
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

/// Whether a host permission `pattern` grants the page at `url`: the pattern
/// matches, and the page is not a file. This browser grants no file access,
/// and Chrome's file access is a switch of the extension that the bridge
/// cannot read, so a host pattern never covers a file page.
pub(crate) fn grants_url(pattern: &str, url: &Url) -> bool {
    url.scheme() != "file" && matches_url(pattern, url)
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
    /// Read from a parsed `manifest.json`: `permissions` and
    /// `host_permissions`. A host pattern counts under `permissions` only in
    /// Manifest V2, where Chrome reads it there; in Manifest V3 Chrome reads
    /// host patterns from `host_permissions` alone and ignores the others.
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
        let manifest_v3 = manifest.get("manifest_version").and_then(Value::as_u64) == Some(3);
        let permissions = list("permissions");
        let hosts: Vec<String> = permissions
            .iter()
            .filter(|entry| !manifest_v3 && is_host(entry))
            .chain(
                list("host_permissions")
                    .iter()
                    .filter(|entry| is_host(entry)),
            )
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
            || Url::parse(url).is_ok_and(|url| self.hosts.iter().any(|host| grants_url(host, &url)))
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

    /// The `tabs.query` url filter (`matches_url`): a pattern names the
    /// pages it matches, file pages included, whatever the extension may read.
    #[test]
    fn query_patterns_follow_chromes_rules() {
        for (pattern, page, expected) in [
            ("<all_urls>", "https://example.com/a", true),
            ("<all_urls>", "file:///tmp/x.html", true),
            ("<all_urls>", "chrome://extensions/", false),
            ("<all_urls>", "about:blank", false),
            ("*://*/*", "http://example.com/", true),
            ("*://*/*", "https://example.com/x?y", true),
            // A scheme of `*` is http or https, never file.
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
            ("file:///tmp/*", "file:///etc/hosts", false),
            // Chrome compares hosts case-insensitively.
            ("https://EXAMPLE.com/*", "https://example.com/", true),
            ("*://*.Example.COM/*", "https://www.example.com/a", true),
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
                "query {pattern} on {page}"
            );
        }
    }

    /// The host permissions (`grants_url`): a host pattern never grants a
    /// file page, whatever its scheme, and every other page matches as the
    /// query rule has it.
    #[test]
    fn host_grants_never_cover_a_file_page() {
        for (pattern, page, expected) in [
            ("<all_urls>", "https://example.com/a", true),
            ("<all_urls>", "file:///tmp/x.html", false),
            ("*://*/*", "http://example.com/", true),
            ("*://*/*", "file:///tmp/x.html", false),
            ("file:///tmp/*", "file:///tmp/x.html", false),
            ("file:///*", "file:///tmp/x.html", false),
            ("https://example.com/*", "https://example.com/", true),
            ("chrome-extension://*/*", "file:///tmp/x.html", false),
        ] {
            assert_eq!(
                grants_url(pattern, &url(page)),
                expected,
                "grant {pattern} on {page}"
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
    fn manifest_version_3_takes_host_patterns_only_from_host_permissions() {
        let v3 = Grants::from_manifest(&json!({
            "manifest_version": 3,
            "permissions": ["tabs", "https://a.example/*", "<all_urls>"],
            "host_permissions": ["https://b.example/*"]
        }));
        assert_eq!(
            v3,
            Grants {
                tabs: true,
                all_urls: false,
                hosts: vec!["https://b.example/*".to_string()]
            }
        );
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
        // A file page is seen with the tabs permission and never with a host pattern.
        assert!(tabs.sees("file:///tmp/x.html"));
        let all = Grants {
            hosts: vec!["<all_urls>".to_string()],
            ..Grants::default()
        };
        assert!(all.sees("https://example.com/"));
        assert!(!all.sees("file:///tmp/x.html"));
    }
}
