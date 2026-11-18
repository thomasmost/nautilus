//! The manifest rewrite that puts the bridge ahead of an extension's
//! service worker, and the loader it points the manifest at. Pure.
//!
//! The loader sits in the same directory as the original entry file:
//! `importScripts('x.js')` inside the entry resolves against the
//! worker's own URL, so a loader anywhere else would break the
//! extension's relative imports.

use serde_json::Value;

/// The loader's file name, beside the original entry.
pub(crate) const LOADER_NAME: &str = "nautilus-loader.js";

/// The directory in an extension that holds the bridge's files. Not
/// `_nautilus`: Chromium refuses names that start with an underscore.
pub(crate) const BRIDGE_DIR: &str = "nautilus-bridge";

/// The entry the manifest named before the rewrite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Original {
    /// Relative to the extension's root, no leading slash.
    pub service_worker: String,
    /// `background.type` was `"module"`.
    pub module: bool,
}

/// What the rewrite did.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Rewrite {
    /// The manifest now names the loader.
    Wrapped {
        manifest: Value,
        /// Where the loader goes, relative to the extension's root.
        loader_path: String,
        /// The loader's source.
        loader: String,
        original: Original,
    },
    /// The manifest already names a loader: nothing to change.
    Already,
    /// There is no service worker to wrap (Manifest V2, or no
    /// background at all): the extension runs unbridged.
    NoWorker,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RewriteError {
    NotAnObject,
    /// The worker path leaves the extension or is no file path.
    BadWorkerPath(String),
}

impl std::fmt::Display for RewriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RewriteError::NotAnObject => write!(f, "the manifest is not a JSON object"),
            RewriteError::BadWorkerPath(path) => {
                write!(
                    f,
                    "the service worker path {path:?} is not a file in the extension"
                )
            }
        }
    }
}

/// Whether `worker` (a manifest's `background.service_worker`) is a
/// loader this module wrote.
pub(crate) fn is_loader(worker: &str) -> bool {
    worker.rsplit('/').next() == Some(LOADER_NAME)
}

/// Point `manifest`'s service worker at a loader. Every other key is
/// left as it is.
pub(crate) fn rewrite_manifest(manifest: &Value) -> Result<Rewrite, RewriteError> {
    if !manifest.is_object() {
        return Err(RewriteError::NotAnObject);
    }
    let background = manifest.get("background");
    let Some(worker) = background
        .and_then(|b| b.get("service_worker"))
        .and_then(|w| w.as_str())
    else {
        return Ok(Rewrite::NoWorker);
    };
    let worker = normalise(worker)?;
    if is_loader(&worker) {
        return Ok(Rewrite::Already);
    }
    let original = Original {
        module: background
            .and_then(|b| b.get("type"))
            .and_then(|t| t.as_str())
            == Some("module"),
        service_worker: worker,
    };
    let loader_path = match original.service_worker.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/{LOADER_NAME}"),
        None => LOADER_NAME.to_string(),
    };
    let mut manifest = manifest.clone();
    manifest["background"]["service_worker"] = Value::String(loader_path.clone());
    Ok(Rewrite::Wrapped {
        manifest,
        loader: loader_source(&original),
        loader_path,
        original,
    })
}

/// The loader for `original`: the bridge, then the extension's own
/// entry, both during the worker's first evaluation so that each one's
/// top-level listeners count as registered at startup. A classic
/// worker that cannot load the bridge still runs, unbridged; a module
/// worker cannot catch a failed static import.
pub(crate) fn loader_source(original: &Original) -> String {
    let bridge = Value::String(format!("/{BRIDGE_DIR}/bridge.js")).to_string();
    let entry = Value::String(format!("/{}", original.service_worker)).to_string();
    if original.module {
        format!(
            "// Written by Nautilus: the bridge, then the extension's own worker.\n\
             import {bridge};\n\
             import {entry};\n"
        )
    } else {
        format!(
            "// Written by Nautilus: the bridge, then the extension's own worker.\n\
             try {{ importScripts({bridge}); }} catch (e) {{ console.error(\"Nautilus bridge failed to load\", e); }}\n\
             importScripts({entry});\n"
        )
    }
}

/// Whether `part` names a Windows device (`CON`, `PRN`, `AUX`, `NUL`,
/// `COM1`-`COM9`, `LPT1`-`LPT9`), with or without an extension, in any
/// case. Windows ignores trailing spaces and dots in a name, so `nul .js`
/// and `CON.` are the same devices. Checked on every platform: the rule
/// is about the manifest, which may come from any host.
fn is_windows_device(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("");
    let stem = stem.trim_end_matches(' ').to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']))
    };
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT")
}

