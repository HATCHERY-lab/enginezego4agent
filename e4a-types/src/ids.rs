// Portions derived from pi-durable 1.5.2 (storage/scan, documents, ids, and entries), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Document keys, the next minted id, and fork-cutoff visibility. No I/O.

/// `c/{conversation_id}/{kind}`, or `c/{conversation_id}/{kind}/t/{task_id}` when a task is set.
///
/// An empty `kind` is refused.
pub fn doc_key(conversation_id: i64, kind: &str, task_id: Option<i64>) -> Result<String, String> {
    if kind.is_empty() {
        return Err("kind must be a non-empty string".to_string());
    }
    Ok(match task_id {
        Some(task_id) => format!("c/{conversation_id}/{kind}/t/{task_id}"),
        None => format!("c/{conversation_id}/{kind}"),
    })
}

/// The id after `current`.
///
/// A negative `current` is refused. `i64::MAX` is refused; addition does not saturate.
pub fn next_id(current: i64) -> Result<i64, String> {
    if current < 0 {
        return Err("current id is negative".to_string());
    }
    current
        .checked_add(1)
        .ok_or_else(|| "ID space is exhausted".to_string())
}

/// Whether `entry_id` is inside a fork cutoff.
///
/// Visible when `parent_cutoff` is absent, or when `entry_id` is at or below it.
pub fn entry_visible(entry_id: i64, parent_cutoff: Option<i64>) -> bool {
    match parent_cutoff {
        None => true,
        Some(cutoff) => entry_id <= cutoff,
    }
}

#[cfg(test)]
mod tests {
    use super::{doc_key, entry_visible, next_id};

    #[test]
    fn doc_key_omits_task_until_one_is_set() {
        assert_eq!(doc_key(7, "note", None).as_deref(), Ok("c/7/note"));
        assert_eq!(
            doc_key(7, "note", Some(3)).as_deref(),
            Ok("c/7/note/t/3")
        );
    }

    #[test]
    fn doc_key_rejects_an_empty_kind() {
        assert_eq!(
            doc_key(1, "", None),
            Err("kind must be a non-empty string".to_string())
        );
        assert_eq!(
            doc_key(1, "", Some(2)),
            Err("kind must be a non-empty string".to_string())
        );
    }

    #[test]
    fn next_id_steps_and_refuses_negative_or_max() {
        assert_eq!(next_id(0), Ok(1));
        assert_eq!(next_id(5), Ok(6));
        assert_eq!(next_id(i64::MAX - 1), Ok(i64::MAX));
        assert_eq!(next_id(-1), Err("current id is negative".to_string()));
        assert_eq!(next_id(i64::MIN), Err("current id is negative".to_string()));
        assert_eq!(next_id(i64::MAX), Err("ID space is exhausted".to_string()));
    }

    #[test]
    fn entry_visible_includes_the_cutoff() {
        assert!(entry_visible(4, None));
        assert!(entry_visible(-1, None));
        assert!(entry_visible(4, Some(4)));
        assert!(entry_visible(3, Some(4)));
        assert!(!entry_visible(5, Some(4)));
    }
}
