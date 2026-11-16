// Portions derived from pi-durable 1.5.2 (packages/durable/src/session/conversation-tails.ts, packages/durable/src/session/keys.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 session/conversation-tails and session/keys, MIT, see LICENSES/pi-MIT.txt.

//! Newest entry id kept for a conversation, and `{prefix}-{n}` record keys.
//! Pure: no storage, no I/O.

use crate::Error;

/// The greatest id in `ids`. `None` when `ids` is empty.
pub fn tail_entry_id(ids: &[i64]) -> Option<i64> {
    ids.iter().copied().max()
}

/// Keeps the greater of `current` and `entry_id`. A negative `entry_id` is ignored.
pub fn advance_tail(current: Option<i64>, entry_id: i64) -> Option<i64> {
    if entry_id < 0 {
        return current;
    }
    match current {
        Some(id) if id > entry_id => Some(id),
        _ => Some(entry_id),
    }
}

/// `{prefix}-{n}`. An empty `prefix` is an error.
pub fn mint_key(prefix: &str, n: u64) -> Result<String, Error> {
    if prefix.is_empty() {
        return Err(Error::message("key prefix is empty"));
    }
    Ok(format!("{prefix}-{n}"))
}

/// Split `text` on the last `-` into a prefix and an integer suffix.
///
/// No `-`, or a suffix that is not an integer, is `None`.
pub fn parse_key(text: &str) -> Option<(String, u64)> {
    let (prefix, suffix) = text.rsplit_once('-')?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let n = suffix.parse().ok()?;
    Some((prefix.to_owned(), n))
}

#[cfg(test)]
mod tests {
    use super::{advance_tail, mint_key, parse_key, tail_entry_id};

    #[test]
    fn tail_entry_id_is_the_max() {
        assert_eq!(tail_entry_id(&[]), None);
        assert_eq!(tail_entry_id(&[7]), Some(7));
        assert_eq!(tail_entry_id(&[1, 9, 4]), Some(9));
        assert_eq!(tail_entry_id(&[-5, -1, -8]), Some(-1));
        assert_eq!(tail_entry_id(&[i64::MIN, 0, i64::MAX]), Some(i64::MAX));
    }

    #[test]
    fn advance_tail_keeps_the_greater_id() {
        assert_eq!(advance_tail(None, 3), Some(3));
        assert_eq!(advance_tail(None, 0), Some(0));
        assert_eq!(advance_tail(Some(4), 9), Some(9));
        assert_eq!(advance_tail(Some(9), 4), Some(9));
        assert_eq!(advance_tail(Some(9), 9), Some(9));
        assert_eq!(advance_tail(Some(-2), 0), Some(0));
        assert_eq!(advance_tail(None, -1), None);
        assert_eq!(advance_tail(Some(5), -1), Some(5));
        assert_eq!(advance_tail(Some(-4), -1), Some(-4));
        assert_eq!(advance_tail(Some(i64::MAX), i64::MAX), Some(i64::MAX));
        assert_eq!(advance_tail(None, i64::MIN), None);
    }

    #[test]
    fn mint_key_formats_prefix_and_number() {
        assert_eq!(mint_key("entry", 0).expect("prefix"), "entry-0");
        assert_eq!(mint_key("task", 42).expect("prefix"), "task-42");
        assert_eq!(
            mint_key("entry-append", u64::MAX).expect("prefix"),
            format!("entry-append-{}", u64::MAX)
        );
        assert!(mint_key("", 1).is_err());
    }

    #[test]
    fn parse_key_splits_on_the_last_dash() {
        assert_eq!(parse_key("task-1"), Some(("task".to_owned(), 1)));
        assert_eq!(
            parse_key("entry-append-42"),
            Some(("entry-append".to_owned(), 42))
        );
        assert_eq!(parse_key("n-0"), Some(("n".to_owned(), 0)));
        assert_eq!(parse_key("a-01"), Some(("a".to_owned(), 1)));
        assert_eq!(parse_key("-7"), Some(("".to_owned(), 7)));
        assert_eq!(
            parse_key(&format!("p-{}", u64::MAX)),
            Some(("p".to_owned(), u64::MAX))
        );
        assert_eq!(parse_key("nodash"), None);
        assert_eq!(parse_key(""), None);
        assert_eq!(parse_key("prefix-"), None);
        assert_eq!(parse_key("-"), None);
        assert_eq!(parse_key("a-b"), None);
        assert_eq!(parse_key("a-1.5"), None);
        assert_eq!(parse_key("a-+1"), None);
        assert_eq!(parse_key("a- 1"), None);
        assert_eq!(parse_key(&format!("p-{}", u64::MAX as u128 + 1)), None);
    }
}
