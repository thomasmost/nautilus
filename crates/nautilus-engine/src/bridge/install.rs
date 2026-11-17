//! Writing the bridge into an extension's directory, and the helper
//! extension into the profile. File work only; no CEF.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::rewrite::{BRIDGE_DIR, Rewrite, rewrite_manifest};

const BRIDGE_JS: &str = include_str!("js/bridge.js");
const RELAY_HTML: &str = include_str!("js/relay.html");
const RELAY_JS: &str = include_str!("js/relay.js");
const HELPER_MANIFEST: &str = include_str!("js/helper/manifest.json");
const HELPER_HTML: &str = include_str!("js/helper/helper.html");
const HELPER_JS: &str = include_str!("js/helper/helper.js");

/// The helper extension's id, which its manifest's `key` fixes.
pub(crate) const HELPER_ID: &str = "gblknehfeeaadaalhiogpllhfpelpngj";

/// The helper's directory under the profile's `chromium/extensions/`.
const HELPER_DIR: &str = "nautilus-helper";

/// What became of an extension's directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bridged {
    /// The loader and the bridge's files are in place.
    Yes,
    /// The extension has no service worker; it is loaded as it is.
    NoWorker,
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// Write the helper extension under `store_dir` (the profile's
/// `chromium/extensions`) and return its directory. Written at every
/// start, so a newer build's helper replaces an older one's.
pub(crate) fn write_helper(store_dir: &Path) -> Result<PathBuf, String> {
    let dir = store_dir.join(HELPER_DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    write(&dir.join("manifest.json"), HELPER_MANIFEST)?;
    write(&dir.join("helper.html"), HELPER_HTML)?;
    write(&dir.join("helper.js"), HELPER_JS)?;
    Ok(dir)
}

/// Put the bridge into the extension in `dir`: its files under
/// `nautilus-bridge/`, and, the first time, the loader beside the
/// worker's entry, the manifest pointing at it, and a record of the
/// original entry in `nautilus-bridge/original.json`. Removing the
/// loader and that directory and restoring the entry undoes it.
pub(crate) fn rewrite_in_place(dir: &Path) -> Result<Bridged, String> {
    let manifest_path = dir.join("manifest.json");
    let text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("the extension has no manifest: {e}"))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("the manifest is not JSON: {e}"))?;
    let rewrite = rewrite_manifest(&manifest).map_err(|e| e.to_string())?;
    if rewrite == Rewrite::NoWorker {
        return Ok(Bridged::NoWorker);
    }
    let bridge_dir = dir.join(BRIDGE_DIR);
    fs::create_dir_all(&bridge_dir)
        .map_err(|e| format!("creating {}: {e}", bridge_dir.display()))?;
    write(&bridge_dir.join("bridge.js"), BRIDGE_JS)?;
    write(&bridge_dir.join("relay.html"), RELAY_HTML)?;
    write(&bridge_dir.join("relay.js"), RELAY_JS)?;
    if let Rewrite::Wrapped {
        manifest,
        loader_path,
        loader,
        original,
    } = rewrite
    {
        write(&dir.join(&loader_path), &loader)?;
        let record = serde_json::json!({
            "service_worker": original.service_worker,
            "module": original.module,
        });
        write(&bridge_dir.join("original.json"), &record.to_string())?;
        let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        write(&manifest_path, &text)?;
    }
    Ok(Bridged::Yes)
}

/// The directory name of the profile's copy of the unpacked extension
/// at `source` (an absolute path): `unpacked-<name>-<hash>`, the
/// folder's name reduced to lower-case letters, digits and dashes, and
/// eight hex digits of the SHA-256 of its path. The same source always
/// lands in the same copy, and two folders of one name do not collide.
/// Phases 5 to 7 keep this scheme; nothing else names a copy.
pub(crate) fn copy_name(source: &Path) -> String {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(32)
        .collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "extension" } else { slug };
    let hash: String = Sha256::digest(source.to_string_lossy().as_bytes())
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("unpacked-{slug}-{hash}")
}

