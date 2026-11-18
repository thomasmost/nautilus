//! The bridge's decisions as pure functions, with no CEF types: which
//! relay pages to open and close, whether a queued job runs now, and what
//! a delivery's outcome means for another attempt. `Bridge` in `relay.rs`
//! carries them out, and the tests here pin them down.

use serde_json::Value;

/// What a delivery's outcome means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    /// The worker took the delivery and it succeeded.
    Finished,
    /// The relay reports the worker never acknowledged the delivery, on
    /// the first attempt: open a fresh relay and send the same delivery
    /// id again, so a worker instance that did take it ignores it.
    Retry,
    /// No more attempts. `reopen` says the relay itself failed, so a fresh
    /// relay is wanted for the next click.
    Abandon { reopen: bool },
}

/// What a queued job does, given the state of the relay page it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum JobTurn {
    /// The page is ready: run the job now.
    Run,
    /// The page is still loading: keep the job.
    Wait,
    /// The page is gone and not wanted: the job cannot be carried out.
    Drop,
}

/// The relay pages to open and to close, given the open ones and the
/// bridged extensions that are wanted (`bridged`).
pub(crate) fn relay_changes(open: &[&str], bridged: &[String]) -> (Vec<String>, Vec<String>) {
    let to_open = bridged
        .iter()
        .filter(|id| !open.contains(&id.as_str()))
        .cloned()
        .collect();
    let to_close = open
        .iter()
        .filter(|id| !bridged.iter().any(|b| b == *id))
        .map(|id| id.to_string())
        .collect();
    (to_open, to_close)
}

/// What a queued job does, given its relay page: `page` is `None` when
/// there is no such page, `Some(ready)` otherwise. A relay that is gone
/// but wanted is reopened by the engine's next turn, so its job waits for
/// it instead of failing.
pub(crate) fn job_turn(page: Option<bool>, wanted: bool) -> JobTurn {
    match page {
        None if wanted => JobTurn::Wait,
        None => JobTurn::Drop,
        Some(false) => JobTurn::Wait,
        Some(true) => JobTurn::Run,
    }
}

/// What a queued job is, for the queue's rules. The job itself is in
/// `relay`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobKind {
    /// A tab list: answered with an error when it cannot be made.
    Query,
    /// A click: logged as dropped when it cannot be delivered.
    Click,
    /// A debugger report: the table decides it, and the relay only carries
    /// the answer. It is never failed by a page's loss.
    Debugger,
}

/// Which queued jobs fail when the page under `key` is gone. `is_helper`
/// says the page is the helper's, which every tab list and click needs;
/// otherwise the page is extension `key`'s relay. A debugger report is
/// never failed (it waits for the next table), so the worker's attach or
/// detach is still applied.
pub(crate) fn failed_by_loss(key: &str, is_helper: bool, jobs: &[(JobKind, &str)]) -> Vec<bool> {
    jobs.iter()
        .map(|(kind, extension)| *kind != JobKind::Debugger && (is_helper || *extension == key))
        .collect()
}

/// What the bound on the queue does with a queue of `kinds` (oldest
/// first) that holds at most `max` jobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overflow {
    /// The queue is within its bound.
    Fits,
    /// Fail the job at this index: the oldest that is not a debugger
    /// report.
    Fail(usize),
    /// Every queued job is a debugger report, which is never failed: apply
    /// the one at this index (the oldest) against the table now.
    Apply(usize),
}

pub(crate) fn overflow(kinds: &[JobKind], max: usize) -> Overflow {
    if kinds.len() <= max {
        return Overflow::Fits;
    }
    match kinds.iter().position(|kind| *kind != JobKind::Debugger) {
        Some(index) => Overflow::Fail(index),
        None => Overflow::Apply(0),
    }
}

/// Whether a callback from a page made under `generation` belongs to the
/// page now under its key, `current`. A page that is gone (`None`), or a
/// successor under a new generation, does not match.
pub(crate) fn is_generation(current: Option<u64>, generation: u64) -> bool {
    current == Some(generation)
}

