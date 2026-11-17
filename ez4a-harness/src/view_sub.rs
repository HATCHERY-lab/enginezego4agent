// engine zero for agents view subscription.

//! Tail publish for an already-loaded line list. No channel and no thread.

use crate::view::window;

/// Tail to publish when loaded lines change, or nothing when they do not.
///
/// Returns `view::window(next, limit).1` when `next` is longer than `previous`
/// or the last line differs. Equal slices yield an empty vec, as does any
/// other difference that leaves the length and the last line unchanged.
pub fn on_lines(previous: &[String], next: &[String], limit: usize) -> Vec<String> {
    if next.len() > previous.len() || previous.last() != next.last() {
        window(next, limit).1
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::on_lines;
    use crate::view::window;

    fn lines(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|text| (*text).to_owned()).collect()
    }

    #[test]
    fn equal_slices_publish_nothing() {
        let sample = lines(&["a", "b"]);
        assert_eq!(on_lines(&sample, &sample, 2), Vec::<String>::new());
        assert_eq!(on_lines(&[], &[], 4), Vec::<String>::new());
        assert_eq!(on_lines(&sample, &lines(&["a", "b"]), 1), Vec::<String>::new());
    }

    #[test]
    fn longer_next_publishes_the_window_tail() {
        let previous = lines(&["a", "b"]);
        let next = lines(&["a", "b", "c"]);
        assert_eq!(on_lines(&previous, &next, 2), window(&next, 2).1);
        assert_eq!(on_lines(&previous, &next, 2), lines(&["b", "c"]));
        assert_eq!(on_lines(&[], &next, 10), window(&next, 10).1);
        assert_eq!(on_lines(&previous, &next, 0), Vec::<String>::new());
    }

    #[test]
    fn changed_last_line_publishes_the_window_tail() {
        let previous = lines(&["a", "b", "c"]);
        let rewritten = lines(&["a", "b", "c2"]);
        assert_eq!(on_lines(&previous, &rewritten, 2), window(&rewritten, 2).1);
        assert_eq!(on_lines(&previous, &rewritten, 2), lines(&["b", "c2"]));

        let shorter = lines(&["z"]);
        assert_eq!(on_lines(&previous, &shorter, 4), window(&shorter, 4).1);
        assert_eq!(on_lines(&previous, &[], 3), window(&[], 3).1);
    }

    #[test]
    fn same_length_and_last_line_publish_nothing() {
        let previous = lines(&["a", "b", "c"]);
        let middle = lines(&["z", "y", "c"]);
        assert_eq!(on_lines(&previous, &middle, 3), Vec::<String>::new());
        let shorter_same_last = lines(&["x", "c"]);
        assert_eq!(
            on_lines(&previous, &shorter_same_last, 2),
            Vec::<String>::new()
        );
    }
}
