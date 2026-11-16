// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/context.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/context, MIT, see LICENSES/pi-MIT.txt.

//! Inclusive entry-id window. No store reads.

/// Requested entry span. `None` leaves that side open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub from_entry: Option<i64>,
    pub to_entry: Option<i64>,
}

/// Drop a `from_entry` older than `first` and a `to_entry` newer than `last`.
///
/// `None` for `first` or `last` leaves that side unchanged. When both ends
/// remain and `from_entry > to_entry`, the result is the single newest entry
/// `last` when `last` is set, and both ends cleared otherwise.
pub fn clamp_bounds(bounds: Bounds, first: Option<i64>, last: Option<i64>) -> Bounds {
    let from_entry = match (bounds.from_entry, first) {
        (Some(from), Some(first)) if from < first => None,
        (from, _) => from,
    };
    let to_entry = match (bounds.to_entry, last) {
        (Some(to), Some(last)) if to > last => None,
        (to, _) => to,
    };
    match (from_entry, to_entry) {
        (Some(from), Some(to)) if from > to => match last {
            Some(last) => Bounds {
                from_entry: Some(last),
                to_entry: Some(last),
            },
            None => Bounds {
                from_entry: None,
                to_entry: None,
            },
        },
        _ => Bounds {
            from_entry,
            to_entry,
        },
    }
}

/// `chars / 4`. Upstream context defines no divisor.
///
/// Empty input is 0. Any non-empty input is at least 1.
pub fn token_estimate(chars: usize) -> u64 {
    if chars == 0 {
        return 0;
    }
    u64::try_from(chars / 4).unwrap_or(u64::MAX).max(1)
}

/// `entry_id` lies in `bounds`, inclusive.
///
/// A missing end does not constrain that side.
pub fn within(entry_id: i64, bounds: Bounds) -> bool {
    if let Some(from) = bounds.from_entry {
        if entry_id < from {
            return false;
        }
    }
    if let Some(to) = bounds.to_entry {
        if entry_id > to {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{clamp_bounds, token_estimate, within, Bounds};

    fn bounds(from_entry: Option<i64>, to_entry: Option<i64>) -> Bounds {
        Bounds {
            from_entry,
            to_entry,
        }
    }

    #[test]
    fn clamp_drops_ends_outside_the_known_span() {
        let requested = bounds(Some(1), Some(100));
        assert_eq!(
            clamp_bounds(requested, Some(10), Some(50)),
            bounds(None, None)
        );
        assert_eq!(
            clamp_bounds(requested, None, Some(50)),
            bounds(Some(1), None)
        );
        assert_eq!(
            clamp_bounds(requested, Some(10), None),
            bounds(None, Some(100))
        );
        assert_eq!(clamp_bounds(requested, None, None), requested);
        assert_eq!(
            clamp_bounds(bounds(Some(10), Some(50)), Some(10), Some(50)),
            bounds(Some(10), Some(50))
        );
    }

    #[test]
    fn inverted_span_collapses_to_the_newest_entry_or_clears() {
        let inverted = bounds(Some(40), Some(10));
        assert_eq!(
            clamp_bounds(inverted, Some(1), Some(50)),
            bounds(Some(50), Some(50))
        );
        assert_eq!(clamp_bounds(inverted, None, None), bounds(None, None));
        assert_eq!(
            clamp_bounds(bounds(Some(8), Some(3)), Some(1), None),
            bounds(None, None)
        );
        assert_eq!(
            clamp_bounds(bounds(Some(20), Some(80)), None, Some(50)),
            bounds(Some(20), None)
        );
    }

    #[test]
    fn token_estimate_is_quarter_chars_with_a_floor_of_one() {
        assert_eq!(token_estimate(0), 0);
        assert_eq!(token_estimate(1), 1);
        assert_eq!(token_estimate(3), 1);
        assert_eq!(token_estimate(4), 1);
        assert_eq!(token_estimate(5), 1);
        assert_eq!(token_estimate(8), 2);
    }

    #[test]
    fn within_is_inclusive_and_open_ends_do_not_constrain() {
        let span = bounds(Some(10), Some(20));
        assert!(!within(9, span));
        assert!(within(10, span));
        assert!(within(20, span));
        assert!(!within(21, span));
        assert!(within(3, bounds(None, Some(3))));
        assert!(!within(4, bounds(None, Some(3))));
        assert!(within(3, bounds(Some(3), None)));
        assert!(!within(2, bounds(Some(3), None)));
        assert!(within(i64::MIN, bounds(None, None)));
        assert!(within(i64::MAX, bounds(None, None)));
        assert!(!within(5, bounds(Some(8), Some(4))));
    }
}
