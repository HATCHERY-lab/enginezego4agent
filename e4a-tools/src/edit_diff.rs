// Portions derived from pi-durable 1.5.2 (packages/durable/src/tools/edit-diff.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 tools/edit-diff and tools/image, MIT, see LICENSES/pi-MIT.txt.

//! Multi-match text replacement.
//!
//! [`apply_hunks`] replaces every non-overlapping copy. The single-match `edit`
//! tool is unchanged.

/// Replace every non-overlapping occurrence of `old` with `new`.
///
/// Search resumes after each match, so an overlapping tail is not a second
/// match. An empty `old` is an error. Zero occurrences is an error that names
/// the count. A match whose replacement is identical to `old` still succeeds.
pub fn apply_hunks(original: &str, old: &str, new: &str) -> Result<String, String> {
    if old.is_empty() {
        return Err("old string is empty".to_string());
    }
    let mut result = String::with_capacity(original.len());
    let mut rest = original;
    let mut count = 0usize;
    while let Some(index) = rest.find(old) {
        result.push_str(&rest[..index]);
        result.push_str(new);
        rest = &rest[index + old.len()..];
        count = count.saturating_add(1);
    }
    if count == 0 {
        return Err(format!("old string matched {count} times"));
    }
    result.push_str(rest);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::apply_hunks;

    #[test]
    fn replaces_every_non_overlapping_occurrence() {
        assert_eq!(apply_hunks("ab ab ab", "ab", "x").unwrap(), "x x x");
        assert_eq!(apply_hunks("aaaa", "aa", "b").unwrap(), "bb");
        assert_eq!(apply_hunks("aaa", "aa", "b").unwrap(), "ba");
        assert_eq!(apply_hunks("a", "a", "aa").unwrap(), "aa");
        assert_eq!(apply_hunks("a b a", "a", "").unwrap(), " b ");
        assert_eq!(apply_hunks("same", "same", "same").unwrap(), "same");
    }

    #[test]
    fn empty_old_is_an_error() {
        let error = apply_hunks("abc", "", "x").unwrap_err();
        assert_eq!(error, "old string is empty");
    }

    #[test]
    fn zero_occurrences_names_the_count() {
        let error = apply_hunks("abc", "z", "x").unwrap_err();
        assert_eq!(error, "old string matched 0 times");
        let empty = apply_hunks("", "a", "b").unwrap_err();
        assert_eq!(empty, "old string matched 0 times");
    }
}
