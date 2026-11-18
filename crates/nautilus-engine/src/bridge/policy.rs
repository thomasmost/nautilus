//! The bridge's decisions as pure functions, with no CEF types: which
//! relay pages to open and close, whether a queued job runs now, what a
//! worker's hello says about its debugger sessions, and what a delivery's
//! outcome means for another attempt. `Bridge` in `relay.rs` carries them
//! out, and the tests here pin them down.
//!
//! The rule for a debugger (review cycle 4): uncertainty never forgets. A
//! tracked debugger is forgotten only on positive evidence (`Evidence`); a
//! timeout, a failed evaluation, an unreachable worker or a relay that is
//! given up on closes the relay and keeps the debugger.

use std::time::Duration;

use osmena_core::TabId;
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

/// One worker session state a hello reports for one tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Session {
    /// The worker's own debugger session on the tab answers.
    Attached,
    /// Chromium says the worker has no session on the tab, or the tab does
    /// not exist (only these two answers are "not attached").
    NotAttached,
    /// The probe timed out or failed some other way.
    Unknown,
}

/// What a debugger's state rests on, for the forgetting that is not a worker's
/// own detach report or a tab's close (those are applied as they arrive, in
/// `relay::set_debugged`). Only positive evidence forgets one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Evidence {
    /// A live worker's hello said its session on the tab is not attached.
    WorkerSaysNotAttached,
    /// The extension is off or gone in the manager's list (`sync`).
    ExtensionGone,
    /// No positive evidence: a timeout, a failed evaluation, an unreachable
    /// worker, a relay that is given up on, a hello that says attached or
    /// unknown. The debugger is kept.
    Nothing,
}

/// Whether `evidence` forgets a debugger. Every caller that forgets one
/// asks this first, so the rule is the one place that decides.
pub(crate) fn forgets(evidence: Evidence) -> bool {
    !matches!(evidence, Evidence::Nothing)
}

/// The evidence a hello's state for one tab gives: only "not attached" is
/// positive.
pub(crate) fn session_evidence(state: Session) -> Evidence {
    match state {
        Session::NotAttached => Evidence::WorkerSaysNotAttached,
        Session::Attached | Session::Unknown => Evidence::Nothing,
    }
}

/// The tabs an extension's debuggers no longer cover, from its worker's
/// hello. `sessions` are the hello's tabs resolved to Nautilus tabs; a tab
/// the hello does not name, or names as `Attached` or `Unknown`, keeps its
/// debugger. `tracked` are the tabs the engine shows for the extension.
pub(crate) fn tabs_to_forget(sessions: &[(TabId, Session)], tracked: &[TabId]) -> Vec<TabId> {
    tracked
        .iter()
        .filter(|tab| {
            sessions
                .iter()
                .any(|(named, state)| named == *tab && forgets(session_evidence(*state)))
        })
        .copied()
        .collect()
}

/// Whether a worker is the instance the engine last saw for its extension,
/// by the instance id in its hello. A first hello is the same as nothing
/// seen before; a different id is a restarted worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Identity {
    /// The worker is the one the engine last saw, or the first it has seen.
    Same,
    /// Another worker instance: the worker was started again since.
    Restarted,
}

pub(crate) fn identity(seen: Option<&str>, reported: &str) -> Identity {
    match seen {
        Some(seen) if seen != reported => Identity::Restarted,
        _ => Identity::Same,
    }
}

/// Whether a queued job made by the worker instance `instance` is kept once
/// the extension's worker is `current`. A tab list holds the call id of one
/// instance: answering it in another would resolve that instance's own call
/// with the same id, so it is dropped. A click is a delivery and is kept (a
/// new instance has no record of its id, so it runs once there). A debugger
/// report is kept whichever instance made it: the session it reports
/// survives a worker stop (measured), so the table still applies it.
pub(crate) fn job_kept(kind: JobKind, instance: &str, current: &str) -> bool {
    match kind {
        JobKind::Query => instance == current,
        JobKind::Click | JobKind::Debugger => true,
    }
}

/// Whether the answer to a call of `instance` is sent, given the worker
/// instance the engine knows as current (`None`: none yet). An answer to
/// another instance's call is never sent: that instance is gone and its
/// call ids mean nothing to its successor.
pub(crate) fn reply_due(instance: &str, current: Option<&str>) -> bool {
    current.is_none_or(|current| current == instance)
}

