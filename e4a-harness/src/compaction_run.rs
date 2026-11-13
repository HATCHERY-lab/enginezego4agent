// engine4agent context load and compaction run.

//! Summary entry built from line texts. No model call.

use serde_json::Value;

use crate::compaction::serialize_conversation;
use crate::compaction_job::summary_entry;
use crate::context_load::Line;

/// Join line texts, pass that string to `summarize`, and return `summary_entry`.
///
/// Texts are joined with one blank line, the same way `serialize_conversation`
/// joins a transcript. An empty or whitespace-only summary is `summary_entry`'s
/// error, returned as text. No model is called.
pub fn compact_with(
    lines: &[Line],
    covered_until: i64,
    summarize: impl FnOnce(&str) -> String,
) -> Result<Value, String> {
    let texts: Vec<String> = lines.iter().map(|line| line.text.clone()).collect();
    let transcript = serialize_conversation(&texts);
    let summary = summarize(&transcript);
    summary_entry(&summary, covered_until).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::compact_with;
    use crate::compaction_job::summary_entry;
    use crate::context_load::Line;
    use serde_json::json;

    fn line(entry_id: i64, text: &str) -> Line {
        Line {
            entry_id,
            text: text.to_owned(),
        }
    }

    #[test]
    fn joins_texts_and_returns_the_summary_entry() {
        let lines = vec![line(1, "alpha"), line(2, "beta")];
        let mut seen = String::new();
        let entry = compact_with(&lines, 12, |text| {
            seen = text.to_owned();
            "Kept the goal.".to_owned()
        })
        .unwrap();
        assert_eq!(seen, "alpha\n\nbeta");
        assert_eq!(entry, summary_entry("Kept the goal.", 12).unwrap());
        assert_eq!(entry["kind"], json!("pi.compaction"));
        assert_eq!(entry["head"], json!(12));
    }

    #[test]
    fn empty_summary_is_the_summary_entry_error() {
        let err = compact_with(&[line(1, "alpha")], 3, |_| String::new()).unwrap_err();
        assert_eq!(err, summary_entry("", 3).unwrap_err().to_string());
        let blank = compact_with(&[line(1, "alpha")], 3, |_| " \n\t".to_owned()).unwrap_err();
        assert_eq!(blank, "compaction summary is empty");
    }

    #[test]
    fn a_single_line_is_not_given_a_separator() {
        let entry = compact_with(&[line(4, "only")], 4, |text| {
            assert_eq!(text, "only");
            "ok".to_owned()
        })
        .unwrap();
        assert_eq!(entry["head"], json!(4));
    }

    #[test]
    fn no_lines_are_an_empty_transcript() {
        let entry = compact_with(&[], 0, |text| {
            assert_eq!(text, "");
            "still".to_owned()
        })
        .unwrap();
        assert_eq!(entry["head"], json!(0));
    }
}
