//! Writing the bridge into an extension's directory, and the helper
//! extension into the profile: the rewrite on disk, the marker that
//! makes it happen once per version of the script, and the copy of a
//! folder the user loads (the user's folder is never written to; dot-files
//! are left out of the copy). File work only; no CEF.

use std::fs;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::rewrite::{BRIDGE_DIR, Rewrite, rewrite_manifest};

pub(crate) const BRIDGE_JS: &str = include_str!("js/bridge.js");
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
pub(crate) fn rewrite_in_place(dir: &Path, skipped: &[PathBuf]) -> Result<Bridged, String> {
    let manifest_path = dir.join("manifest.json");
    let text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("the extension has no manifest: {e}"))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("the manifest is not JSON: {e}"))?;
    let rewrite = rewrite_manifest(&manifest).map_err(|e| e.to_string())?;
    if rewrite == Rewrite::NoWorker {
        return Ok(Bridged::NoWorker);
    }
    // The loader imports the worker's entry: without it the worker cannot
    // run, so the load fails here rather than leaving a broken copy. A
    // manifest that already names the loader (a bridged copy) is checked
    // against the entry the first rewrite recorded.
    let entry = match &rewrite {
        Rewrite::Wrapped { original, .. } => Some(original.service_worker.clone()),
        _ => recorded_entry(dir),
    };
    if let Some(entry) = entry {
        entry_in_copy(&entry, dir.join(&entry).is_file(), skipped)?;
    }
    let bridge_dir = dir.join(BRIDGE_DIR);
    fs::create_dir_all(&bridge_dir)
        .map_err(|e| format!("creating {}: {e}", bridge_dir.display()))?;
    write(
        &bridge_dir.join("bridge.js"),
        &crate::bridge::worker_script(),
    )?;
    write(&bridge_dir.join("relay.html"), RELAY_HTML)?;
    write(&bridge_dir.join("relay.js"), RELAY_JS)?;
    if let Rewrite::Wrapped {
        manifest,
        loader_path,
        loader,
        original,
    } = rewrite
    {
        write(&join_under(dir, &loader_path)?, &loader)?;
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

/// The file beside the manifest that records which version of the bridge
/// script the directory holds. Its name does not start with `_`:
/// Chromium refuses to load a directory holding such a file.
pub(crate) const MARKER: &str = "nautilus-bridge.json";

/// The version a bridge script is recorded under: 16 hex digits of its
/// SHA-256, so any change to the script re-applies it everywhere at the
/// next start.
pub(crate) fn version_of(script: &str) -> String {
    Sha256::digest(script.as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Make sure the extension in `dir` carries the bridge at `version`,
/// calling `apply` (the rewrite: [`rewrite_in_place`]) only when the
/// marker does not already say so. Returns whether anything was written.
/// A failed `apply` writes no marker, so the next call tries again.
pub(crate) fn ensure_bridged(
    dir: &Path,
    version: &str,
    apply: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<bool, String> {
    if !dir.join("manifest.json").is_file() {
        return Err(format!("{} holds no manifest.json", dir.display()));
    }
    let recorded = fs::read_to_string(dir.join(MARKER))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| {
            value
                .get("version")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    if recorded.as_deref() == Some(version) {
        return Ok(false);
    }
    apply(dir)?;
    write_marker(dir, version)?;
    Ok(true)
}

fn write_marker(dir: &Path, version: &str) -> Result<(), String> {
    let marker = dir.join(MARKER);
    write(
        &marker,
        &serde_json::json!({ "version": version }).to_string(),
    )
}

/// The directory name of the profile's copy of the unpacked extension
/// at `source` (an absolute path): `unpacked-<name>-<hash>`, the
/// folder's name reduced to lower-case letters, digits and dashes, and
/// eight hex digits of the SHA-256 of its path. The same source always
/// lands in the same copy, and two folders of one name do not collide.
/// On Windows the path is case-folded first (`path_key_text`), so two
/// spellings of one folder share a copy. Phases 5 to 7 keep this scheme;
/// nothing else names a copy.
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
    let key = path_key_text(&source.to_string_lossy(), cfg!(windows));
    let hash: String = Sha256::digest(key.as_bytes())
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("unpacked-{slug}-{hash}")
}

/// Bring the extension at `path` into the profile's `store_dir` at
/// `version`, and return the directory it is loaded from and what became
/// of it. A marked copy in the store is rewritten in place: it is not
/// copied again, so a newer build's bridge reaches it. Anything else is a
/// source folder: it is copied into the store, bridged, and moved into
/// place (`copy_and_bridge`). The source is only read.
pub(crate) fn bridge_source(
    path: &Path,
    store_dir: &Path,
    version: &str,
) -> Result<(PathBuf, Bridged), String> {
    let path =
        std::path::absolute(path).map_err(|e| format!("resolving {}: {e}", path.display()))?;
    let store = std::path::absolute(store_dir)
        .map_err(|e| format!("resolving {}: {e}", store_dir.display()))?;
    if path != store && path.starts_with(&store) && path.join(MARKER).is_file() {
        let bridged = rewrite_in_place(&path, &[])?;
        write_marker(&path, version)?;
        return Ok((path, bridged));
    }
    if !path.join("manifest.json").is_file() {
        return Err(format!("{} holds no manifest.json", path.display()));
    }
    fs::create_dir_all(&store).map_err(|e| format!("creating {}: {e}", store.display()))?;
    refuse_nested(&path, &store)?;
    copy_and_bridge(&path, &store, version)
}

/// Copy the source at `source` into `store_dir` and bridge the copy at
/// `version`, replacing an earlier copy of the same folder. The copy is
/// built beside its place and moved there only once the bridge is in, so a
/// failure leaves the store as it was: no partial copy, and the previous
/// copy, if any, intact. The name hashes the absolute path, so a relative
/// and an absolute spelling of one folder land in one copy.
fn copy_and_bridge(
    source: &Path,
    store_dir: &Path,
    version: &str,
) -> Result<(PathBuf, Bridged), String> {
    let copy = store_dir.join(copy_name(source));
    let staging = store_dir.join(format!("{}.partial", copy_name(source)));
    let _ = fs::remove_dir_all(&staging);
    match stage(source, &staging, &copy, version) {
        Ok(bridged) => Ok((copy, bridged)),
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            Err(e)
        }
    }
}

/// Copy `source` into `staging`, bridge the copy under the marker, and
/// move it to `copy`, replacing an earlier copy.
fn stage(source: &Path, staging: &Path, copy: &Path, version: &str) -> Result<Bridged, String> {
    let (_, skipped) = copy_unpacked(source, staging)?;
    let mut bridged = Bridged::Yes;
    ensure_bridged(staging, version, |at| {
        bridged = rewrite_in_place(at, &skipped)?;
        Ok(())
    })?;
    if copy.exists() {
        fs::remove_dir_all(copy).map_err(|e| format!("clearing {}: {e}", copy.display()))?;
    }
    fs::rename(staging, copy)
        .map_err(|e| format!("moving the copy into {}: {e}", copy.display()))?;
    Ok(bridged)
}

/// Copy the source at `source` into `dest`, replacing what `dest` held,
/// leaving out every dot-file and dot-directory at any depth (`.git`,
/// `.DS_Store`, a `.cache` folder: nothing a browser loads from them). Links
/// are not followed: each one is skipped, logged, and returned by its place
/// in the extension, so the load can tell whether the worker's entry was
/// one. Returns `dest` and that list. `dest` must not be inside `source`.
pub(crate) fn copy_unpacked(source: &Path, dest: &Path) -> Result<(PathBuf, Vec<PathBuf>), String> {
    if !source.join("manifest.json").is_file() {
        return Err(format!("{} holds no manifest.json", source.display()));
    }
    let (absolute_source, absolute_dest) = (
        std::path::absolute(source).map_err(|e| format!("resolving {}: {e}", source.display()))?,
        std::path::absolute(dest).map_err(|e| format!("resolving {}: {e}", dest.display()))?,
    );
    if absolute_dest.starts_with(&absolute_source) {
        return Err(format!(
            "{} is inside {}; refusing to copy a folder into itself",
            dest.display(),
            source.display()
        ));
    }
    if dest.exists() {
        fs::remove_dir_all(dest).map_err(|e| format!("clearing {}: {e}", dest.display()))?;
    }
    let mut skipped = Vec::new();
    copy_dir(source, dest, Path::new(""), &mut skipped)?;
    Ok((dest.to_path_buf(), skipped))
}

/// The worker's entry the first rewrite recorded in a bridged copy, if
/// there is a record.
fn recorded_entry(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join(BRIDGE_DIR).join("original.json")).ok()?;
    let record: serde_json::Value = serde_json::from_str(&text).ok()?;
    record.get("service_worker")?.as_str().map(str::to_string)
}

/// Refuse a source that is the store, inside it, or holds it: copying
/// either into the other would recurse into its own copy.
fn refuse_nested(source: &Path, store: &Path) -> Result<(), String> {
    let canon =
        |p: &Path| fs::canonicalize(p).map_err(|e| format!("resolving {}: {e}", p.display()));
    let (source, store) = (canon(source)?, canon(store)?);
    if source.starts_with(&store) || store.starts_with(&source) {
        return Err(format!(
            "{} and the extension store {} are nested; refusing to copy",
            source.display(),
            store.display()
        ));
    }
    Ok(())
}

/// Whether the copy holds the worker's entry `entry` (relative to the
/// extension). `is_file` says it is a file in the copy; `skipped` are the
/// links the copy left out. A copy without its entry cannot run the worker,
/// and the reason says which of the two it was.
pub(crate) fn entry_in_copy(entry: &str, is_file: bool, skipped: &[PathBuf]) -> Result<(), String> {
    if is_file {
        return Ok(());
    }
    if skipped.iter().any(|link| link == Path::new(entry)) {
        return Err(format!(
            "the worker's entry {entry} is a symbolic link, and links are not copied: the extension cannot load its worker"
        ));
    }
    if Path::new(entry).components().any(
        |part| matches!(part, Component::Normal(name) if name.to_string_lossy().starts_with('.')),
    ) {
        return Err(format!(
            "the worker's entry {entry} is under a dot-file or dot-directory, which is not copied: the extension cannot load its worker"
        ));
    }
    Err(format!(
        "the worker's entry {entry} is missing from the extension: the extension cannot load its worker"
    ))
}

/// `relative` joined onto `dir`, if it names a file inside `dir`: no
/// root, no `..`, no drive or stream (a `:` in any component).
fn join_under(dir: &Path, relative: &str) -> Result<PathBuf, String> {
    let rel = Path::new(relative);
    let plain = !relative.is_empty()
        && !relative.contains(':')
        && rel.components().all(|c| matches!(c, Component::Normal(_)));
    let joined = dir.join(rel);
    if plain && joined.starts_with(dir) {
        Ok(joined)
    } else {
        Err(format!("{relative:?} is not a file inside the extension"))
    }
}

/// The text a path is hashed as. On Windows it is folded to lower case
/// with backslashes, so one folder typed two ways names one copy; elsewhere
/// the path is hashed as it is, so existing copies keep their names.
fn path_key_text(path: &str, windows: bool) -> String {
    if windows {
        path.to_lowercase().replace('/', "\\")
    } else {
        path.to_string()
    }
}

/// Copy the tree at `from` into `to`. `prefix` is `from`'s place in the
/// extension (empty at the top), and each link left out is pushed onto
/// `skipped` by its place, so the load can see whether the worker's entry
/// was one of them.
fn copy_dir(
    from: &Path,
    to: &Path,
    prefix: &Path,
    skipped: &mut Vec<PathBuf>,
) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("creating {}: {e}", to.display()))?;
    let entries = fs::read_dir(from).map_err(|e| format!("reading {}: {e}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("reading {}: {e}", from.display()))?;
        let name = entry.file_name();
        // Dot-files and dot-directories are not part of the copy.
        if name.to_string_lossy().starts_with('.') {
            log::debug!(
                "bridge: {} is not copied into the extension's copy",
                entry.path().display()
            );
            continue;
        }
        let (source, target) = (entry.path(), to.join(&name));
        let place = prefix.join(&name);
        // `symlink_metadata` does not follow a link. A link is skipped:
        // following one can loop back into the tree or leave it, and a
        // dangling one has nothing to copy. The skip is logged at warning
        // level, since the extension may depend on the file.
        let kind = fs::symlink_metadata(&source)
            .map_err(|e| format!("reading {}: {e}", source.display()))?;
        if kind.is_symlink() {
            log::warn!(
                "bridge: the symbolic link {} is not copied into the extension's copy",
                source.display()
            );
            skipped.push(place);
        } else if kind.is_dir() {
            copy_dir(&source, &target, &place, skipped)?;
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
    use std::cell::Cell;

    /// The version the spike's tests bridge at (any text will do).
    const VERSION: &str = "test-version";

    /// The spike's `copy_and_rewrite`, now the entry point `bridge_source`
    /// at a fixed version, so the spike's tests keep their assertions.
    fn copy_and_rewrite(source: &Path, store: &Path) -> Result<(PathBuf, Bridged), String> {
        bridge_source(source, store, VERSION)
    }

    fn put(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    const MANIFEST: &str = r#"{"manifest_version": 3, "name": "T", "version": "1", "background": {"service_worker": "sw.js"}}"#;

    #[test]
    fn the_bridge_is_put_in_once_per_version() {
        let dir = tempfile::tempdir().unwrap();
        put(&dir.path().join("manifest.json"), MANIFEST);
        let applied = Cell::new(0);
        let apply = |at: &Path| {
            assert_eq!(at, dir.path());
            applied.set(applied.get() + 1);
            Ok(())
        };
        assert_eq!(ensure_bridged(dir.path(), "v1", apply), Ok(true));
        assert_eq!(ensure_bridged(dir.path(), "v1", apply), Ok(false));
        assert_eq!(applied.get(), 1);
        assert_eq!(
            fs::read_to_string(dir.path().join(MARKER)).unwrap(),
            r#"{"version":"v1"}"#
        );
        // Another script version: once more.
        assert_eq!(ensure_bridged(dir.path(), "v2", apply), Ok(true));
        assert_eq!(applied.get(), 2);
        // A marker that is not ours to read counts as none.
        put(&dir.path().join(MARKER), "not json");
        assert_eq!(ensure_bridged(dir.path(), "v2", apply), Ok(true));
        assert_eq!(applied.get(), 3);
    }

    #[test]
    fn a_failed_rewrite_leaves_no_marker_and_no_manifest_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        put(&dir.path().join("manifest.json"), MANIFEST);
        assert_eq!(
            ensure_bridged(dir.path(), "v1", |_| Err(
                "the manifest is not JSON".to_string()
            )),
            Err("the manifest is not JSON".to_string())
        );
        assert!(!dir.path().join(MARKER).exists());
        let empty = tempfile::tempdir().unwrap();
        let outcome = ensure_bridged(empty.path(), "v1", |_| Ok(()));
        assert!(outcome.unwrap_err().ends_with("holds no manifest.json"));
    }

    #[test]
    fn versions_are_sixteen_hex_digits_of_the_script() {
        let one = version_of("a");
        assert_eq!(one.len(), 16);
        assert!(one.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(one, version_of("a"));
        assert_ne!(one, version_of("b"));
    }

    #[test]
    fn dot_files_and_dot_directories_are_left_out_at_every_depth() {
        let source = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        put(&source.path().join("manifest.json"), MANIFEST);
        put(&source.path().join("sw.js"), "// worker");
        put(&source.path().join("icons/16.png"), "png");
        put(&source.path().join("src/y.js"), "y");
        put(&source.path().join(".git/config"), "secret");
        put(&source.path().join(".DS_Store"), "junk");
        put(&source.path().join("icons/.keep"), "");
        put(&source.path().join(".hidden/a.js"), "a");
        put(&source.path().join("src/.cache/x.js"), "x");
        let dest = store.path().join("copy");
        let (copy, skipped) = copy_unpacked(source.path(), &dest).unwrap();
        assert_eq!(copy, dest);
        assert!(skipped.is_empty(), "{skipped:?}");
        for present in ["manifest.json", "sw.js", "icons/16.png", "src/y.js"] {
            assert!(dest.join(present).is_file(), "{present}");
        }
        for absent in [".git", ".DS_Store", "icons/.keep", ".hidden", "src/.cache"] {
            assert!(!dest.join(absent).exists(), "{absent}");
        }
    }

    #[test]
    fn a_folder_is_copied_and_bridged_under_the_marker_and_the_source_is_left_alone() {
        let source = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        put(&source.path().join("manifest.json"), MANIFEST);
        put(&source.path().join("sw.js"), "// worker");
        put(&source.path().join("icons/16.png"), "png");
        put(&source.path().join("src/deep/er/x.js"), "x");
        let (copy, bridged) = bridge_source(source.path(), store.path(), "v1").unwrap();
        assert_eq!(bridged, Bridged::Yes);
        assert_eq!(copy.parent(), Some(store.path()));
        // The name is Phase 1's scheme, of the absolute path.
        assert_eq!(
            copy.file_name().unwrap().to_string_lossy(),
            copy_name(&std::path::absolute(source.path()).unwrap())
        );
        assert!(copy.join("src/deep/er/x.js").is_file());
        assert!(copy.join("icons/16.png").is_file());
        assert!(copy.join("nautilus-loader.js").is_file());
        assert!(copy.join("nautilus-bridge/bridge.js").is_file());
        assert_eq!(
            fs::read_to_string(copy.join(MARKER)).unwrap(),
            r#"{"version":"v1"}"#
        );
        // The source: untouched.
        assert_eq!(
            fs::read_to_string(source.path().join("manifest.json")).unwrap(),
            MANIFEST
        );
        assert!(!source.path().join(MARKER).exists());
        assert!(!source.path().join("nautilus-bridge").exists());
        assert!(!source.path().join("nautilus-loader.js").exists());
        // Loading the same folder again replaces the copy, at the same
        // place: an edit arrives, a deleted file goes.
        put(&source.path().join("sw.js"), "// edited");
        fs::remove_file(source.path().join("icons/16.png")).unwrap();
        let (again, _) = bridge_source(source.path(), store.path(), "v1").unwrap();
        assert_eq!(again, copy);
        assert_eq!(
            fs::read_to_string(again.join("sw.js")).unwrap(),
            "// edited"
        );
        assert!(!again.join("icons/16.png").exists());
        assert!(again.join(MARKER).is_file());
    }

    #[test]
    fn a_folder_without_a_manifest_is_not_copied() {
        let source = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        put(&source.path().join("readme.txt"), "no extension here");
        let error = bridge_source(source.path(), store.path(), "v1").unwrap_err();
        assert!(error.ends_with("holds no manifest.json"), "{error}");
        assert_eq!(fs::read_dir(store.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_worker_under_a_dot_directory_fails_the_load_and_leaves_no_copy() {
        let source = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        put(
            &source.path().join("manifest.json"),
            r#"{"background":{"service_worker":".bg/sw.js"}}"#,
        );
        put(&source.path().join(".bg/sw.js"), "// hidden");
        let error = bridge_source(source.path(), store.path(), "v1").unwrap_err();
        assert!(error.contains("not copied"), "{error}");
        assert_eq!(fs::read_dir(store.path()).unwrap().count(), 0);
    }

    /// Phase 1's rewrite, through the marker: applying it again (a new
    /// script version) must not wrap the loader in a loader, and must
    /// leave the worker's file the current script.
    #[test]
    fn the_rewrite_can_be_applied_again() {
        let dir = tempfile::tempdir().unwrap();
        put(&dir.path().join("manifest.json"), MANIFEST);
        put(&dir.path().join("sw.js"), "// worker");
        let apply = |at: &Path| rewrite_in_place(at, &[]).map(|_| ());
        assert_eq!(ensure_bridged(dir.path(), "v1", apply), Ok(true));
        let first = fs::read_to_string(dir.path().join("manifest.json")).unwrap();
        assert_ne!(first, MANIFEST);
        let loader = fs::read_to_string(dir.path().join("nautilus-loader.js")).unwrap();
        // An older build's script on disk, and a newer version asked for.
        fs::write(
            dir.path().join("nautilus-bridge/bridge.js"),
            "// an older bridge",
        )
        .unwrap();
        assert_eq!(ensure_bridged(dir.path(), "v2", apply), Ok(true));
        let second = fs::read_to_string(dir.path().join("manifest.json")).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            fs::read_to_string(dir.path().join("sw.js")).unwrap(),
            "// worker"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("nautilus-loader.js")).unwrap(),
            loader
        );
        // The script the worker loads is the current one, in the one
        // file the loader imports.
        assert_eq!(
            fs::read_to_string(dir.path().join("nautilus-bridge/bridge.js")).unwrap(),
            crate::bridge::worker_script()
        );
        assert!(
            fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().starts_with('_'))
        );
    }

    #[test]
    fn a_marked_copy_is_rewritten_in_place_and_not_copied_again() {
        let source = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        put(&source.path().join("manifest.json"), MANIFEST);
        put(&source.path().join("sw.js"), "// worker");
        let (copy, _) = bridge_source(source.path(), store.path(), "v1").unwrap();
        put(&copy.join("stale.txt"), "x");
        put(
            &copy.join("nautilus-bridge/bridge.js"),
            "// an older bridge",
        );
        let (again, bridged) = bridge_source(&copy, store.path(), "v2").unwrap();
        assert_eq!(again, copy);
        assert_eq!(bridged, Bridged::Yes);
        assert!(
            copy.join("stale.txt").exists(),
            "copied again instead of rewritten"
        );
        assert_eq!(
            fs::read_to_string(copy.join("nautilus-bridge/bridge.js")).unwrap(),
            crate::bridge::worker_script()
        );
        assert_eq!(
            fs::read_to_string(copy.join(MARKER)).unwrap(),
            r#"{"version":"v2"}"#
        );
        let entries: Vec<_> = fs::read_dir(store.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![copy.file_name().unwrap().to_os_string()]);
    }

    #[test]
    fn a_failed_load_leaves_no_partial_copy_and_the_previous_copy_intact() {
        let source = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        put(&source.path().join("manifest.json"), MANIFEST);
        put(&source.path().join("sw.js"), "// worker");
        let (copy, _) = bridge_source(source.path(), store.path(), "v1").unwrap();
        let manifest_before = fs::read_to_string(copy.join("manifest.json")).unwrap();
        // The source's manifest is now broken: the new copy is staged, its
        // rewrite fails, and the store keeps only the previous copy.
        put(&source.path().join("manifest.json"), "not json");
        let error = bridge_source(source.path(), store.path(), "v2").unwrap_err();
        assert!(error.contains("not JSON"), "{error}");
        let names: Vec<_> = fs::read_dir(store.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![copy.file_name().unwrap().to_os_string()]);
        assert_eq!(
            fs::read_to_string(copy.join("manifest.json")).unwrap(),
            manifest_before
        );
        assert_eq!(
            fs::read_to_string(copy.join(MARKER)).unwrap(),
            r#"{"version":"v1"}"#
        );
    }

    #[test]
    fn a_failed_rewrite_of_a_marked_copy_writes_nothing_and_keeps_its_marker() {
        let source = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        put(&source.path().join("manifest.json"), MANIFEST);
        put(&source.path().join("sw.js"), "// worker");
        let (copy, _) = bridge_source(source.path(), store.path(), "v1").unwrap();
        put(
            &copy.join("nautilus-bridge/bridge.js"),
            "// an older bridge",
        );
        // A broken manifest in the copy: the rewrite reads it first and fails
        // before any bridge file is written; the marker stays at v1, so the
        // next start tries again.
        put(&copy.join("manifest.json"), "not json");
        let error = bridge_source(&copy, store.path(), "v2").unwrap_err();
        assert!(error.contains("not JSON"), "{error}");
        assert_eq!(
            fs::read_to_string(copy.join("nautilus-bridge/bridge.js")).unwrap(),
            "// an older bridge"
        );
        assert_eq!(
            fs::read_to_string(copy.join(MARKER)).unwrap(),
            r#"{"version":"v1"}"#
        );
        assert!(copy.join("sw.js").is_file());
        assert!(copy.join("nautilus-loader.js").is_file());
    }

    #[test]
    fn a_marked_copy_whose_worker_is_gone_fails_the_rewrite() {
        let source = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        put(&source.path().join("manifest.json"), MANIFEST);
        put(&source.path().join("sw.js"), "// worker");
        let (copy, _) = bridge_source(source.path(), store.path(), "v1").unwrap();
        // The manifest already names the loader, so the entry check reads the
        // worker's name from the record the first rewrite wrote.
        fs::remove_file(copy.join("sw.js")).unwrap();
        let error = bridge_source(&copy, store.path(), "v2").unwrap_err();
        assert!(error.contains("missing"), "{error}");
        assert_eq!(
            fs::read_to_string(copy.join(MARKER)).unwrap(),
            r#"{"version":"v1"}"#
        );
    }

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
        assert_eq!(rewrite_in_place(&copy, &[]), Ok(Bridged::Yes));
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
            crate::bridge::worker_script()
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
    fn a_windows_spelling_is_folded_before_it_is_hashed_on_every_platform() {
        // Case and separators fold on Windows only.
        assert_eq!(
            path_key_text(r"C:\Users\Me\Ext", true),
            path_key_text(r"c:\users\me\ext", true)
        );
        assert_eq!(
            path_key_text("C:/Users/Me/Ext", true),
            path_key_text(r"c:\users\me\ext", true)
        );
        assert_ne!(
            path_key_text("/Users/Me/Ext", false),
            path_key_text("/users/me/ext", false)
        );
    }

    #[test]
    fn a_loader_path_must_stay_inside_the_extension() {
        let dir = Path::new("/ext/root");
        assert_eq!(
            join_under(dir, "background/nautilus-loader.js"),
            Ok(dir.join("background/nautilus-loader.js"))
        );
        for bad in [
            "/etc/x.js",
            "../x.js",
            "C:/Users/x/nautilus-loader.js",
            "C:sw.js",
            "a:b.js",
        ] {
            assert!(join_under(dir, bad).is_err(), "{bad}");
        }
    }

    // Windows folds the path's case before hashing, so the name differs
    // there; the fold itself has its own tests.
    #[cfg(not(windows))]
    #[test]
    fn a_macos_copy_name_is_unchanged() {
        // Pinned from the scheme before the Windows case fold: existing
        // installs find their copies by this name.
        assert_eq!(
            copy_name(Path::new("/Users/someone/dev/My Extension (v2)")),
            "unpacked-my-extension--v2-cd4209bb"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_loop_in_the_source_is_skipped_and_the_copy_ends() {
        let root = scratch("loop");
        let source = extension(
            &root,
            r#"{"background":{"service_worker":"background/sw.js"}}"#,
        );
        std::os::unix::fs::symlink(&source, source.join("loop")).unwrap();
        std::os::unix::fs::symlink(&source, source.join("background/up")).unwrap();
        let (copy, bridged) = copy_and_rewrite(&source, &root.join("store")).unwrap();
        assert_eq!(bridged, Bridged::Yes);
        assert!(fs::symlink_metadata(copy.join("loop")).is_err());
        assert!(fs::symlink_metadata(copy.join("background/up")).is_err());
        assert!(copy.join("background/sw.js").is_file());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_worker_entry_is_checked_present_skipped_or_absent() {
        assert_eq!(entry_in_copy("background/sw.js", true, &[]), Ok(()));
        let skipped = [PathBuf::from("background/sw.js")];
        let linked = entry_in_copy("background/sw.js", false, &skipped).unwrap_err();
        assert!(linked.contains("symbolic link"), "{linked}");
        let absent = entry_in_copy("background/sw.js", false, &[]).unwrap_err();
        assert!(absent.contains("missing"), "{absent}");
        // A link elsewhere in the copy does not stand in for the entry.
        let other = [PathBuf::from("lib/other.js")];
        assert!(
            entry_in_copy("background/sw.js", false, &other)
                .unwrap_err()
                .contains("missing")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_worker_entry_that_is_a_link_fails_the_load_and_leaves_no_copy() {
        let root = scratch("linked-entry");
        let source = extension(
            &root,
            r#"{"background":{"service_worker":"background/sw.js"}}"#,
        );
        fs::remove_file(source.join("background/sw.js")).unwrap();
        fs::write(
            source.join("background/real.js"),
            "importScripts('lib.js');",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            source.join("background/real.js"),
            source.join("background/sw.js"),
        )
        .unwrap();
        let error = copy_and_rewrite(&source, &root.join("store")).unwrap_err();
        assert!(error.contains("symbolic link"), "{error}");
        assert!(!root.join("store").join(copy_name(&source)).exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_worker_entry_that_is_absent_fails_the_load() {
        let root = scratch("absent-entry");
        let source = extension(
            &root,
            r#"{"background":{"service_worker":"background/missing.js"}}"#,
        );
        let error = copy_and_rewrite(&source, &root.join("store")).unwrap_err();
        assert!(error.contains("missing"), "{error}");
        assert!(!root.join("store").join(copy_name(&source)).exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_in_the_source_does_not_fail_the_load() {
        let root = scratch("dangling");
        let source = extension(
            &root,
            r#"{"background":{"service_worker":"background/sw.js"}}"#,
        );
        std::os::unix::fs::symlink(root.join("nowhere"), source.join("dangling.js")).unwrap();
        let (copy, bridged) = copy_and_rewrite(&source, &root.join("store")).unwrap();
        assert_eq!(bridged, Bridged::Yes);
        assert!(fs::symlink_metadata(copy.join("dangling.js")).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_source_that_contains_is_inside_or_is_the_store_is_refused() {
        let root = scratch("nested");
        let manifest = r#"{"background":{"service_worker":"background/sw.js"}}"#;
        // The store inside the source, as a profile inside an extension.
        let source = extension(&root, manifest);
        let inside = source.join("chromium/extensions");
        assert!(copy_and_rewrite(&source, &inside).is_err());
        assert!(!inside.join(copy_name(&source)).exists());
        // The source inside the store.
        let profile = root.join("profile");
        let nested = extension(&profile, manifest);
        assert!(copy_and_rewrite(&nested, &profile).is_err());
        // The source is the store.
        assert!(copy_and_rewrite(&source, &source).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_copy_leaves_nothing_behind_in_the_store() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("partial");
        let source = extension(
            &root,
            r#"{"background":{"service_worker":"background/sw.js"}}"#,
        );
        let unreadable = source.join("zz.js");
        fs::write(&unreadable, "x").unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        let store = root.join("store");
        let result = copy_and_rewrite(&source, &store);
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(result.is_err());
        let leftovers: Vec<_> = fs::read_dir(&store)
            .map(|dir| dir.map(|e| e.unwrap().file_name()).collect())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "{leftovers:?}");
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
