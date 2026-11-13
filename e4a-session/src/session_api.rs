// Portions derived from pi-durable 1.5.2 (packages/durable/src/session/session.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 session/session, MIT, see LICENSES/pi-MIT.txt.

//! Entry visibility, the fork point, usage deltas, and message roles.
//! Pure: no storage, no I/O.

/// Whether `entry_id` is still shown.
///
/// Archived entries are hidden. Otherwise an absent `cutoff` keeps the entry,
/// and a cutoff keeps ids at or below it.
pub fn visible_entry(entry_id: i64, cutoff: Option<i64>, archived: bool) -> bool {
    if archived {
        return false;
    }
    match cutoff {
        None => true,
        Some(cutoff) => entry_id <= cutoff,
    }
}

/// The entry a parent fork points at.
///
/// `entry_id` of `0` or less is refused. The fork point is that entry, not its parent.
pub fn fork_parent(entry_id: i64) -> Result<i64, String> {
    if entry_id <= 0 {
        return Err("fork parent requires a positive entry id".to_string());
    }
    Ok(entry_id)
}

/// Non-negative `(input, output)` change from `before_*` to `after_*`.
///
/// An `after` smaller than its `before` is refused. A difference that does not
/// fit in `i64` is refused; subtraction does not wrap.
pub fn usage_delta(
    before_in: i64,
    before_out: i64,
    after_in: i64,
    after_out: i64,
) -> Result<(i64, i64), String> {
    let input = counter_delta(before_in, after_in, "input")?;
    let output = counter_delta(before_out, after_out, "output")?;
    Ok((input, output))
}

/// `user`, `assistant`, `tool`, and `system` name themselves. Every other kind is `unknown`.
pub fn role_of(kind: &str) -> &'static str {
    match kind {
        "user" => "user",
        "assistant" => "assistant",
        "tool" => "tool",
        "system" => "system",
        _ => "unknown",
    }
}

fn counter_delta(before: i64, after: i64, name: &str) -> Result<i64, String> {
    if after < before {
        return Err(format!("{name} usage decreased"));
    }
    after
        .checked_sub(before)
        .ok_or_else(|| format!("{name} usage delta overflows"))
}

#[cfg(test)]
mod tests {
    use super::{fork_parent, role_of, usage_delta, visible_entry};

    #[test]
    fn visible_entry_hides_archived_and_honors_cutoff() {
        assert!(!visible_entry(1, None, true));
        assert!(!visible_entry(1, Some(10), true));
        assert!(!visible_entry(100, Some(1), true));
        assert!(!visible_entry(i64::MIN, Some(i64::MAX), true));

        assert!(visible_entry(1, None, false));
        assert!(visible_entry(0, None, false));
        assert!(visible_entry(-4, None, false));
        assert!(visible_entry(i64::MIN, None, false));
        assert!(visible_entry(i64::MAX, None, false));

        assert!(visible_entry(4, Some(4), false));
        assert!(visible_entry(3, Some(4), false));
        assert!(!visible_entry(5, Some(4), false));
        assert!(visible_entry(i64::MIN, Some(i64::MIN), false));
        assert!(!visible_entry(i64::MIN + 1, Some(i64::MIN), false));
        assert!(!visible_entry(0, Some(-1), false));
        assert!(visible_entry(i64::MAX, Some(i64::MAX), false));
        assert!(!visible_entry(i64::MAX, Some(i64::MAX - 1), false));
    }

    #[test]
    fn fork_parent_is_the_entry_when_positive() {
        assert_eq!(fork_parent(1), Ok(1));
        assert_eq!(fork_parent(42), Ok(42));
        assert_eq!(fork_parent(i64::MAX), Ok(i64::MAX));
        let refused = "fork parent requires a positive entry id".to_string();
        assert_eq!(fork_parent(0), Err(refused.clone()));
        assert_eq!(fork_parent(-1), Err(refused.clone()));
        assert_eq!(fork_parent(i64::MIN), Err(refused));
    }

    #[test]
    fn usage_delta_is_the_non_negative_difference() {
        assert_eq!(usage_delta(0, 0, 0, 0), Ok((0, 0)));
        assert_eq!(usage_delta(1, 2, 4, 2), Ok((3, 0)));
        assert_eq!(usage_delta(1, 2, 1, 9), Ok((0, 7)));
        assert_eq!(usage_delta(-2, -3, 1, 0), Ok((3, 3)));
        assert_eq!(usage_delta(i64::MIN, 4, i64::MIN, 4), Ok((0, 0)));
        assert_eq!(
            usage_delta(i64::MAX - 3, i64::MAX - 1, i64::MAX, i64::MAX),
            Ok((3, 1))
        );

        assert_eq!(
            usage_delta(5, 5, 4, 9),
            Err("input usage decreased".to_string())
        );
        assert_eq!(
            usage_delta(5, 5, 5, 4),
            Err("output usage decreased".to_string())
        );
        assert_eq!(
            usage_delta(5, 5, 4, 4),
            Err("input usage decreased".to_string())
        );
        assert_eq!(
            usage_delta(0, 0, -1, -1),
            Err("input usage decreased".to_string())
        );
        assert_eq!(
            usage_delta(i64::MIN, 0, i64::MAX, 0),
            Err("input usage delta overflows".to_string())
        );
        assert_eq!(
            usage_delta(0, -1, 0, i64::MAX),
            Err("output usage delta overflows".to_string())
        );
        assert_eq!(
            usage_delta(i64::MIN, i64::MIN, -1, i64::MAX),
            Err("input usage delta overflows".to_string())
        );
    }

    #[test]
    fn role_of_keeps_the_four_roles() {
        assert_eq!(role_of("user"), "user");
        assert_eq!(role_of("assistant"), "assistant");
        assert_eq!(role_of("tool"), "tool");
        assert_eq!(role_of("system"), "system");
        assert_eq!(role_of("unknown"), "unknown");
        assert_eq!(role_of(""), "unknown");
        assert_eq!(role_of("User"), "unknown");
        assert_eq!(role_of("toolResult"), "unknown");
        assert_eq!(role_of(" system"), "unknown");
        assert_eq!(role_of("user "), "unknown");
    }
}
