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
    /// Whether the user had it on. The copy takes the same state.
    enabled: bool,
}

/// What the manager does with a copy once it is listed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Settled {
    /// The original to remove: a keyless copy has a new id, so the original's
    /// stored data goes with it.
    pub original: Option<String>,
    /// The state the copy must have: the original's.
    pub enabled: bool,
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
    /// The copies made, and what each one must settle to once it is listed.
    removing: HashMap<PathBuf, Settled>,
}

impl Migrations {
    /// The original `id` in `source` (a folder the user loaded) is to move
    /// into a copy.
    pub(crate) fn start(&mut self, source: &str, id: &str, keyed: bool, enabled: bool) {
        self.started.insert(
            source.to_string(),
            Original {
                id: id.to_string(),
                keyed,
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
                self.removing.insert(
                    copy.clone(),
                    Settled {
                        original: (!original.keyed).then(|| original.id.clone()),
                        enabled: original.enabled,
                    },
                );
                Some(Next::Load(copy.clone()))
            }
            Err(_) => Some(Next::Disable(original.id)),
        }
    }

    /// The listing names `dir` as a loaded folder: if it is a migrated copy's
    /// folder, what it must settle to is returned once.
    pub(crate) fn listed(&mut self, dir: &Path) -> Option<Settled> {
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
    fn a_keyless_original_is_removed_and_its_copy_takes_its_state_once_listed() {
        let mut migrations = Migrations::default();
        migrations.start("/users/src", "old", false, true);
        assert_eq!(
            migrations.finish("/users/src", &Ok(copy())),
            Some(Next::Load(copy()))
        );
        assert_eq!(
            migrations.listed(&copy()),
            Some(Settled {
                original: Some("old".to_string()),
                enabled: true,
            })
        );
        assert_eq!(migrations.listed(&copy()), None);
    }

    #[test]
    fn a_keyless_copy_of_a_switched_off_original_is_switched_off_too() {
        let mut migrations = Migrations::default();
        migrations.start("/users/src", "old", false, false);
        migrations.finish("/users/src", &Ok(copy()));
        assert_eq!(
            migrations.listed(&copy()),
            Some(Settled {
                original: Some("old".to_string()),
                enabled: false,
            })
        );
    }

    #[test]
    fn a_keyed_copy_keeps_the_id_so_only_its_state_is_settled() {
        for enabled in [true, false] {
            let mut migrations = Migrations::default();
            migrations.start("/users/src", "keyed", true, enabled);
            assert_eq!(
                migrations.finish("/users/src", &Ok(copy())),
                Some(Next::Load(copy()))
            );
            assert_eq!(
                migrations.listed(&copy()),
                Some(Settled {
                    original: None,
                    enabled,
                })
            );
        }
    }

    #[test]
    fn a_copy_that_is_not_listed_takes_nothing_away() {
        let mut migrations = Migrations::default();
        migrations.start("/users/src", "old", false, true);
        migrations.finish("/users/src", &Ok(copy()));
        let elsewhere = PathBuf::from("/profile/extensions/unpacked-b-4567cdef");
        assert_eq!(migrations.listed(&elsewhere), None);
        assert!(migrations.listed(&copy()).is_some());
    }

    #[test]
    fn a_failed_copy_switches_the_original_off_and_settles_nothing() {
        for keyed in [false, true] {
            let mut migrations = Migrations::default();
            migrations.start("/users/src", "old", keyed, true);
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
        migrations.start("/users/src", "old", false, true);
        migrations.finish("/users/src", &Ok(copy()));
        // Finished: the same source is not migrated again by a second outcome.
        assert_eq!(migrations.finish("/users/src", &Ok(copy())), None);
    }
}
