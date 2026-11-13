// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/transcript.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/transcript, MIT, see LICENSES/pi-MIT.txt.

//! Lines a client can show, cut at an entry id, and rendered as plain text.

/// One visible transcript row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptLine {
    pub entry_id: i64,
    pub role: String,
    pub text: String,
}

/// Lines at or before `cutoff`, in the same order.
///
/// `None` keeps every line. A negative cutoff keeps none: entry ids are not a
/// range that can start below zero.
pub fn visible_lines(lines: &[TranscriptLine], cutoff: Option<i64>) -> Vec<TranscriptLine> {
    match cutoff {
        None => lines.to_vec(),
        Some(id) if id < 0 => Vec::new(),
        Some(id) => lines
            .iter()
            .filter(|line| line.entry_id <= id)
            .cloned()
            .collect(),
    }
}

/// Join `role: text` with newlines. An empty slice is an empty string.
pub fn render(lines: &[TranscriptLine]) -> String {
    lines
        .iter()
        .map(|line| format!("{}: {}", line.role, line.text))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{render, visible_lines, TranscriptLine};

    fn line(entry_id: i64, role: &str, text: &str) -> TranscriptLine {
        TranscriptLine {
            entry_id,
            role: role.to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn no_cutoff_keeps_every_line_in_order() {
        let lines = vec![line(3, "user", "a"), line(1, "assistant", "b")];
        assert_eq!(visible_lines(&lines, None), lines);
        assert_eq!(visible_lines(&[], None), Vec::<TranscriptLine>::new());
    }

    #[test]
    fn cutoff_keeps_ids_at_or_before_it() {
        let lines = vec![
            line(1, "user", "a"),
            line(4, "assistant", "b"),
            line(2, "user", "c"),
            line(2, "assistant", "d"),
        ];
        assert_eq!(
            visible_lines(&lines, Some(2)),
            vec![
                line(1, "user", "a"),
                line(2, "user", "c"),
                line(2, "assistant", "d"),
            ]
        );
        assert_eq!(visible_lines(&lines, Some(0)), Vec::<TranscriptLine>::new());
        assert_eq!(visible_lines(&lines, Some(i64::MAX)), lines);
    }

    #[test]
    fn negative_cutoff_is_empty() {
        let lines = vec![line(-5, "user", "a"), line(0, "assistant", "b")];
        assert_eq!(visible_lines(&lines, Some(-1)), Vec::<TranscriptLine>::new());
        assert_eq!(
            visible_lines(&lines, Some(i64::MIN)),
            Vec::<TranscriptLine>::new()
        );
        assert_eq!(visible_lines(&[], Some(-1)), Vec::<TranscriptLine>::new());
    }

    #[test]
    fn render_joins_role_and_text_without_a_trailing_newline() {
        assert_eq!(render(&[]), "");
        assert_eq!(render(&[line(1, "user", "hello")]), "user: hello");
        assert_eq!(
            render(&[line(1, "user", "hello"), line(2, "assistant", "world")]),
            "user: hello\nassistant: world"
        );
        assert_eq!(render(&[line(1, "", "")]), ": ");
    }
}
