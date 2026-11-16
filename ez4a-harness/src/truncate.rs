// Portions derived from pi-durable 1.5.2, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Character truncation.
//!
//! Upstream keeps whole lines under a line count and a byte budget. This port
//! cuts on a Unicode scalar boundary.

/// Keep the first `max_chars` Unicode scalars.
///
/// The cut is always on a char boundary. `max_chars == 0` yields an empty string.
/// A shorter input is returned whole.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_on_a_char_boundary_and_zero_is_empty() {
        assert_eq!(truncate_chars("héllo", 0), "");
        assert_eq!(truncate_chars("héllo", 1), "h");
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("héllo", 5), "héllo");
        assert_eq!(truncate_chars("héllo", 8), "héllo");
        assert_eq!(truncate_chars("", 3), "");
    }
}
