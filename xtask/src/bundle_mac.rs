//! `cargo xtask bundle-mac`: the macOS application bundle.
//!
//! CEF on macOS runs from a bundle with a fixed shape (CEF's
//! `GeneralUsage` wiki, "macOS"): the framework under
//! `Contents/Frameworks`, and one helper app per process kind beside it,
//! each named after the main executable (`<Name> Helper`, `<Name> Helper
//! (GPU)`, `(Renderer)`, `(Plugin)`, `(Alerts)`); Chromium derives the
//! helper paths from the main executable's file name. The browser
//! executable loads the framework from `../Frameworks` and a helper from
//! `../../..` (`nautilus_engine::run_subprocess` and `run_helper`).
//!
//! The task builds the release binaries, lays the bundle out under
//! `target/release/Nautilus.app`, writes the property lists, copies the
//! framework from the CEF distribution the build used, and signs the
//! result ad hoc (every executable on Apple silicon must carry a
//! signature, and `cargo` only signs what it links).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The product as the bundle names it: the app, its executable, and
/// the helper apps (`Nautilus Helper`, …). Chromium derives the helper
/// names from the main executable's file name, so the two agree.
const PRODUCT: &str = "Nautilus";
/// `identifier` in `[package.metadata.packager]` of the binary crate.
const IDENTIFIER: &str = "org.nautilus.software";
const MINIMUM_SYSTEM: &str = "12.0";
const FRAMEWORK: &str = "Chromium Embedded Framework.framework";
/// Chromium's process kinds, as helper app suffixes (CEF's layout).
const HELPERS: [&str; 5] = ["", " (GPU)", " (Renderer)", " (Plugin)", " (Alerts)"];

pub fn bundle_mac(root: &Path, version: &str) -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("the macOS bundle is built on macOS (the binaries and the CEF framework are macOS builds)".to_string());
    }
    let status = Command::new(cargo())
        .args(["build", "--release", "-p", "nautilus"])
        .current_dir(root)
        .status()
        .map_err(|e| format!("running cargo build: {e}"))?;
    if !status.success() {
        return Err("cargo build --release -p nautilus failed".to_string());
    }
    let release = root.join("target").join("release");
    let browser_bin = release.join("nautilus");
    let helper_bin = release.join("nautilus-helper");
    for bin in [&browser_bin, &helper_bin] {
        if !bin.is_file() {
            return Err(format!("{} was not built", bin.display()));
        }
    }
    let framework = find_framework(root)?;
    let icon = root.join("assets").join("icons").join("nautilus.icns");

    let app = release.join(format!("{PRODUCT}.app"));
    if app.exists() {
        fs::remove_dir_all(&app).map_err(|e| format!("removing {}: {e}", app.display()))?;
    }
    let contents = app.join("Contents");
    let frameworks = contents.join("Frameworks");
    io_err(fs::create_dir_all(contents.join("MacOS")), &contents)?;
    io_err(fs::create_dir_all(contents.join("Resources")), &contents)?;
    io_err(fs::create_dir_all(&frameworks), &frameworks)?;

    // The browser.
    copy_executable(&browser_bin, &contents.join("MacOS").join(PRODUCT))?;
    let icon_file = if icon.is_file() {
        let name = format!("{PRODUCT}.icns");
        io_err(
            fs::copy(&icon, contents.join("Resources").join(&name)),
            &icon,
        )?;
        Some(name)
    } else {
        eprintln!("warning: no icon at {}", icon.display());
        None
    };
    write(
        &contents.join("Info.plist"),
        &info_plist(PRODUCT, IDENTIFIER, version, icon_file.as_deref(), false),
    )?;

    // The framework.
    copy_dir(&framework, &frameworks.join(FRAMEWORK))?;

    // The helpers.
    for suffix in HELPERS {
        let name = format!("{PRODUCT} Helper{suffix}");
        let helper_app = frameworks.join(format!("{name}.app"));
        let helper_contents = helper_app.join("Contents");
        io_err(
            fs::create_dir_all(helper_contents.join("MacOS")),
            &helper_contents,
        )?;
        copy_executable(&helper_bin, &helper_contents.join("MacOS").join(&name))?;
        let kind = suffix
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .to_ascii_lowercase();
        let identifier = if kind.is_empty() {
            format!("{IDENTIFIER}.helper")
        } else {
            format!("{IDENTIFIER}.helper.{kind}")
        };
        write(
            &helper_contents.join("Info.plist"),
            &info_plist(&name, &identifier, version, None, true),
        )?;
    }

    sign(&app)?;
    Ok(app)
}

