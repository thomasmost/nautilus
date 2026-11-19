//! The rule a scripted run's control directory must meet
//! (`NAUTILUS_TEST_CONTROL`, `ExtensionManager::poll_control`). Only a
//! build with the `test-hooks` feature has the control at all; a release
//! build has none of this.

/// Why `NAUTILUS_TEST_CONTROL`'s directory must not be used, or `None`
/// when it may. Unix: the directory must belong to the user that owns the
/// profile's engine directory (the browser created it), and no one but its
/// owner may write to it (`control_refusal_for`). Windows: the directory,
/// canonicalised, must lie inside the user's own profile tree (the canonical
/// `%USERPROFILE%`, which holds `%TEMP%`), by `outside_profile`. Any other
/// platform refuses, since it has no rule.
#[cfg(unix)]
pub(crate) fn control_refusal(dir: &std::path::Path, profile: &std::path::Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = match std::fs::metadata(dir) {
        Ok(meta) => meta,
        Err(e) => return Some(format!("it cannot be read: {e}")),
    };
    match std::fs::metadata(profile) {
        Ok(owner) => control_refusal_for(&meta, owner.uid()),
        Err(e) => Some(format!("the profile directory cannot be read: {e}")),
    }
}

#[cfg(windows)]
pub(crate) fn control_refusal(dir: &std::path::Path, _profile: &std::path::Path) -> Option<String> {
    let dir = match std::fs::canonicalize(dir) {
        Ok(dir) => dir,
        Err(e) => return Some(format!("it cannot be canonicalised: {e}")),
    };
    if !dir.is_dir() {
        return Some("it is not a directory".to_string());
    }
    let Some(home) = std::env::var_os("USERPROFILE") else {
        return Some("USERPROFILE is not set, so the profile tree is unknown".to_string());
    };
    match std::fs::canonicalize(&home) {
        Ok(home) => outside_profile(&dir, &home, true),
        Err(e) => Some(format!(
            "the user's profile folder cannot be canonicalised: {e}"
        )),
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn control_refusal(
    _dir: &std::path::Path,
    _profile: &std::path::Path,
) -> Option<String> {
    Some("this platform has no rule for the control directory".to_string())
}

/// The Windows containment rule, as a pure function over canonical paths so
/// that it is tested on every platform: `dir` must be `root` or inside it,
/// compared component by component (`C:\Users\Thomas2` is not inside
/// `C:\Users\Thomas`), with each name compared case-insensitively when
/// `fold_case` (Windows' file system). `None` when `dir` may be used.
#[cfg_attr(not(windows), allow(dead_code))]
fn outside_profile(
    dir: &std::path::Path,
    root: &std::path::Path,
    fold_case: bool,
) -> Option<String> {
    let names = |path: &std::path::Path| -> Vec<String> {
        path.components()
            .map(|part| {
                let name = part.as_os_str().to_string_lossy().into_owned();
                if fold_case { name.to_lowercase() } else { name }
            })
            .collect()
    };
    let (dir_names, root_names) = (names(dir), names(root));
    let inside = !root_names.is_empty()
        && dir_names.len() >= root_names.len()
        && dir_names[..root_names.len()] == root_names[..];
    if inside {
        None
    } else {
        Some(format!(
            "it is not inside your profile folder {}",
            root.display()
        ))
    }
}

/// The owner and mode test on the control directory's metadata, against
/// the uid that owns the profile.
#[cfg(unix)]
fn control_refusal_for(meta: &std::fs::Metadata, owner: u32) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    if !meta.is_dir() {
        return Some("it is not a directory".to_string());
    }
    if meta.uid() != owner {
        return Some(format!(
            "it belongs to uid {}, not to uid {owner}, which owns the profile",
            meta.uid()
        ));
    }
    if meta.mode() & 0o022 != 0 {
        return Some(format!(
            "it is writable by its group or others (mode {:o})",
            meta.mode() & 0o777
        ));
    }
    None
}

/// Control-directory tests. The containment rule runs on every platform; the
/// owner and mode check on Unix.
#[cfg(test)]
mod containment_tests {
    use std::path::Path;

    use super::outside_profile;

    const HOME: &str = "C:/Users/Thomas";

    #[test]
    fn a_directory_inside_the_profile_tree_is_accepted() {
        let home = Path::new(HOME);
        assert_eq!(
            outside_profile(
                Path::new("C:/Users/Thomas/AppData/Local/Temp/ctl-1"),
                home,
                true
            ),
            None
        );
        assert_eq!(
            outside_profile(home, home, true),
            None,
            "the profile folder itself"
        );
    }

    #[test]
    fn a_name_that_only_starts_like_the_profile_is_not_inside_it() {
        let home = Path::new(HOME);
        assert!(outside_profile(Path::new("C:/Users/Thomas2/ctl"), home, true).is_some());
        assert!(outside_profile(Path::new("C:/Users/ThomasX"), home, true).is_some());
    }

    #[test]
    fn a_directory_elsewhere_or_above_the_profile_is_refused() {
        let home = Path::new(HOME);
        assert!(outside_profile(Path::new("D:/work/ctl"), home, true).is_some());
        assert!(outside_profile(Path::new("C:/Users"), home, true).is_some());
        assert!(outside_profile(Path::new("C:/Temp/ctl"), home, true).is_some());
    }

    #[test]
    fn names_are_compared_case_insensitively_on_windows_only() {
        let home = Path::new(HOME);
        let spelled = Path::new("c:/users/THOMAS/ctl");
        assert_eq!(outside_profile(spelled, home, true), None);
        assert!(outside_profile(spelled, home, false).is_some());
    }

    #[test]
    fn an_empty_profile_path_refuses_everything() {
        assert!(outside_profile(Path::new("C:/Users/Thomas/ctl"), Path::new(""), true).is_some());
    }
}

/// Control-directory tests (Unix: the owner and mode check).
#[cfg(all(test, unix))]
mod control_tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::PathBuf;

    use super::control_refusal_for;

    /// A directory under the system temp dir with `mode`, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str, mode: u32) -> Dir {
            let path =
                std::env::temp_dir().join(format!("nautilus-control-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            Dir(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_directory_only_its_owner_can_write_is_accepted() {
        let dir = Dir::new("owner-only", 0o700);
        let meta = std::fs::metadata(&dir.0).unwrap();
        assert_eq!(control_refusal_for(&meta, meta.uid()), None);
    }

    #[test]
    fn a_directory_readable_but_not_writable_by_others_is_accepted() {
        let dir = Dir::new("read-only-for-others", 0o755);
        let meta = std::fs::metadata(&dir.0).unwrap();
        assert_eq!(control_refusal_for(&meta, meta.uid()), None);
    }

    #[test]
    fn a_directory_writable_by_its_group_or_others_is_refused() {
        for (tag, mode) in [
            ("group-write", 0o770),
            ("other-write", 0o702),
            ("open", 0o777),
        ] {
            let dir = Dir::new(tag, mode);
            let meta = std::fs::metadata(&dir.0).unwrap();
            let refusal = control_refusal_for(&meta, meta.uid());
            assert!(
                refusal
                    .as_deref()
                    .is_some_and(|r| r.contains("writable by its group or others")),
                "mode {mode:o} should be refused, got {refusal:?}"
            );
        }
    }

    #[test]
    fn a_directory_owned_by_another_user_is_refused() {
        let dir = Dir::new("other-owner", 0o700);
        let meta = std::fs::metadata(&dir.0).unwrap();
        let refusal = control_refusal_for(&meta, meta.uid().wrapping_add(1));
        assert!(
            refusal
                .as_deref()
                .is_some_and(|r| r.contains("belongs to uid")),
            "got {refusal:?}"
        );
    }

    #[test]
    fn something_that_is_not_a_directory_is_refused() {
        let dir = Dir::new("not-a-dir", 0o700);
        let file = dir.0.join("plain-file");
        std::fs::write(&file, b"x").unwrap();
        let meta = std::fs::metadata(&file).unwrap();
        assert_eq!(
            control_refusal_for(&meta, meta.uid()),
            Some("it is not a directory".to_string())
        );
    }
}
