// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/compaction.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/compaction, MIT, see LICENSES/pi-MIT.txt.

//! Choose the kept tail of a conversation and serialize lines for a summary.

/// Index of the first message kept. Messages at and after it stay.
///
/// Walks back from the tail until `keep_recent_tokens` are kept, including the
/// message that crosses the budget. `None` when the list is empty, when the
/// total fits in the budget, or when the walk only reaches the first message
/// and there is nothing older to cut. One message larger than the budget still
/// returns `Some(0)`: a non-empty list is never cut down to zero messages.
pub fn select_cut(token_counts: &[u64], keep_recent_tokens: u64) -> Option<usize> {
    if token_counts.is_empty() || list_fits(token_counts, keep_recent_tokens) {
        return None;
    }
    let mut kept = 0u64;
    for index in (0..token_counts.len()).rev() {
        kept = kept.saturating_add(token_counts[index]);
        if kept < keep_recent_tokens {
            continue;
        }
        if index > 0 {
            return Some(index);
        }
        if token_counts.len() == 1 && token_counts[0] > keep_recent_tokens {
            return Some(0);
        }
        return None;
    }
    None
}

/// Saturating sum of per-message token counts.
pub fn estimate_context(token_counts: &[u64]) -> u64 {
    token_counts.iter().copied().fold(0, u64::saturating_add)
}

/// Join transcript lines with one blank line between them.
pub fn serialize_conversation(lines: &[String]) -> String {
    lines.join("\n\n")
}

/// `true` when the exact sum is within `budget`. Overflow does not fit.
fn list_fits(token_counts: &[u64], budget: u64) -> bool {
    let mut total = 0u64;
    for count in token_counts {
        match total.checked_add(*count) {
            Some(sum) => total = sum,
            None => return false,
        }
    }
    total <= budget
}

#[cfg(test)]
mod tests {
    use super::{estimate_context, select_cut, serialize_conversation};

    #[test]
    fn empty_or_fitting_list_has_no_cut() {
        assert_eq!(select_cut(&[], 10), None);
        assert_eq!(select_cut(&[1, 2, 3], 6), None);
        assert_eq!(select_cut(&[1, 2, 3], 100), None);
        assert_eq!(select_cut(&[0, 0], 0), None);
        assert_eq!(select_cut(&[50], 50), None);
    }

    #[test]
    fn cut_keeps_the_suffix_that_reaches_the_budget() {
        assert_eq!(select_cut(&[10, 20, 30, 40], 50), Some(2));
        assert_eq!(select_cut(&[10, 20, 30, 40], 40), Some(3));
        assert_eq!(select_cut(&[10, 20, 30, 40], 100), None);
        assert_eq!(select_cut(&[10, 41], 50), None);
    }

    #[test]
    fn oversized_tail_keeps_the_last_index() {
        assert_eq!(select_cut(&[5, 100], 10), Some(1));
        assert_eq!(select_cut(&[100], 10), Some(0));
        assert_eq!(select_cut(&[1], 0), Some(0));
        assert_eq!(select_cut(&[200, 10], 100), None);
    }

    #[test]
    fn estimate_saturates_and_serialize_joins_on_a_blank_line() {
        assert_eq!(estimate_context(&[]), 0);
        assert_eq!(estimate_context(&[1, 2, 3]), 6);
        assert_eq!(estimate_context(&[u64::MAX, 1]), u64::MAX);
        assert_eq!(serialize_conversation(&[]), "");
        assert_eq!(serialize_conversation(&["only".into()]), "only");
        assert_eq!(
            serialize_conversation(&["a".into(), "b".into()]),
            "a\n\nb"
        );
    }
}
