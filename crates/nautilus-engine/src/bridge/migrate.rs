//! Moving an extension an earlier build loaded from the user's own folder
//! into the profile's copy. The original is removed only once the copy has
//! been bridged and listed; when the copy cannot be made, the original is
//! switched off, so it never runs unbridged with Chromium's own tab
//! functions. No CEF here: the manager asks, and carries out what it is told.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The original extension a migration moves from.
struct Original {
    id: String,
    /// Whether its manifest carries a key. A keyed copy keeps the id, so the
    /// original is replaced by Chromium's load; a keyless one gets a new id,
    /// so the original must be removed.
    keyed: bool,
}

/// What the manager does when a migration's copy has been made.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Next {
    /// Load the copy from this folder.
    Load(PathBuf),
    /// The copy could not be made: switch the original `id` off.
    Disable(String),
}

#[derive(Default)]
pub(crate) struct Migrations {
    /// The migrations in progress, by the source folder's name as the unpack
    /// queue reports it.
    started: HashMap<String, Original>,
    /// The copies made for keyless originals, and the original each replaces
    /// once the copy is listed.
    removing: HashMap<PathBuf, String>,
}

impl Migrations {
    /// The original `id` in `source` (a folder the user loaded) is to move
    /// into a copy.
    pub(crate) fn start(&mut self, source: &str, id: &str, keyed: bool) {
        self.started.insert(
            source.to_string(),
            Original {
                id: id.to_string(),
                keyed,
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
                if !original.keyed {
                    self.removing.insert(copy.clone(), original.id);
                }
                Some(Next::Load(copy.clone()))
            }
            Err(_) => Some(Next::Disable(original.id)),
        }
    }

    /// The listing names `dir` as a loaded folder: if it is a keyless copy's
    /// folder, the original's id is returned once, to be removed.
    pub(crate) fn listed(&mut self, dir: &Path) -> Option<String> {
        self.removing.remove(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy() -> PathBuf {
        PathBuf::from("/profile/extensions/unpacked-a-0123abcd")
    }

    #[test]
    fn a_keyless_original_is_removed_once_its_copy_is_listed() {
        let mut migrations = Migrations::default();
        migrations.start("/users/src", "old", false);
        assert_eq!(
            migrations.finish("/users/src", &Ok(copy())),
            Some(Next::Load(copy()))
        );
        assert_eq!(migrations.listed(&copy()), Some("old".to_string()));
        assert_eq!(migrations.listed(&copy()), None);
    }

    #[test]
    fn a_copy_that_is_not_listed_takes_nothing_away() {
        let mut migrations = Migrations::default();
        migrations.start("/users/src", "old", false);
        migrations.finish("/users/src", &Ok(copy()));
        let elsewhere = PathBuf::from("/profile/extensions/unpacked-b-4567cdef");
        assert_eq!(migrations.listed(&elsewhere), None);
        assert_eq!(migrations.listed(&copy()), Some("old".to_string()));
    }

    #[test]
    fn a_keyed_original_keeps_its_id_and_nothing_is_removed() {
        let mut migrations = Migrations::default();
        migrations.start("/users/src", "keyed", true);
        assert_eq!(
            migrations.finish("/users/src", &Ok(copy())),
            Some(Next::Load(copy()))
        );
        assert_eq!(migrations.listed(&copy()), None);
    }

    #[test]
    fn a_failed_copy_switches_the_original_off_and_removes_nothing() {
        for keyed in [false, true] {
            let mut migrations = Migrations::default();
            migrations.start("/users/src", "old", keyed);
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
        migrations.start("/users/src", "old", false);
        migrations.finish("/users/src", &Ok(copy()));
        // Finished: the same source is not migrated again by a second outcome.
        assert_eq!(migrations.finish("/users/src", &Ok(copy())), None);
    }
}
