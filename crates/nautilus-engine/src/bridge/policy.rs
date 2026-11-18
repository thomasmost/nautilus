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
    /// The page is gone: the job cannot be carried out.
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

/// What `job_turn` does with a job whose page is `page`: `None` when there
/// is no such page, `Some(ready)` otherwise.
pub(crate) fn job_turn(page: Option<bool>) -> JobTurn {
    match page {
        None => JobTurn::Drop,
        Some(false) => JobTurn::Wait,
        Some(true) => JobTurn::Run,
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
        assert_eq!(job_turn(Some(true)), JobTurn::Run);
        assert_eq!(job_turn(Some(false)), JobTurn::Wait);
        assert_eq!(job_turn(None), JobTurn::Drop);
    }
}
