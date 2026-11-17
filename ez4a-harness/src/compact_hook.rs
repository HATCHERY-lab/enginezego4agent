// engine zero for agents compaction hook. No network client.

//! Compact after an answered turn when the estimate crosses the threshold.

use ez4a_llm::Llm;
use serde_json::Value;

use crate::compaction_job::{should_compact, Settings};
use crate::compaction_model::compact_model;
use crate::context_load::Line;

/// Summary entry when [`should_compact`] is true, otherwise `None`.
///
/// A false decision does not call the model. `covered_until` is the last
/// line's `entry_id`, or `0` when `lines` is empty. No network client is opened.
pub async fn compact_if_needed<L: Llm>(
    llm: &L,
    lines: &[Line],
    estimated_tokens: u64,
    context_limit: u64,
    settings: &Settings,
) -> Result<Option<Value>, String> {
    if !should_compact(estimated_tokens, context_limit, settings) {
        return Ok(None);
    }
    let covered_until = lines.last().map(|line| line.entry_id).unwrap_or(0);
    Ok(Some(compact_model(llm, lines, covered_until).await?))
}

#[cfg(test)]
mod tests {
    use super::compact_if_needed;
    use crate::compaction_job::{should_compact, summary_entry, Settings};
    use crate::context_load::Line;
    use ez4a_llm::FauxLlm;

    fn line(entry_id: i64, text: &str) -> Line {
        Line {
            entry_id,
            text: text.to_owned(),
        }
    }

    fn boundary(settings: &Settings, context_limit: u64) -> (u64, u64) {
        let at = context_limit.saturating_sub(settings.reserve_tokens);
        (at.saturating_sub(1), at)
    }

    #[tokio::test]
    async fn below_the_threshold_does_not_call_the_model() {
        let llm = FauxLlm::text("short turn");
        let settings = Settings::default();
        let context_limit = 200_000;
        let (below, at) = boundary(&settings, context_limit);
        assert!(!should_compact(below, context_limit, &settings));
        assert!(should_compact(at, context_limit, &settings));
        let got = compact_if_needed(
            &llm,
            &[line(4, "short turn")],
            below,
            context_limit,
            &settings,
        )
        .await
        .unwrap();
        assert!(got.is_none());
        assert_eq!(llm.calls(), 0);
    }

    #[tokio::test]
    async fn at_the_threshold_returns_a_summary_containing_the_faux_text() {
        let llm = FauxLlm::text("Kept the goal.");
        let settings = Settings::default();
        let context_limit = 200_000;
        let (_below, at) = boundary(&settings, context_limit);
        let lines = [line(3, "older"), line(9, "Kept the goal.")];
        let got = compact_if_needed(&llm, &lines, at, context_limit, &settings)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, summary_entry("Kept the goal.", 9).unwrap());
        assert!(got.to_string().contains("Kept the goal."));
        assert_eq!(llm.calls(), 1);
    }
}
