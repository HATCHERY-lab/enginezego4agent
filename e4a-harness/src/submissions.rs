// Portions derived from pi-durable 1.5.2 harness, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Admission and kind-withdrawal of durable submissions.
//!
//! No submission store and no commit. The caller keeps the vec and applies the
//! result. A rejection is stored on the waiter before [`admit`] returns.

use crate::Scheduler;

/// One admitted submission. `kind` is the draft kind stored with the id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub id: i64,
    pub kind: String,
    pub conversation_id: i64,
}

/// Admit `next_id` for `conversation_id`.
///
/// An empty `draft_kind` is rejected. An id already present in `existing` is rejected.
/// `existing` is not modified. Either rejection is stored on `waiters` under
/// `next_id` before this returns. [`Scheduler::take_wait_error`] polls it.
pub fn admit(
    waiters: &Scheduler,
    existing: &[Submission],
    draft_kind: &str,
    conversation_id: i64,
    next_id: i64,
) -> Result<Submission, String> {
    if draft_kind.is_empty() {
        let failure = "submission kind is empty".to_owned();
        waiters.record_wait_failure(next_id, failure.clone());
        return Err(failure);
    }
    if existing.iter().any(|item| item.id == next_id) {
        let failure = format!("duplicate submission id {next_id}");
        waiters.record_wait_failure(next_id, failure.clone());
        return Err(failure);
    }
    Ok(Submission {
        id: next_id,
        kind: draft_kind.to_owned(),
        conversation_id,
    })
}

/// Remove every item whose kind is `kind`.
///
/// Returned items keep the order they had. Items that stay keep theirs.
pub fn withdraw_kind(items: &mut Vec<Submission>, kind: &str) -> Vec<Submission> {
    let (withdrawn, kept): (Vec<Submission>, Vec<Submission>) = std::mem::take(items)
        .into_iter()
        .partition(|item| item.kind == kind);
    *items = kept;
    withdrawn
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::Scheduler;

    fn submission(id: i64, kind: &str, conversation_id: i64) -> Submission {
        Submission {
            id,
            kind: kind.to_owned(),
            conversation_id,
        }
    }

    #[test]
    fn admit_rejects_an_empty_kind_and_a_duplicate_id() {
        let waiters = Scheduler::new();
        let existing = vec![submission(4, "steer", 1)];
        assert_eq!(
            admit(&waiters, &existing, "", 1, 5).unwrap_err(),
            "submission kind is empty"
        );
        assert_eq!(
            admit(&waiters, &existing, "followUp", 9, 4).unwrap_err(),
            "duplicate submission id 4"
        );
        assert_eq!(existing, vec![submission(4, "steer", 1)]);
        assert_eq!(
            admit(&waiters, &existing, "followUp", 9, 5).unwrap(),
            submission(5, "followUp", 9)
        );
        assert_eq!(
            admit(&waiters, &[], "steer", 2, 1).unwrap(),
            submission(1, "steer", 2)
        );
    }

    #[test]
    fn failed_admit_keeps_its_string() {
        let waiters = Scheduler::new();
        drop(admit(&waiters, &[], "", 3, 9).unwrap_err());
        assert_eq!(
            admit(&waiters, &[], "steer", 3, 9).unwrap(),
            submission(9, "steer", 3)
        );
        assert_eq!(
            waiters.take_wait_error(9).as_deref(),
            Some("submission kind is empty")
        );
        let existing = vec![submission(4, "steer", 1)];
        drop(admit(&waiters, &existing, "followUp", 9, 4).unwrap_err());
        assert_eq!(
            waiters.take_wait_error(4).as_deref(),
            Some("duplicate submission id 4")
        );
        assert!(waiters.take_wait_error(9).is_none());
        assert!(waiters.take_wait_error(4).is_none());
    }

    #[test]
    fn withdraw_kind_keeps_the_order_of_what_stays() {
        let mut items = vec![
            submission(1, "steer", 3),
            submission(2, "reset", 3),
            submission(3, "followUp", 3),
            submission(4, "reset", 8),
        ];
        let withdrawn = withdraw_kind(&mut items, "reset");
        assert_eq!(
            withdrawn,
            vec![submission(2, "reset", 3), submission(4, "reset", 8)]
        );
        assert_eq!(
            items,
            vec![submission(1, "steer", 3), submission(3, "followUp", 3)]
        );
        assert!(withdraw_kind(&mut items, "reset").is_empty());
        assert_eq!(
            items,
            vec![submission(1, "steer", 3), submission(3, "followUp", 3)]
        );
        let all = withdraw_kind(&mut items, "steer");
        assert_eq!(all, vec![submission(1, "steer", 3)]);
        assert_eq!(items, vec![submission(3, "followUp", 3)]);
        let mut empty = Vec::new();
        assert!(withdraw_kind(&mut empty, "steer").is_empty());
    }
}
