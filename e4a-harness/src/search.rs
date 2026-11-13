// Portions derived from pi-durable 1.5.2, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Line search.
//!
//! Upstream ranks tools with BM25 for code mode. That ranking is not copied.
//! This port finds the first line that contains a needle.

/// 0-based index of the first line that contains `needle`.
///
/// Lines split on `\n` and `\r\n`, as `str::lines`. An empty needle is `None`.
pub fn find_line(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack.lines().position(|line| line.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_first_line_and_rejects_an_empty_needle() {
        assert_eq!(find_line("alpha\nbeta\ngamma", "bet"), Some(1));
        assert_eq!(find_line("alpha\r\nbeta", "alpha"), Some(0));
        assert_eq!(find_line("alpha\nbeta", "nope"), None);
        assert_eq!(find_line("alpha\nbeta", ""), None);
        assert_eq!(find_line("", "a"), None);
        assert_eq!(find_line("alpha\nbeta\nalpha", "alpha"), Some(0));
    }
}
