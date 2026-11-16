// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/view.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/view, MIT, see LICENSES/pi-MIT.txt.

//! Tail window over already-loaded lines. No store reads and no mounts.

/// Start index of the tail, and at most `limit` lines from the end.
///
/// `limit == 0` yields an empty tail and a start equal to `lines.len()`,
/// including when `lines` is empty. A limit past the front starts at `0`.
pub fn window(lines: &[String], limit: usize) -> (usize, Vec<String>) {
    let start = lines.len().saturating_sub(limit);
    (start, lines[start..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::window;

    fn lines(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|text| (*text).to_owned()).collect()
    }

    #[test]
    fn zero_limit_is_an_empty_tail_at_the_end() {
        let lines = lines(&["a", "b"]);
        assert_eq!(window(&lines, 0), (2, Vec::<String>::new()));
        assert_eq!(window(&[], 0), (0, Vec::<String>::new()));
    }

    #[test]
    fn tail_is_at_most_limit_lines() {
        let sample = lines(&["a", "b", "c", "d"]);
        assert_eq!(window(&sample, 2), (2, lines(&["c", "d"])));
        assert_eq!(window(&sample, 4), (0, sample.clone()));
        assert_eq!(window(&sample, 9), (0, sample));
        assert_eq!(window(&[], 3), (0, Vec::<String>::new()));
    }
}
