// Portions derived from pi-durable 1.5.2 (storage/scan, documents, ids, and entries), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Ascending id page. `after` is exclusive, the same cut as a scan cursor.

/// Ids strictly after `after`, at most `limit`.
///
/// `ids` is ascending. `after` drops that id and every id below it.
/// A zero `limit` is empty.
pub fn page(ids: &[i64], after: Option<i64>, limit: usize) -> Vec<i64> {
    if limit == 0 {
        return Vec::new();
    }
    let start = match after {
        Some(after) => ids.partition_point(|id| *id <= after),
        None => 0,
    };
    ids[start..].iter().copied().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::page;

    #[test]
    fn limit_zero_is_empty() {
        assert_eq!(page(&[1, 2, 3], None, 0), Vec::<i64>::new());
        assert_eq!(page(&[1, 2, 3], Some(1), 0), Vec::<i64>::new());
    }

    #[test]
    fn none_starts_at_the_beginning() {
        assert_eq!(page(&[-2, -1, 0, 1], None, 2), vec![-2, -1]);
        assert_eq!(page(&[], None, 5), Vec::<i64>::new());
    }

    #[test]
    fn after_drops_that_id_and_everything_below() {
        assert_eq!(page(&[1, 2, 3, 4, 5], Some(3), 10), vec![4, 5]);
        assert_eq!(page(&[1, 2, 3, 4, 5], Some(3), 1), vec![4]);
        assert_eq!(page(&[1, 2, 2, 3], Some(2), 10), vec![3]);
    }

    #[test]
    fn missing_after_still_drops_lower_or_equal_ids() {
        assert_eq!(page(&[1, 3, 5], Some(2), 10), vec![3, 5]);
        assert_eq!(page(&[1, 2], Some(9), 5), Vec::<i64>::new());
    }
}