/// The CEF distribution the build used: `CEF_PATH` as `cef-dll-sys`
/// resolves it (the versioned subdirectory for this CEF version, else
/// the directory itself), holding the framework at its top level (the
/// flattened layout the `download-cef` crate unpacks) or under
/// `Release/` (a distribution unpacked by hand).
fn find_framework(root: &Path) -> Result<PathBuf, String> {
    let cef_path = std::env::var_os("CEF_PATH").map(PathBuf::from).ok_or(
        "CEF_PATH is not set; point it at the CEF distribution the build used (README.md)",
    )?;
    let mut candidates = Vec::new();
    if let Some(version) = cef_version(root) {
        let arch = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        };
        candidates.push(cef_path.join(&version).join(format!("cef_macos_{arch}")));
    }
    candidates.push(cef_path.clone());
    for dir in &candidates {
        for framework in [dir.join(FRAMEWORK), dir.join("Release").join(FRAMEWORK)] {
            if framework.is_dir() {
                return Ok(framework);
            }
        }
    }
    Err(format!(
        "no {FRAMEWORK} under {} (looked in {})",
        cef_path.display(),
        candidates
            .iter()
            .map(|c| c.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// The CEF version the `cef` crate in `Cargo.lock` is built against:
/// the part after `+` of its version (`154.5.0+154.0.34` → `154.0.34`).
fn cef_version(root: &Path) -> Option<String> {
    let lock = fs::read_to_string(root.join("Cargo.lock")).ok()?;
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() != "name = \"cef\"" {
            continue;
        }
        let version = lines.next()?.trim().strip_prefix("version = \"")?;
        let version = version.trim_end_matches('"');
        return version.split_once('+').map(|(_, cef)| cef.to_string());
    }
    None
}

fn info_plist(
    name: &str,
    identifier: &str,
    version: &str,
    icon: Option<&str>,
    helper: bool,
) -> String {
    let mut keys = vec![
        ("CFBundleDevelopmentRegion", "en".to_string()),
        ("CFBundleDisplayName", name.to_string()),
        ("CFBundleExecutable", name.to_string()),
        ("CFBundleIdentifier", identifier.to_string()),
        ("CFBundleInfoDictionaryVersion", "6.0".to_string()),
        ("CFBundleName", name.to_string()),
        ("CFBundlePackageType", "APPL".to_string()),
        ("CFBundleShortVersionString", version.to_string()),
        ("CFBundleSignature", "????".to_string()),
        ("CFBundleVersion", version.to_string()),
        ("LSMinimumSystemVersion", MINIMUM_SYSTEM.to_string()),
        ("NSPrincipalClass", "NSApplication".to_string()),
    ];
    if let Some(icon) = icon {
        keys.push(("CFBundleIconFile", icon.to_string()));
    }
    let mut body = String::new();
    for (key, value) in keys {
        body.push_str(&format!(
            "\t<key>{key}</key>\n\t<string>{}</string>\n",
            escape_xml(&value)
        ));
    }
    // Booleans.
    let flags: &[(&str, bool)] = if helper {
        // Helpers show no Dock icon and no menu.
        &[("LSUIElement", true), ("NSHighResolutionCapable", true)]
    } else {
        &[
            ("NSHighResolutionCapable", true),
            ("NSSupportsAutomaticGraphicsSwitching", true),
        ]
    };
    for (key, on) in flags {
        body.push_str(&format!(
            "\t<key>{key}</key>\n\t<{}/>\n",
            if *on { "true" } else { "false" }
        ));
    }
    if !helper {
        // What the browser may ask the user for; the strings show in
        // the system's permission prompts.
        for (key, value) in [
            (
                "NSCameraUsageDescription",
                "A web page asked to use the camera.",
            ),
            (
                "NSMicrophoneUsageDescription",
                "A web page asked to use the microphone.",
            ),
            (
                "NSLocationUsageDescription",
                "A web page asked for your location.",
            ),
        ] {
            body.push_str(&format!(
                "\t<key>{key}</key>\n\t<string>{}</string>\n",
                escape_xml(value)
            ));
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n{body}</dict>\n</plist>\n"
    )
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Ad-hoc signature over the whole bundle, if `codesign` is there.
fn sign(app: &Path) -> Result<(), String> {
    let status = Command::new("codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(app)
        .status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("codesign failed: {status}")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            eprintln!("warning: codesign not found; the bundle is unsigned");
            Ok(())
        }
        Err(e) => Err(format!("running codesign: {e}")),
    }
}

fn copy_executable(from: &Path, to: &Path) -> Result<(), String> {
    io_err(fs::copy(from, to), from)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        io_err(
            fs::set_permissions(to, fs::Permissions::from_mode(0o755)),
            to,
        )?;
    }
    Ok(())
}

/// Copy a directory tree, symbolic links as links (a framework may
/// hold them).
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    io_err(fs::create_dir_all(to), to)?;
    for entry in io_err(fs::read_dir(from), from)? {
        let entry = io_err(entry, from)?;
        let kind = io_err(entry.file_type(), &entry.path())?;
        let target = to.join(entry.file_name());
        if kind.is_symlink() {
            #[cfg(unix)]
            {
                let link = io_err(fs::read_link(entry.path()), &entry.path())?;
                io_err(std::os::unix::fs::symlink(&link, &target), &target)?;
            }
            #[cfg(not(unix))]
            return Err(format!(
                "cannot copy the symbolic link {}",
                entry.path().display()
            ));
        } else if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            io_err(fs::copy(entry.path(), &target), &entry.path())?;
        }
    }
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    io_err(fs::write(path, text), path)
}

