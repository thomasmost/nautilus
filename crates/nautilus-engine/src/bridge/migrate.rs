//! Moving an extension an earlier build loaded from the user's own folder
//! into the profile's copy. The original is switched off while the copy is
//! made, and stays switched off: Chromium does not let the engine remove it
//! (its uninstall needs a user gesture), and a keyless copy has a new id, so
//! the original keeps its stored data under the old id and stays in the list.
//! When the copy cannot be made, the original stays switched off. No CEF here:
//! the manager asks, and carries out what it is told.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The original extension a migration moves from.
struct Original {
    id: String,
    /// Whether the user had it on. The copy takes the same state.
    enabled: bool,
}

/// What the manager does when a migration's copy has been made.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Next {
    /// Load the copy from this folder.
    Load(PathBuf),
    /// The copy could not be made: the original `id` stays switched off.
    Disable(String),
}

#[derive(Default)]
pub(crate) struct Migrations {
    /// The migrations in progress, by the source folder's name as the unpack
    /// queue reports it.
    started: HashMap<String, Original>,
    /// The copies made, and whether each must be on once it is listed.
    settling: HashMap<PathBuf, bool>,
}

impl Migrations {
    /// The original `id` in `source` (a folder the user loaded) is to move
    /// into a copy. `enabled` is the state the user left it in.
    pub(crate) fn start(&mut self, source: &str, id: &str, enabled: bool) {
        self.started.insert(
            source.to_string(),
            Original {
                id: id.to_string(),
                enabled,
            },
        );
    }

    /// The copy of `source` finished with `outcome`. None when `source` is
    /// not a migration in progress.
    pub(crate) fn finish(
        &mut self,
        source: &str,
        outcome: &Result<PathBuf, String>,
    ) -> Option<Next> {
        let original = self.started.remove(source)?;
        match outcome {
            Ok(copy) => {
                self.settling.insert(copy.clone(), original.enabled);
                Some(Next::Load(copy.clone()))
            }
            Err(_) => Some(Next::Disable(original.id)),
        }
    }

    /// The listing names `dir` as a loaded folder: if it is a migrated copy's
    /// folder, the state it must be in is returned once.
    pub(crate) fn listed(&mut self, dir: &Path) -> Option<bool> {
        self.settling.remove(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy() -> PathBuf {
        PathBuf::from("/profile/extensions/unpacked-a-0123abcd")
    }

    #[test]
    fn a_keyless_copy_takes_the_originals_state_once_listed() {
        let mut migrations = Migrations::default();
        migrations.start("/users/src", "old", true);
        assert_eq!(
            migrations.finish("/users/src", &Ok(copy())),
            Some(Next::Load(copy()))
        );
        assert_eq!(migrations.listed(&copy()), Some(true));
        assert_eq!(migrations.listed(&copy()), None);
    }

    #[test]
    fn a_copy_of_a_switched_off_original_stays_off_once_listed() {
        let mut migrations = Migrations::default();
        migrations.start("/users/src", "old", false);
        migrations.finish("/users/src", &Ok(copy()));
        assert_eq!(migrations.listed(&copy()), Some(false));
    }

    #[test]
    fn a_keyed_copy_takes_the_originals_state_once_listed() {
        for enabled in [true, false] {
            let mut migrations = Migrations::default();
            migrations.start("/users/src", "keyed", enabled);
            assert_eq!(
                migrations.finish("/users/src", &Ok(copy())),
                Some(Next::Load(copy()))
            );
            assert_eq!(migrations.listed(&copy()), Some(enabled));
        }
    }

    #[test]
    fn a_copy_that_is_not_listed_takes_nothing_away() {
        let mut migrations = Migrations::default();
        migrations.start("/users/src", "old", true);
        migrations.finish("/users/src", &Ok(copy()));
        let elsewhere = PathBuf::from("/profile/extensions/unpacked-b-4567cdef");
        assert_eq!(migrations.listed(&elsewhere), None);
        assert!(migrations.listed(&copy()).is_some());
    }

    #[test]
    fn a_failed_copy_switches_the_original_off_and_settles_nothing() {
        for enabled in [false, true] {
            let mut migrations = Migrations::default();
            migrations.start("/users/src", "old", enabled);
            assert_eq!(
                migrations.finish("/users/src", &Err("unreadable".to_string())),
                Some(Next::Disable("old".to_string()))
            );
            assert_eq!(migrations.listed(&copy()), None);
        }
    }

    #[test]
    fn a_folder_that_is_not_migrating_is_not_a_copy() {
        let mut migrations = Migrations::default();
        assert_eq!(migrations.finish("/users/src", &Ok(copy())), None);
        migrations.start("/users/src", "old", true);
        migrations.finish("/users/src", &Ok(copy()));
        // Finished: the same source is not migrated again by a second outcome.
        assert_eq!(migrations.finish("/users/src", &Ok(copy())), None);
    }
}