/// The outcome of one delivery attempt: `Ok` is the relay's answer
/// `{ok, error?, received, attempts}`, `Err` a failed evaluation (the
/// relay page is gone or its call failed). `attempt` counts from 0.
///
/// An evaluation failure is never retried: the worker may have taken the
/// delivery before the call failed, and a new worker instance has no
/// record of its id. A relay that says the worker never acknowledged the
/// delivery is retried once, under the same id.
pub(crate) fn after_delivery(outcome: &Result<Value, String>, attempt: u8) -> Next {
    match outcome {
        Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(true) => Next::Finished,
        Ok(value) if value.get("received").and_then(Value::as_bool) == Some(true) => {
            Next::Abandon { reopen: false }
        }
        Ok(_) if attempt == 0 => Next::Retry,
        Ok(_) => Next::Abandon { reopen: true },
        Err(_) => Next::Abandon { reopen: true },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_delivery_that_succeeds_is_finished() {
        let ok = Ok(json!({"ok": true, "attempts": 1, "received": true}));
        assert_eq!(after_delivery(&ok, 0), Next::Finished);
        assert_eq!(after_delivery(&ok, 1), Next::Finished);
    }

    #[test]
    fn a_relay_that_never_heard_the_worker_is_retried_once_under_the_same_id() {
        let not_received =
            Ok(json!({"ok": false, "error": "the worker stopped", "received": false}));
        assert_eq!(after_delivery(&not_received, 0), Next::Retry);
        assert_eq!(
            after_delivery(&not_received, 1),
            Next::Abandon { reopen: true },
            "a second failure is abandoned, and its relay is replaced"
        );
    }

    #[test]
    fn a_worker_that_took_the_delivery_and_failed_is_not_sent_it_again() {
        let failed = Ok(json!({"ok": false, "error": "no action", "received": true}));
        assert_eq!(after_delivery(&failed, 0), Next::Abandon { reopen: false });
    }

    #[test]
    fn a_failed_evaluation_is_not_retried_because_the_worker_may_have_run_it() {
        // The relay's state is unknown: the worker may have taken the
        // delivery before the call failed, and a new worker instance has
        // no record of its id. The click is abandoned and the relay is
        // replaced so the next click has a live one.
        let gone: Result<Value, String> = Err("Execution context was destroyed.".to_string());
        assert_eq!(after_delivery(&gone, 0), Next::Abandon { reopen: true });
        assert_eq!(after_delivery(&gone, 1), Next::Abandon { reopen: true });
    }

    #[test]
    fn relay_changes_opens_the_wanted_relays_and_closes_the_rest() {
        let bridged = vec!["b".to_string(), "c".to_string()];
        assert_eq!(
            relay_changes(&["a", "b"], &bridged),
            (vec!["c".to_string()], vec!["a".to_string()])
        );
        assert_eq!(relay_changes(&[], &[]), (Vec::new(), Vec::new()));
        assert_eq!(
            relay_changes(&["b", "c"], &bridged),
            (Vec::new(), Vec::new()),
            "an open relay that is still wanted stays as it is"
        );
    }

    #[test]
    fn a_job_runs_when_its_page_is_ready_waits_while_it_loads_and_drops_without_one() {
        assert_eq!(job_turn(Some(true), true), JobTurn::Run);
        assert_eq!(job_turn(Some(false), true), JobTurn::Wait);
        assert_eq!(job_turn(None, false), JobTurn::Drop);
    }

    #[test]
    fn a_job_of_a_relay_that_is_gone_but_wanted_waits_for_the_reopening() {
        // The engine reopens a wanted relay on its next turn, so a retry's
        // click (queued just after its relay closed) must not be dropped.
        assert_eq!(job_turn(None, true), JobTurn::Wait);
    }

    #[test]
    fn a_relay_that_closed_underneath_is_listed_to_open_again() {
        let bridged = vec!["a".to_string()];
        assert_eq!(
            relay_changes(&[], &bridged),
            (vec!["a".to_string()], Vec::new()),
            "a wanted relay with no page is opened by the next turn"
        );
    }

    #[test]
    fn a_page_loss_fails_its_queries_and_clicks_and_never_a_debugger_report() {
        let jobs = [
            (JobKind::Query, "a"),
            (JobKind::Click, "a"),
            (JobKind::Debugger, "a"),
            (JobKind::Click, "b"),
        ];
        assert_eq!(
            failed_by_loss("a", false, &jobs),
            vec![true, true, false, false],
            "a relay's loss fails the jobs of that extension only"
        );
        assert_eq!(
            failed_by_loss("helper", true, &jobs),
            vec![true, true, false, true],
            "the helper's loss fails every tab list and click, and keeps the debugger report"
        );
    }

    #[test]
    fn a_full_queue_fails_the_oldest_job_that_is_not_a_debugger_report() {
        use JobKind::{Click, Debugger, Query};
        assert_eq!(overflow(&[Query, Click], 2), Overflow::Fits);
        assert_eq!(overflow(&[Debugger, Query, Click], 2), Overflow::Fail(1));
        assert_eq!(
            overflow(&[Debugger, Debugger, Click], 2),
            Overflow::Fail(2),
            "a debugger report at the front is kept"
        );
    }

    #[test]
    fn a_queue_of_debugger_reports_applies_the_oldest_rather_than_dropping_it() {
        use JobKind::Debugger;
        assert_eq!(
            overflow(&[Debugger, Debugger, Debugger], 2),
            Overflow::Apply(0)
        );
    }

    #[test]
    fn requeueing_past_the_bound_is_trimmed_to_it_one_job_at_a_time() {
        // The relay's requeue path: after a turn, the kept jobs and any new
        // ones are trimmed until the queue fits.
        use JobKind::{Click, Query};
        let mut kinds = vec![Query; 3];
        kinds.extend(vec![Click; 2]);
        let mut failed = Vec::new();
        while let Overflow::Fail(index) = overflow(&kinds, 4) {
            failed.push(kinds.remove(index));
        }
        assert_eq!(failed, vec![Query]);
        assert_eq!(kinds.len(), 4, "the queue is at its bound, not past it");
    }

    #[test]
    fn a_page_callback_belongs_to_the_page_only_under_its_own_generation() {
        assert!(is_generation(Some(7), 7));
        assert!(!is_generation(Some(8), 7), "a successor under the same key");
        assert!(!is_generation(None, 7), "a page that is gone");
    }
}