fn io_err<T>(result: io::Result<T>, path: &Path) -> Result<T, String> {
    result.map_err(|e| format!("{}: {e}", path.display()))
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cef_version_is_the_build_metadata_of_the_cef_crate() {
        let dir = std::env::temp_dir().join(format!("nautilus-xtask-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Cargo.lock"),
            "[[package]]\nname = \"cc\"\nversion = \"1.0\"\n\n[[package]]\nname = \"cef\"\nversion = \"154.5.0+154.0.34\"\n",
        )
        .unwrap();
        assert_eq!(cef_version(&dir).as_deref(), Some("154.0.34"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_property_list_names_the_executable_and_marks_helpers() {
        let main = info_plist(
            "Nautilus",
            "org.nautilus.software",
            "0.1.0",
            Some("Nautilus.icns"),
            false,
        );
        assert!(main.contains("<key>CFBundleExecutable</key>\n\t<string>Nautilus</string>"));
        assert!(main.contains("<key>CFBundleIconFile</key>\n\t<string>Nautilus.icns</string>"));
        assert!(!main.contains("LSUIElement"));
        let helper = info_plist(
            "Nautilus Helper (GPU)",
            "org.nautilus.software.helper.gpu",
            "0.1.0",
            None,
            true,
        );
        assert!(helper.contains("<key>LSUIElement</key>\n\t<true/>"));
        assert!(helper.contains("<string>Nautilus Helper (GPU)</string>"));
        assert!(!helper.contains("CFBundleIconFile"));
    }
}
