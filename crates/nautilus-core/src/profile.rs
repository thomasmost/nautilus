//! Per-user profile directories. All of a profile's state lives under one
//! directory so Phase 2 can run one process per profile.
//!
//! | OS      | Profile directory                                          |
//! |---------|------------------------------------------------------------|
//! | macOS   | `~/Library/Application Support/Nautilus/profiles/<name>/`   |
//! | Windows | `%APPDATA%\Nautilus\profiles\<name>\`                       |

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use directories::BaseDirs;

pub const APP_DIR_NAME: &str = "Nautilus";
pub const PROFILES_DIR_NAME: &str = "profiles";
pub const DEFAULT_PROFILE_NAME: &str = "default";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub dir: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// Names are one path component: ASCII letters, digits, `-`, `_`, and
    /// `.`. A name is never empty, never ends in `.` (Windows strips trailing
    /// dots, so `a.` would resolve to `a`, and `.`/`..` are covered by this
    /// rule), and is never a Windows device name such as `CON` or `COM1`,
    /// with or without an extension, in any letter case. These rules apply on
    /// every platform so profile names stay portable.
    InvalidName(String),
    /// The OS reported no home or config directory for this user.
    NoConfigDirectory,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::InvalidName(name) => write!(
                f,
                "invalid profile name {name:?}: use ASCII letters, digits, '-', '_', or '.', without a trailing '.' or a Windows device name"
            ),
            ProfileError::NoConfigDirectory => {
                write!(f, "could not determine the user's configuration directory")
            }
        }
    }
}

impl std::error::Error for ProfileError {}

impl Profile {
    /// Resolve `name` under the current user's per-OS config directory.
    pub fn resolve(name: &str) -> Result<Profile, ProfileError> {
        let base = BaseDirs::new().ok_or(ProfileError::NoConfigDirectory)?;
        Profile::resolve_under(base.config_dir(), name)
    }

    /// Resolve `name` under an explicit config root. `resolve` passes the
    /// OS config directory; tests pass a temporary directory.
    pub fn resolve_under(config_root: &Path, name: &str) -> Result<Profile, ProfileError> {
        if !is_valid_name(name) {
            return Err(ProfileError::InvalidName(name.to_string()));
        }
        Ok(Profile {
            name: name.to_string(),
            dir: config_root
                .join(APP_DIR_NAME)
                .join(PROFILES_DIR_NAME)
                .join(name),
        })
    }

    pub fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.toml")
    }

    /// Reserved for Phase 2.
    pub fn keymap_path(&self) -> PathBuf {
        self.dir.join("keymap.toml")
    }

    /// The engine's data directory (Chromium's cache path): cookies, local
    /// storage, the HTTP cache, and the rest of the browsing state.
    pub fn engine_dir(&self) -> PathBuf {
        self.dir.join("chromium")
    }

    /// Create the profile directory and the engine's subdirectory.
    pub fn ensure_dirs(&self) -> io::Result<()> {
        std::fs::create_dir_all(self.engine_dir())
    }

    /// The names of the other profiles on this machine: every validly
    /// named directory under the profiles directory except this one,
    /// sorted. Each is a profile another `nautilus --profile <name>` process
    /// can run. Returns nothing if the directory cannot be read.
    pub fn others(&self) -> Vec<String> {
        let Some(profiles_dir) = self.dir.parent() else {
            return Vec::new();
        };
        let mut names: Vec<String> = std::fs::read_dir(profiles_dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .filter(|name| is_valid_name(name) && *name != self.name)
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.ends_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !is_windows_device_name(name.split('.').next().unwrap_or(name))
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`, and `LPT0`-`LPT9`, compared
/// case-insensitively. Windows reserves these even with an extension, so
/// callers pass the part of the name before the first `.`.
fn is_windows_device_name(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            let bytes = upper.as_bytes();
            bytes.len() == 4
                && (upper.starts_with("COM") || upper.starts_with("LPT"))
                && bytes[3].is_ascii_digit()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_under_app_and_profiles_directories() {
        let p = Profile::resolve_under(Path::new("/cfg"), "work").unwrap();
        assert_eq!(p.name, "work");
        assert_eq!(p.dir, Path::new("/cfg/Nautilus/profiles/work"));
        assert_eq!(
            p.settings_path(),
            Path::new("/cfg/Nautilus/profiles/work/settings.toml")
        );
        assert_eq!(
            p.keymap_path(),
            Path::new("/cfg/Nautilus/profiles/work/keymap.toml")
        );
        assert_eq!(
            p.engine_dir(),
            Path::new("/cfg/Nautilus/profiles/work/chromium")
        );
    }

    #[test]
    fn rejects_names_that_are_not_a_single_path_component() {
        for bad in ["", ".", "..", "...", "a.", "a/b", "a\\b", "a b", "ä"] {
            assert_eq!(
                Profile::resolve_under(Path::new("/cfg"), bad),
                Err(ProfileError::InvalidName(bad.to_string())),
                "{bad:?} should be rejected"
            );
        }
        assert!(Profile::resolve_under(Path::new("/cfg"), "ok-name_1.2").is_ok());
    }

    #[test]
    fn rejects_windows_device_names_in_any_case_and_with_an_extension() {
        let reserved = [
            "con", "CON", "Prn", "aux", "nul", "com0", "com1", "COM9", "lpt0", "LPT9", "con.txt",
            "Nul.log",
        ];
        for bad in reserved {
            assert_eq!(
                Profile::resolve_under(Path::new("/cfg"), bad),
                Err(ProfileError::InvalidName(bad.to_string())),
                "{bad:?} should be rejected"
            );
        }
        for ok in ["console", "com10", "COM", "nul2", "a.con"] {
            assert!(
                Profile::resolve_under(Path::new("/cfg"), ok).is_ok(),
                "{ok:?} should be accepted"
            );
        }
    }

    #[test]
    fn resolve_uses_the_platform_config_directory() {
        let p = Profile::resolve(DEFAULT_PROFILE_NAME).unwrap();
        let dir = p.dir.to_string_lossy().replace('\\', "/");
        assert!(dir.ends_with("/Nautilus/profiles/default"), "{dir}");
        if cfg!(target_os = "macos") {
            assert!(
                dir.contains("/Library/Application Support/Nautilus/"),
                "{dir}"
            );
        }
        if cfg!(target_os = "windows") {
            assert!(dir.contains("/AppData/Roaming/Nautilus/"), "{dir}");
        }
    }

    #[test]
    fn ensure_dirs_creates_profile_and_engine_directories() {
        let root = tempfile::tempdir().unwrap();
        let p = Profile::resolve_under(root.path(), "t").unwrap();
        p.ensure_dirs().unwrap();
        assert!(p.engine_dir().is_dir());
    }
}

#[cfg(test)]
mod other_profiles_tests {
    use super::*;

    #[test]
    fn others_lists_sibling_profile_directories_sorted_without_self() {
        let root = tempfile::tempdir().expect("temp dir");
        let me = Profile::resolve_under(root.path(), "work").unwrap();
        for name in ["work", "zeta", "alpha", "bad name"] {
            std::fs::create_dir_all(root.path().join("Nautilus/profiles").join(name)).unwrap();
        }
        std::fs::write(root.path().join("Nautilus/profiles/notes.txt"), "x").unwrap();
        assert_eq!(me.others(), vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[test]
    fn others_is_empty_when_the_profiles_directory_does_not_exist() {
        let root = tempfile::tempdir().expect("temp dir");
        let me = Profile::resolve_under(root.path(), "solo").unwrap();
        assert!(me.others().is_empty());
    }
}
