// Portions derived from pi-durable 1.5.2, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Retry delay.
//!
//! Upstream arms host timers and declines background timers on Cloudflare
//! Workers. This port is the delay only: no timer is armed.

/// Milliseconds to wait before attempt `attempt`.
///
/// Attempt 0 is 200. Each later attempt doubles that, and the result is capped
/// at 5000: 200, 400, 800, 1600, 3200, then 5000.
pub fn backoff_ms(attempt: u32) -> u64 {
    // 200 << 5 is 6400, already past the cap, so later attempts stay at 5000.
    if attempt >= 5 {
        return 5_000;
    }
    (200u64 << attempt).min(5_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_from_200_and_caps_at_5000() {
        assert_eq!(backoff_ms(0), 200);
        assert_eq!(backoff_ms(1), 400);
        assert_eq!(backoff_ms(2), 800);
        assert_eq!(backoff_ms(3), 1_600);
        assert_eq!(backoff_ms(4), 3_200);
        assert_eq!(backoff_ms(5), 5_000);
        assert_eq!(backoff_ms(6), 5_000);
        assert_eq!(backoff_ms(u32::MAX), 5_000);
    }
}
