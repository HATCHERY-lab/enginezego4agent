// Portions derived from pi-durable 1.5.2 harness, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Admission and kind-withdrawal of durable submissions.
//!
//! No store and no commit. The caller keeps the vec and applies the result.

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
/// `existing` is not modified.
pub fn admit(
    existing: &[Submission],
    draft_kind: &str,
    conversation_id: i64,
    next_id: i64,
) -> Result<Submission, String> {
    if draft_kind.is_empty() {
        return Err("submission kind is empty".to_owned());
    }
    if existing.iter().any(|item| item.id == next_id) {
        return Err(format!("duplicate submission id {next_id}"));
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

    fn submission(id: i64, kind: &str, conversation_id: i64) -> Submission {
        Submission {
            id,
            kind: kind.to_owned(),
            conversation_id,
        }
    }

    #[test]
    fn admit_rejects_an_empty_kind_and_a_duplicate_id() {
        let existing = vec![submission(4, "steer", 1)];
        assert_eq!(
            admit(&existing, "", 1, 5).unwrap_err(),
            "submission kind is empty"
        );
        assert_eq!(
            admit(&existing, "followUp", 9, 4).unwrap_err(),
            "duplicate submission id 4"
        );
        assert_eq!(existing, vec![submission(4, "steer", 1)]);
        assert_eq!(
            admit(&existing, "followUp", 9, 5).unwrap(),
            submission(5, "followUp", 9)
        );
        assert_eq!(
            admit(&[], "steer", 2, 1).unwrap(),
            submission(1, "steer", 2)
        );
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