/// Copy the unpacked extension at `source` into `store_dir`, replacing
/// an earlier copy of it, and put the bridge into the copy. The
/// source is only read.
pub(crate) fn copy_and_rewrite(
    source: &Path,
    store_dir: &Path,
) -> Result<(PathBuf, Bridged), String> {
    if !source.join("manifest.json").is_file() {
        return Err(format!("{} has no manifest.json", source.display()));
    }
    // The name hashes the absolute path, so a relative and an absolute
    // spelling of one folder land in one copy.
    let source =
        &std::path::absolute(source).map_err(|e| format!("resolving {}: {e}", source.display()))?;
    let copy = store_dir.join(copy_name(source));
    if copy.exists() {
        fs::remove_dir_all(&copy).map_err(|e| format!("clearing {}: {e}", copy.display()))?;
    }
    copy_dir(source, &copy)?;
    let bridged = rewrite_in_place(&copy)?;
    Ok((copy, bridged))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("creating {}: {e}", to.display()))?;
    let entries = fs::read_dir(from).map_err(|e| format!("reading {}: {e}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("reading {}: {e}", from.display()))?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        // `metadata` follows a symbolic link, so a linked file or
        // directory is copied as what it points at.
        let kind =
            fs::metadata(&source).map_err(|e| format!("reading {}: {e}", source.display()))?;
        if kind.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            fs::copy(&source, &target).map_err(|e| format!("copying {}: {e}", source.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::rewrite::LOADER_NAME;

    /// An empty directory of this test's own under the system's
    /// temporary directory.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nautilus-bridge-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn extension(root: &Path, manifest: &str) -> PathBuf {
        let dir = root.join("source");
        fs::create_dir_all(dir.join("background")).unwrap();
        fs::write(dir.join("manifest.json"), manifest).unwrap();
        fs::write(dir.join("background/sw.js"), "importScripts('lib.js');").unwrap();
        fs::write(dir.join("background/lib.js"), "// lib").unwrap();
        dir
    }

    fn standard_base64(text: &str) -> Vec<u8> {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let (mut bits, mut count) = (0u32, 0);
        for byte in text.bytes().filter(|b| *b != b'=') {
            let value = ALPHABET.iter().position(|a| *a == byte).unwrap() as u32;
            bits = (bits << 6) | value;
            count += 6;
            if count >= 8 {
                count -= 8;
                out.push((bits >> count) as u8);
                bits &= (1 << count) - 1;
            }
        }
        out
    }

    #[test]
    fn the_helpers_id_is_the_one_its_key_gives() {
        let manifest: serde_json::Value = serde_json::from_str(HELPER_MANIFEST).unwrap();
        let key = standard_base64(manifest["key"].as_str().unwrap());
        assert_eq!(crate::crx::id_for_key(&key), HELPER_ID);
        assert_eq!(manifest["permissions"], serde_json::json!(["debugger"]));
        assert!(HELPER_JS.contains("chrome.debugger.getTargets"));
    }

    #[test]
    fn the_helper_is_written_whole_and_again() {
        let root = scratch("helper");
        let dir = write_helper(&root).unwrap();
        assert_eq!(dir, root.join("nautilus-helper"));
        for file in ["manifest.json", "helper.html", "helper.js"] {
            assert!(dir.join(file).is_file(), "{file}");
        }
        assert_eq!(write_helper(&root).unwrap(), dir);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_copys_name_is_the_folders_name_and_a_hash_of_its_path() {
        let name = copy_name(Path::new("/Users/someone/dev/My Extension (v2)"));
        let (slug, hash) = name
            .strip_prefix("unpacked-")
            .and_then(|rest| rest.rsplit_once('-'))
            .unwrap();
        // Lower-cased, the space and both brackets each a dash, the
        // trailing dash trimmed.
        assert_eq!(slug, "my-extension--v2");
        assert_eq!(hash.len(), 8);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            name,
            copy_name(Path::new("/Users/someone/dev/My Extension (v2)"))
        );
        assert_ne!(
            name,
            copy_name(Path::new("/Users/other/dev/My Extension (v2)"))
        );
        assert!(copy_name(Path::new("/")).starts_with("unpacked-extension-"));
        assert!(!name.starts_with('_'));
    }

    #[test]
    fn a_copy_is_bridged_and_the_source_is_left_alone() {
        let root = scratch("copy");
        let manifest = r#"{"manifest_version":3,"name":"t","background":{"service_worker":"background/sw.js"}}"#;
        let source = extension(&root, manifest);
        let store = root.join("store");
        let (copy, bridged) = copy_and_rewrite(&source, &store).unwrap();
        assert_eq!(bridged, Bridged::Yes);
        assert_eq!(copy, store.join(copy_name(&source)));
        // The source: untouched.
        assert_eq!(
            fs::read_to_string(source.join("manifest.json")).unwrap(),
            manifest
        );
        assert!(!source.join("nautilus-bridge").exists());
        assert!(!source.join("background").join(LOADER_NAME).exists());
        // The copy: the loader beside the entry, the manifest naming it,
        // the bridge's files, the record of the original entry.
        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(copy.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(
            written["background"]["service_worker"],
            "background/nautilus-loader.js"
        );
        let loader = fs::read_to_string(copy.join("background").join(LOADER_NAME)).unwrap();
        assert!(
            loader.contains("importScripts(\"/background/sw.js\");"),
            "{loader}"
        );
        assert!(copy.join("background/sw.js").is_file());
        assert!(copy.join("background/lib.js").is_file());
        for file in ["bridge.js", "relay.html", "relay.js", "original.json"] {
            assert!(copy.join("nautilus-bridge").join(file).is_file(), "{file}");
        }
        let record: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(copy.join("nautilus-bridge/original.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            record,
            serde_json::json!({"service_worker": "background/sw.js", "module": false})
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rewriting_twice_changes_nothing_more_and_keeps_the_record() {
        let root = scratch("twice");
        let source = extension(
            &root,
            r#"{"background":{"service_worker":"background/sw.js"}}"#,
        );
        let (copy, _) = copy_and_rewrite(&source, &root.join("store")).unwrap();
        let manifest = fs::read_to_string(copy.join("manifest.json")).unwrap();
        let record = fs::read_to_string(copy.join("nautilus-bridge/original.json")).unwrap();
        fs::write(copy.join("nautilus-bridge/bridge.js"), "// an older bridge").unwrap();
        assert_eq!(rewrite_in_place(&copy), Ok(Bridged::Yes));
        assert_eq!(
            fs::read_to_string(copy.join("manifest.json")).unwrap(),
            manifest
        );
        assert_eq!(
            fs::read_to_string(copy.join("nautilus-bridge/original.json")).unwrap(),
            record
        );
        // The bridge's own files are refreshed.
        assert_eq!(
            fs::read_to_string(copy.join("nautilus-bridge/bridge.js")).unwrap(),
            BRIDGE_JS
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn loading_the_same_source_again_replaces_its_copy() {
        let root = scratch("again");
        let source = extension(
            &root,
            r#"{"background":{"service_worker":"background/sw.js"}}"#,
        );
        let store = root.join("store");
        let (first, _) = copy_and_rewrite(&source, &store).unwrap();
        fs::write(first.join("stale.txt"), "x").unwrap();
        let (second, _) = copy_and_rewrite(&source, &store).unwrap();
        assert_eq!(first, second);
        assert!(!second.join("stale.txt").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_extension_without_a_worker_is_copied_and_left_unbridged() {
        let root = scratch("noworker");
        let source = extension(&root, r#"{"name":"plain"}"#);
        let (copy, bridged) = copy_and_rewrite(&source, &root.join("store")).unwrap();
        assert_eq!(bridged, Bridged::NoWorker);
        assert!(!copy.join("nautilus-bridge").exists());
        assert!(copy_and_rewrite(&root.join("missing"), &root.join("store")).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_scripts_agree_on_the_port_and_the_page_names() {
        assert!(BRIDGE_JS.contains("'nautilus-bridge-1'"));
        assert!(RELAY_JS.contains("'nautilus-bridge-1'"));
        assert!(BRIDGE_JS.contains("self.__nautilusTransport"));
        assert!(RELAY_JS.contains("window.nautilusRelay"));
        assert!(RELAY_HTML.contains("relay.js"));
        assert!(HELPER_HTML.contains("helper.js"));
    }
}