/// What an extension's worker looked like in the last table refresh,
/// against what was recorded for it (`worker_target`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerTarget {
    /// The first worker target the engine has seen for the extension.
    First,
    /// The same target as recorded.
    Same,
    /// A different target: the worker was reloaded or started anew. The
    /// relay that reaches the old one is closed.
    Changed,
    /// No worker target is listed (the worker is stopped when idle). The
    /// recorded target is kept, so that its return is compared with it.
    Absent,
}

/// Compare the worker target listed now (`listed`) with the one recorded
/// (`recorded`). An absent target is not a change: an idle stop removes the
/// listing and the worker returns under the same id.
pub(crate) fn worker_target(recorded: Option<&str>, listed: Option<&str>) -> WorkerTarget {
    match (recorded, listed) {
        (_, None) => WorkerTarget::Absent,
        (None, Some(_)) => WorkerTarget::First,
        (Some(recorded), Some(listed)) if recorded == listed => WorkerTarget::Same,
        (Some(_), Some(_)) => WorkerTarget::Changed,
    }
}

/// How many table refreshes a changed worker target waits for a new worker
/// instance to say hello through the relay (`after_change`) before the relay
/// is closed as a zombie.
pub(crate) const CHANGE_GRACE_REFRESHES: u32 = 2;

/// What a changed worker target does to its relay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterChange {
    /// Keep the relay and wait for the next refresh.
    Wait,
    /// A new worker instance reached the engine through this relay: the relay
    /// is connected to the live worker, so it is kept and its calls in flight
    /// on its port are not failed.
    Keep,
    /// No new instance reached the engine through the relay within the grace:
    /// the relay's page cannot reach the new worker (a reloaded extension's
    /// relay is a zombie), so it is closed and opened again.
    Close,
}

/// What a changed worker target does to its relay. `heard`: a new worker
/// instance said hello through the relay since the last refresh. `waited`:
/// refreshes since the change, the change's own refresh counting as 0.
pub(crate) fn after_change(heard: bool, waited: u32) -> AfterChange {
    if heard {
        AfterChange::Keep
    } else if waited >= CHANGE_GRACE_REFRESHES {
        AfterChange::Close
    } else {
        AfterChange::Wait
    }
}

/// How often a table refresh is asked for while a relay is open, so that a
/// reloaded worker is seen within a bounded time even with no tab activity.
/// A reload leaves the relay page alive with no port event (measured:
/// `run.sh reload`); the target list is the one thing that shows it.
pub(crate) const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// Whether a check that was last made `since_last` ago (`None`: never) is due,
/// given the interval between two checks.
pub(crate) fn check_due(since_last: Option<Duration>, interval: Duration) -> bool {
    since_last.is_none_or(|elapsed| elapsed >= interval)
}

/// How many openings of one hidden page may fail in a row before the engine
/// stops opening it (`Opening::GiveUp`).
pub(crate) const MAX_OPENINGS: u32 = 5;
/// The gap after the first opening, and after a success: the schedule.
pub(crate) const BASE_GAP: Duration = Duration::from_millis(250);

/// What the engine does about a hidden page that is wanted and not open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opening {
    /// Open it now.
    Now,
    /// Wait this long more before opening it.
    Wait(Duration),
    /// Stop: it has failed `MAX_OPENINGS` times in a row. Nothing opens it
    /// again until its extension changes.
    GiveUp,
}

/// The gap that must pass after an opening before the next one, given how
/// many openings have failed in a row (`failed`, the count the last opening
/// is part of). 250 ms after a success or a first failure, then doubling:
/// 250, 500, 1000 and 2000 ms before the fifth opening. The gap is bounded by
/// `MAX_OPENINGS`, which ends the schedule before the gap grows further.
pub(crate) fn gap_after(failed: u32) -> Duration {
    BASE_GAP * 2u32.pow(failed.saturating_sub(1))
}

