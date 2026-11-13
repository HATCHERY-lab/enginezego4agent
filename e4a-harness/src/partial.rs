// Portions derived from pi-durable 1.5.2, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Partial assistant text.
//!
//! Upstream races an unstable tail: a delta that replaces that tail does not
//! advance the stable prefix. That racer is not copied. `push_delta` always
//! appends and sets `stable_until` to the new byte length.

/// Growing text and the byte index through which it is treated as stable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartialText {
    pub text: String,
    /// Byte index into `text`. After `push_delta`, this is `text.len()`.
    pub stable_until: usize,
}

/// Append `delta` and mark the whole string stable.
///
/// The upstream racer is not copied: a delta is never treated as a replacement
/// of an unstable tail. `stable_until` becomes the new byte length.
pub fn push_delta(part: &mut PartialText, delta: &str) {
    part.text.push_str(delta);
    part.stable_until = part.text.len();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_delta_appends_and_marks_the_new_length_stable() {
        let mut part = PartialText {
            text: "abc".to_owned(),
            stable_until: 1,
        };
        push_delta(&mut part, "dé");
        assert_eq!(part.text, "abcdé");
        assert_eq!(part.stable_until, part.text.len());
        assert_eq!(part.stable_until, 6);

        push_delta(&mut part, "");
        assert_eq!(part.text, "abcdé");
        assert_eq!(part.stable_until, 6);
    }
}
