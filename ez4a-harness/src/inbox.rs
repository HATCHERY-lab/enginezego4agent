// Portions derived from pi-durable 1.5.2 harness, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Queued submissions waiting for a boundary.
//!
//! `All` places every item. `OneAtATime` places at most the first.
//! A kind of `reset` is a queued reset. Withdrawal drains the queue.

/// One queued submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxItem {
    pub id: i64,
    pub kind: String,
    pub enqueued_at: i64,
}

/// How many queued items of one mode a boundary places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueMode {
    /// Every item, in the order given.
    All,
    /// At most the first item.
    OneAtATime,
}

/// Items a boundary places. `All` keeps every item in order. `OneAtATime` keeps at most the first.
pub fn prepare_boundary(items: &[InboxItem], mode: QueueMode) -> Vec<InboxItem> {
    match mode {
        QueueMode::All => items.to_vec(),
        QueueMode::OneAtATime => items.iter().take(1).cloned().collect(),
    }
}

/// Drain `items`. Each id is paired with `reason`.
pub fn withdraw(items: &mut Vec<InboxItem>, reason: &str) -> Vec<(i64, String)> {
    items
        .drain(..)
        .map(|item| (item.id, reason.to_owned()))
        .collect()
}

/// Whether any item kind is `reset`.
pub fn has_queued_reset(items: &[InboxItem]) -> bool {
    items.iter().any(|item| item.kind == "reset")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64, kind: &str) -> InboxItem {
        InboxItem {
            id,
            kind: kind.to_owned(),
            enqueued_at: id,
        }
    }

    #[test]
    fn all_keeps_order_and_one_at_a_time_keeps_the_first() {
        let items = vec![item(1, "steer"), item(2, "followUp"), item(3, "reset")];
        assert_eq!(prepare_boundary(&items, QueueMode::All), items);
        assert_eq!(
            prepare_boundary(&items, QueueMode::OneAtATime),
            vec![item(1, "steer")]
        );
        let only = vec![item(4, "steer")];
        assert_eq!(prepare_boundary(&only, QueueMode::OneAtATime), only);
        assert!(prepare_boundary(&[], QueueMode::All).is_empty());
        assert!(prepare_boundary(&[], QueueMode::OneAtATime).is_empty());
    }

    #[test]
    fn withdraw_drains_and_pairs_the_reason() {
        let mut items = vec![item(7, "steer"), item(8, "followUp")];
        let settled = withdraw(&mut items, "aborted");
        assert!(items.is_empty());
        assert_eq!(
            settled,
            vec![(7, "aborted".to_owned()), (8, "aborted".to_owned())]
        );
        assert!(withdraw(&mut items, "aborted").is_empty());
    }

    #[test]
    fn reset_kind_is_queued_reset() {
        assert!(!has_queued_reset(&[]));
        assert!(!has_queued_reset(&[item(1, "steer"), item(2, "Reset")]));
        assert!(has_queued_reset(&[item(1, "steer"), item(2, "reset")]));
    }
}
