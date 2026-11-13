// engine4agent context load and compaction run.

//! Lines kept inside an entry-id window. No model call.

use crate::context_bounds::{within, Bounds};

/// One loaded transcript line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub entry_id: i64,
    pub text: String,
}

/// Lines for which `within(entry_id, bounds)` is true, in the same order.
pub fn load_lines(lines: &[Line], bounds: Bounds) -> Vec<Line> {
    lines
        .iter()
        .filter(|line| within(line.entry_id, bounds))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{load_lines, Line};
    use crate::context_bounds::Bounds;

    fn line(entry_id: i64, text: &str) -> Line {
        Line {
            entry_id,
            text: text.to_owned(),
        }
    }

    fn bounds(from_entry: Option<i64>, to_entry: Option<i64>) -> Bounds {
        Bounds {
            from_entry,
            to_entry,
        }
    }

    #[test]
    fn keeps_ids_inside_the_window_in_order() {
        let lines = vec![
            line(1, "a"),
            line(10, "b"),
            line(15, "c"),
            line(21, "d"),
            line(10, "e"),
        ];
        assert_eq!(
            load_lines(&lines, bounds(Some(10), Some(20))),
            vec![line(10, "b"), line(15, "c"), line(10, "e")]
        );
        assert_eq!(
            load_lines(&lines, bounds(Some(10), Some(10))),
            vec![line(10, "b"), line(10, "e")]
        );
        assert_eq!(
            load_lines(&lines, bounds(Some(30), Some(40))),
            Vec::<Line>::new()
        );
    }

    #[test]
    fn open_ends_do_not_drop_that_side() {
        let lines = vec![line(1, "a"), line(5, "b"), line(9, "c")];
        assert_eq!(load_lines(&lines, bounds(None, None)), lines);
        assert_eq!(
            load_lines(&lines, bounds(None, Some(5))),
            vec![line(1, "a"), line(5, "b")]
        );
        assert_eq!(
            load_lines(&lines, bounds(Some(5), None)),
            vec![line(5, "b"), line(9, "c")]
        );
        assert_eq!(load_lines(&[], bounds(None, None)), Vec::<Line>::new());
        assert!(load_lines(&lines, bounds(Some(8), Some(4))).is_empty());
    }
}