/// A manifest path as a relative one with forward slashes and no
/// leading `./` or `/`. Refused: `..`, `\`, `?`, `#`, a `:` anywhere (a
/// drive letter or an alternate data stream on Windows), and a Windows
/// device name in any component.
fn normalise(path: &str) -> Result<String, RewriteError> {
    let bad = || RewriteError::BadWorkerPath(path.to_string());
    let mut rest = path.trim();
    while let Some(stripped) = rest.strip_prefix("./") {
        rest = stripped;
    }
    let rest = rest.trim_start_matches('/');
    if rest.is_empty()
        || rest.ends_with('/')
        || rest.contains('\\')
        || rest.contains([':', '?', '#'])
        || rest
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || is_windows_device(part))
    {
        return Err(bad());
    }
    Ok(rest.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_root_worker_is_wrapped_by_a_loader_beside_it_and_nothing_else_changes() {
        let manifest = json!({
            "manifest_version": 3,
            "name": "x",
            "permissions": ["tabs"],
            "background": {"service_worker": "sw.js"},
            "action": {"default_title": "t"}
        });
        let Rewrite::Wrapped {
            manifest: out,
            loader_path,
            loader,
            original,
        } = rewrite_manifest(&manifest).unwrap()
        else {
            panic!("not wrapped");
        };
        assert_eq!(loader_path, "nautilus-loader.js");
        assert_eq!(
            original,
            Original {
                service_worker: "sw.js".to_string(),
                module: false
            }
        );
        let mut expected = manifest.clone();
        expected["background"]["service_worker"] = json!("nautilus-loader.js");
        assert_eq!(out, expected);
        assert_eq!(
            loader,
            "// Written by Nautilus: the bridge, then the extension's own worker.\n\
             try { importScripts(\"/nautilus-bridge/bridge.js\"); } catch (e) { console.error(\"Nautilus bridge failed to load\", e); }\n\
             importScripts(\"/sw.js\");\n"
        );
    }

    #[test]
    fn a_worker_in_a_directory_gets_its_loader_in_that_directory() {
        for written in [
            "background/main.js",
            "/background/main.js",
            "./background/main.js",
        ] {
            let manifest = json!({"background": {"service_worker": written}});
            let Rewrite::Wrapped {
                loader_path,
                loader,
                original,
                ..
            } = rewrite_manifest(&manifest).unwrap()
            else {
                panic!("not wrapped: {written}");
            };
            assert_eq!(loader_path, "background/nautilus-loader.js", "{written}");
            assert_eq!(original.service_worker, "background/main.js", "{written}");
            assert!(
                loader.contains("importScripts(\"/background/main.js\");"),
                "{loader}"
            );
        }
    }

    #[test]
    fn a_module_worker_gets_static_imports_and_stays_a_module() {
        let manifest = json!({"background": {"service_worker": "sw.js", "type": "module"}});
        let Rewrite::Wrapped {
            manifest: out,
            loader,
            original,
            ..
        } = rewrite_manifest(&manifest).unwrap()
        else {
            panic!("not wrapped");
        };
        assert!(original.module);
        assert_eq!(out["background"]["type"], json!("module"));
        assert_eq!(
            loader,
            "// Written by Nautilus: the bridge, then the extension's own worker.\n\
             import \"/nautilus-bridge/bridge.js\";\n\
             import \"/sw.js\";\n"
        );
        assert!(!loader.contains("importScripts"));
    }

    #[test]
    fn a_rewritten_manifest_is_left_alone() {
        let manifest = json!({"background": {"service_worker": "sw.js"}});
        let Rewrite::Wrapped { manifest: once, .. } = rewrite_manifest(&manifest).unwrap() else {
            panic!("not wrapped");
        };
        assert_eq!(rewrite_manifest(&once), Ok(Rewrite::Already));
        let nested = json!({"background": {"service_worker": "bg/nautilus-loader.js"}});
        assert_eq!(rewrite_manifest(&nested), Ok(Rewrite::Already));
        assert!(is_loader("nautilus-loader.js"));
        assert!(!is_loader("my-nautilus-loader.js"));
    }

    #[test]
    fn a_manifest_without_a_worker_is_not_bridged() {
        for manifest in [
            json!({"name": "x"}),
            json!({"background": {"scripts": ["a.js"]}}),
            json!({"background": {"page": "bg.html"}}),
            json!({"background": {"service_worker": 3}}),
        ] {
            assert_eq!(
                rewrite_manifest(&manifest),
                Ok(Rewrite::NoWorker),
                "{manifest}"
            );
        }
    }

    #[test]
    fn a_worker_path_that_leaves_the_extension_is_refused() {
        assert_eq!(rewrite_manifest(&json!([])), Err(RewriteError::NotAnObject));
        for path in [
            "../sw.js",
            "a/../../sw.js",
            "",
            "/",
            "dir/",
            "a\\b.js",
            "sw.js?x",
            "a//b.js",
            // Windows: a drive or a stream name in a component.
            "C:/Users/x/nautilus-loader.js",
            "C:sw.js",
            "sw.js:stream",
            "background/a:b.js",
            // Windows: device names, with or without an extension, in any case.
            "CON",
            "con.js",
            "background/NUL.txt",
            "Aux/sw.js",
            "COM1.js",
            "lpt9",
            "nul .js",
        ] {
            let manifest = json!({"background": {"service_worker": path}});
            assert_eq!(
                rewrite_manifest(&manifest),
                Err(RewriteError::BadWorkerPath(path.to_string())),
                "{path}"
            );
        }
    }

    #[test]
    fn names_that_only_look_like_device_names_are_allowed() {
        for path in [
            "com10.js",
            "console.js",
            "lpt0.js",
            "nulx.js",
            "background/aux2.js",
        ] {
            let manifest = json!({"background": {"service_worker": path}});
            assert!(
                matches!(rewrite_manifest(&manifest), Ok(Rewrite::Wrapped { .. })),
                "{path}"
            );
        }
    }

    #[test]
    fn a_path_with_a_quote_cannot_break_out_of_the_loader() {
        let manifest = json!({"background": {"service_worker": "a\"b.js"}});
        let Rewrite::Wrapped { loader, .. } = rewrite_manifest(&manifest).unwrap() else {
            panic!("not wrapped");
        };
        assert!(loader.contains(r#"importScripts("/a\"b.js");"#), "{loader}");
    }
}
