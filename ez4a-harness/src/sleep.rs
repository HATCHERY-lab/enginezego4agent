// engine zero for agents remaining host protocols. No I/O.

//! Delay until a wake timestamp.
//!
//! The result is a millisecond count. No thread is parked.

/// Milliseconds from `now_ms` until `wake_ms`.
///
/// `wake_ms` before `now_ms` is an error. A difference that does not fit in
/// `i64` is an error. Nothing sleeps.
pub fn sleep_until(now_ms: i64, wake_ms: i64) -> Result<i64, String> {
    if wake_ms < now_ms {
        return Err("wake time is before now".to_owned());
    }
    wake_ms
        .checked_sub(now_ms)
        .ok_or_else(|| "sleep duration overflows i64".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_non_negative_gap_and_rejects_the_past() {
        assert_eq!(sleep_until(1_000, 1_500).unwrap(), 500);
        assert_eq!(sleep_until(1_000, 1_000).unwrap(), 0);
        assert_eq!(sleep_until(-20, -5).unwrap(), 15);
        assert_eq!(
            sleep_until(10, 9).unwrap_err(),
            "wake time is before now"
        );
        assert_eq!(
            sleep_until(i64::MIN, i64::MAX).unwrap_err(),
            "sleep duration overflows i64"
        );
    }
}
