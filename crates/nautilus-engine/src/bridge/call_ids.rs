//! The engine's ids for the calls of extensions' workers.

use std::collections::HashMap;

use super::transport::CallId;

/// The engine's ids for worker calls. A worker numbers its calls itself, so
/// two extensions' workers can both have a call 7, and a worker instance
/// that is replaced leaves calls whose answers its successor would ignore.
/// The hub is given a Bridge-local id per call (`issue`); this map gives back
/// the worker's (extension, instance, call) when the answer comes (`take`).
#[derive(Default)]
pub(crate) struct CallIds {
    last: CallId,
    calls: HashMap<CallId, (String, String, u64)>,
}

impl CallIds {
    /// The Bridge-local id of call `worker_call` of `instance` of `extension`.
    pub(crate) fn issue(&mut self, extension: &str, instance: &str, worker_call: u64) -> CallId {
        self.last += 1;
        self.calls.insert(
            self.last,
            (extension.to_string(), instance.to_string(), worker_call),
        );
        self.last
    }

    /// The triple of `call`, removed: an answer is sent once.
    pub(crate) fn take(&mut self, call: CallId) -> Option<(String, String, u64)> {
        self.calls.remove(&call)
    }

    /// Drop the calls of `extension`'s instances other than `current`, the
    /// instance a hello has just named. Returns how many were dropped.
    pub(crate) fn retire_instances(&mut self, extension: &str, current: &str) -> usize {
        let before = self.calls.len();
        self.calls
            .retain(|_, (e, instance, _)| e != extension || instance == current);
        before - self.calls.len()
    }
}

#[cfg(test)]
mod call_ids_tests {
    use super::CallIds;

    #[test]
    fn calls_of_two_workers_with_the_same_number_stay_apart() {
        let mut ids = CallIds::default();
        let a = ids.issue("aaaa", "i1", 7);
        let b = ids.issue("bbbb", "i2", 7);
        assert_ne!(a, b);
        assert_eq!(ids.take(b), Some(("bbbb".to_string(), "i2".to_string(), 7)));
        assert_eq!(ids.take(a), Some(("aaaa".to_string(), "i1".to_string(), 7)));
        assert_eq!(ids.take(a), None, "an answer is sent once");
    }

    #[test]
    fn a_new_worker_instance_retires_the_calls_of_the_old_one_and_no_others() {
        let mut ids = CallIds::default();
        let old_one = ids.issue("aaaa", "old", 1);
        let old_two = ids.issue("aaaa", "old", 2);
        let other = ids.issue("bbbb", "old", 3);
        let same = ids.issue("aaaa", "new", 4);
        assert_eq!(
            ids.retire_instances("aaaa", "new"),
            2,
            "the two calls of the old instance go"
        );
        assert_eq!(ids.take(old_one), None);
        assert_eq!(ids.take(old_two), None);
        assert_eq!(
            ids.take(other),
            Some(("bbbb".to_string(), "old".to_string(), 3)),
            "another extension's calls stay"
        );
        assert_eq!(
            ids.take(same),
            Some(("aaaa".to_string(), "new".to_string(), 4)),
            "the current instance's calls stay"
        );
        assert_eq!(
            ids.retire_instances("aaaa", "new"),
            0,
            "a hello of the same instance retires nothing"
        );
    }
}