/// The next opening, given how many openings have failed in a row (`failed`)
/// and the time since the last opening (`None` when it was never opened). The
/// time is an argument, so the schedule is tested without a clock.
pub(crate) fn next_opening(failed: u32, since_last: Option<Duration>) -> Opening {
    if failed >= MAX_OPENINGS {
        return Opening::GiveUp;
    }
    let gap = gap_after(failed);
    match since_last {
        Some(elapsed) if elapsed < gap => Opening::Wait(gap - elapsed),
        _ => Opening::Now,
    }
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

    #[test]
    fn the_worker_is_the_same_while_its_instance_is_reported_unchanged() {
        assert_eq!(identity(Some("a"), "a"), Identity::Same);
        assert_eq!(identity(None, "a"), Identity::Same, "the first contact");
        assert_eq!(identity(Some("a"), "b"), Identity::Restarted);
    }

    #[test]
    fn only_a_not_attached_answer_forgets_a_tab() {
        // The three states of a hello, for the tab the engine tracks (1).
        let tracked = [TabId(1)];
        assert_eq!(
            tabs_to_forget(&[(TabId(1), Session::NotAttached)], &tracked),
            vec![TabId(1)],
            "Chromium said the worker has no session on it"
        );
        assert_eq!(
            tabs_to_forget(&[(TabId(1), Session::Attached)], &tracked),
            Vec::<TabId>::new()
        );
        assert_eq!(
            tabs_to_forget(&[(TabId(1), Session::Unknown)], &tracked),
            Vec::<TabId>::new(),
            "a timeout or another error keeps the debugger"
        );
    }

    #[test]
    fn a_tab_the_hello_does_not_name_keeps_its_debugger() {
        assert_eq!(
            tabs_to_forget(&[(TabId(2), Session::NotAttached)], &[TabId(1)]),
            Vec::<TabId>::new(),
            "no answer about tab 1, so no evidence"
        );
        assert_eq!(tabs_to_forget(&[], &[TabId(1)]), Vec::<TabId>::new());
    }

    #[test]
    fn a_restarted_worker_forgets_only_the_tabs_it_says_have_no_session() {
        // A worker that stopped and started again keeps its sessions (run
        // `stop`); a reloaded one has none (run `reload`).
        let tracked = [TabId(1), TabId(2)];
        assert_eq!(
            tabs_to_forget(
                &[
                    (TabId(1), Session::Attached),
                    (TabId(2), Session::NotAttached)
                ],
                &tracked
            ),
            vec![TabId(2)]
        );
        assert_eq!(
            tabs_to_forget(
                &[
                    (TabId(1), Session::NotAttached),
                    (TabId(2), Session::NotAttached)
                ],
                &tracked
            ),
            vec![TabId(1), TabId(2)],
            "an unload ended every session"
        );
    }

    #[test]
    fn only_positive_evidence_forgets_a_debugger() {
        assert!(forgets(Evidence::WorkerSaysNotAttached));
        assert!(forgets(Evidence::ExtensionGone));
        assert!(
            !forgets(Evidence::Nothing),
            "a timeout, a failed evaluation, an unreachable worker or a given-up relay keeps it"
        );
        assert_eq!(
            session_evidence(Session::Unknown),
            Evidence::Nothing,
            "an unknown probe is not evidence"
        );
    }

    #[test]
    fn a_click_is_carried_out_by_whichever_instance_is_current() {
        assert!(job_kept(JobKind::Click, "old", "new"));
    }

    #[test]
    fn a_tab_list_is_answered_only_by_the_instance_that_made_it() {
        assert!(job_kept(JobKind::Query, "a", "a"));
        assert!(!job_kept(JobKind::Query, "a", "b"));
    }

    #[test]
    fn a_debugger_report_of_an_old_instance_is_applied_but_not_answered() {
        // Its session survives the worker's stop, so the table still applies
        // it; its answer goes to an instance that no longer exists.
        assert!(job_kept(JobKind::Debugger, "a", "b"));
        assert!(!reply_due("a", Some("b")));
        assert!(reply_due("a", Some("a")));
        assert!(
            reply_due("a", None),
            "no instance known yet: answer the caller"
        );
    }

    #[test]
    fn a_relay_that_a_new_worker_instance_reached_is_kept_across_a_worker_target_change() {
        // A stopped worker restarted by a delivery: its new instance says hello
        // through the relay, so the relay is connected to it. Closing it would
        // fail the calls in flight on its port.
        assert_eq!(after_change(true, 0), AfterChange::Keep);
        assert_eq!(
            after_change(true, CHANGE_GRACE_REFRESHES + 5),
            AfterChange::Keep
        );
    }

    #[test]
    fn a_relay_no_new_instance_reaches_is_closed_only_after_the_grace() {
        // A reloaded extension's relay page: the new worker cannot reach it.
        assert_eq!(after_change(false, 0), AfterChange::Wait);
        assert_eq!(
            after_change(false, CHANGE_GRACE_REFRESHES - 1),
            AfterChange::Wait
        );
        assert_eq!(
            after_change(false, CHANGE_GRACE_REFRESHES),
            AfterChange::Close
        );
    }

    #[test]
    fn the_worker_target_changes_only_when_a_different_one_is_listed() {
        assert_eq!(worker_target(None, Some("w1")), WorkerTarget::First);
        assert_eq!(worker_target(Some("w1"), Some("w1")), WorkerTarget::Same);
        assert_eq!(
            worker_target(Some("w1"), Some("w2")),
            WorkerTarget::Changed,
            "a reloaded worker has a new target"
        );
    }

    #[test]
    fn an_idle_stop_is_not_a_change_and_its_return_is_compared_with_the_recorded_target() {
        assert_eq!(
            worker_target(Some("w1"), None),
            WorkerTarget::Absent,
            "the worker is listed as absent when idle"
        );
        // The engine keeps `w1` recorded through the absence, so a return
        // under the same id is the same worker and under a new one a change.
        assert_eq!(worker_target(Some("w1"), Some("w1")), WorkerTarget::Same);
        assert_eq!(worker_target(Some("w1"), Some("w2")), WorkerTarget::Changed);
        assert_eq!(worker_target(None, None), WorkerTarget::Absent);
    }

    #[test]
    fn a_refresh_is_due_when_never_made_or_when_its_interval_has_passed() {
        assert!(check_due(None, REFRESH_INTERVAL), "never refreshed");
        assert!(!check_due(
            Some(Duration::from_millis(1999)),
            REFRESH_INTERVAL
        ));
        assert!(check_due(Some(REFRESH_INTERVAL), REFRESH_INTERVAL));
    }

    #[test]
    fn the_first_opening_is_immediate_and_then_the_gaps_are_250_250_500_1000_2000_ms() {
        assert_eq!(next_opening(0, None), Opening::Now, "the first opening");
        assert_eq!(
            next_opening(0, Some(Duration::ZERO)),
            Opening::Wait(Duration::from_millis(250)),
            "after a success the minimum gap still holds (M19)"
        );
        assert_eq!(
            next_opening(1, Some(Duration::ZERO)),
            Opening::Wait(Duration::from_millis(250))
        );
        assert_eq!(
            next_opening(1, Some(Duration::from_millis(250))),
            Opening::Now
        );
        assert_eq!(
            next_opening(2, Some(Duration::ZERO)),
            Opening::Wait(Duration::from_millis(500))
        );
        assert_eq!(
            next_opening(3, Some(Duration::ZERO)),
            Opening::Wait(Duration::from_secs(1))
        );
        assert_eq!(
            next_opening(4, Some(Duration::ZERO)),
            Opening::Wait(Duration::from_secs(2)),
            "the longest gap is 2 s, reached before the fifth opening"
        );
        assert_eq!(
            next_opening(MAX_OPENINGS, Some(Duration::from_secs(60))),
            Opening::GiveUp
        );
        assert_eq!(next_opening(MAX_OPENINGS + 3, None), Opening::GiveUp);
    }

    #[test]
    fn a_page_that_never_polls_is_opened_five_times_on_the_schedule_and_then_given_up() {
        // Simulate the engine's turns: a page that opens, fails and is closed
        // each time, never polling successfully.
        let mut now = Duration::ZERO;
        let mut last: Option<Duration> = None;
        let mut failed = 0;
        let mut openings = Vec::new();
        loop {
            match next_opening(failed, last.map(|at| now - at)) {
                Opening::Now => {
                    openings.push(now.as_millis());
                    last = Some(now);
                    failed += 1;
                }
                Opening::Wait(more) => now += more,
                Opening::GiveUp => break,
            }
        }
        assert_eq!(openings, vec![0, 250, 750, 1750, 3750]);
        assert_eq!(openings.len() as u32, MAX_OPENINGS);
    }
}
