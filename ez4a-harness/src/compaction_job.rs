// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/compaction.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/compaction, MIT, see LICENSES/pi-MIT.txt.

//! Compaction threshold and the summary entry. No model call.

use serde_json::{json, Value};

use crate::Error;

/// Harness default `keepRecentTokens`.
const DEFAULT_KEEP_RECENT_TOKENS: u64 = 20_000;
/// Harness default `reserveTokens`. Zero is a valid reserve.
const DEFAULT_RESERVE_TOKENS: u64 = 16_384;
/// `floor(0.8 * reserveTokens)` at the default reserve: the summary cap when the model max is not lower.
const DEFAULT_MAX_SUMMARY_TOKENS: u64 = DEFAULT_RESERVE_TOKENS * 4 / 5;

const SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
const SUMMARY_SUFFIX: &str = "\n</summary>";

/// System prompt, then the format prompt. The transcript and any extra focus are not included.
const SUMMARY_INSTRUCTION: &str = concat!(
    "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.",
    "\n\n",
    r#"The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work. If the conversation starts with an earlier summary, preserve its information and fold the newer messages into it.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or "(none)" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages."#,
);

/// Thresholds for one compaction decision.
///
/// Defaults match the harness policy: enabled, 20_000 recent tokens, 16_384
/// reserve. `max_summary_tokens` is `floor(0.8 * reserveTokens)` at that reserve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub keep_recent_tokens: u64,
    pub reserve_tokens: u64,
    pub max_summary_tokens: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_recent_tokens: DEFAULT_KEEP_RECENT_TOKENS,
            reserve_tokens: DEFAULT_RESERVE_TOKENS,
            max_summary_tokens: DEFAULT_MAX_SUMMARY_TOKENS,
        }
    }
}

impl Settings {
    /// Token counts must be integers 0 or more. A negative count is an error.
    ///
    /// A reserve of 0 is valid.
    pub fn try_new(
        enabled: bool,
        keep_recent_tokens: i64,
        reserve_tokens: i64,
        max_summary_tokens: i64,
    ) -> Result<Self, Error> {
        Ok(Self {
            enabled,
            keep_recent_tokens: non_negative("keepRecentTokens", keep_recent_tokens)?,
            reserve_tokens: non_negative("reserveTokens", reserve_tokens)?,
            max_summary_tokens: non_negative("maxSummaryTokens", max_summary_tokens)?,
        })
    }
}

fn non_negative(field: &str, value: i64) -> Result<u64, Error> {
    match u64::try_from(value) {
        Ok(value) => Ok(value),
        Err(_) => Err(Error::message(format!(
            "compaction.{field} must be an integer 0 or more, got {value}"
        ))),
    }
}

/// Whether preparation should compact.
///
/// True when compaction is enabled, `context_limit` is not 0, and
/// `estimated_tokens + reserve_tokens` is at least `context_limit`. A sum that
/// overflows `u64` is treated as over every limit.
pub fn should_compact(estimated_tokens: u64, context_limit: u64, settings: &Settings) -> bool {
    if !settings.enabled || context_limit == 0 {
        return false;
    }
    estimated_tokens.saturating_add(settings.reserve_tokens) >= context_limit
}

/// Instruction text sent to the summarizer: the system prompt and the format prompt.
pub fn summary_instruction() -> &'static str {
    SUMMARY_INSTRUCTION
}

/// Journal object for a compaction summary.
///
/// `kind` is `pi.compaction`. `head` is `covered_until`. `model` is one user
/// message whose text is `summary` wrapped the way the harness stores it.
/// An empty or whitespace-only summary is an error. No model is called.
pub fn summary_entry(summary: &str, covered_until: i64) -> Result<Value, Error> {
    if summary.trim().is_empty() {
        return Err(Error::message("compaction summary is empty"));
    }
    let text = format!("{SUMMARY_PREFIX}{summary}{SUMMARY_SUFFIX}");
    Ok(json!({
        "kind": "pi.compaction",
        "head": covered_until,
        "model": [{
            "role": "user",
            "content": [{ "type": "text", "text": text }],
        }],
    }))
}

#[cfg(test)]
mod tests {
    use super::{should_compact, summary_entry, summary_instruction, Settings};
    use serde_json::json;

    #[test]
    fn defaults_match_the_harness_policy() {
        let settings = Settings::default();
        assert!(settings.enabled);
        assert_eq!(settings.keep_recent_tokens, 20_000);
        assert_eq!(settings.reserve_tokens, 16_384);
        assert_eq!(settings.max_summary_tokens, 13_107);
        assert_eq!(settings.max_summary_tokens, 16_384 * 4 / 5);
    }

    #[test]
    fn negative_token_counts_are_errors_and_zero_reserve_is_kept() {
        let zero = Settings::try_new(true, 0, 0, 0).unwrap();
        assert_eq!(zero.reserve_tokens, 0);
        assert_eq!(zero.keep_recent_tokens, 0);
        assert_eq!(zero.max_summary_tokens, 0);

        let keep = Settings::try_new(true, -1, 0, 0).unwrap_err();
        assert_eq!(
            keep.to_string(),
            "compaction.keepRecentTokens must be an integer 0 or more, got -1"
        );
        let reserve = Settings::try_new(false, 1, -16_384, 1).unwrap_err();
        assert_eq!(
            reserve.to_string(),
            "compaction.reserveTokens must be an integer 0 or more, got -16384"
        );
        let max = Settings::try_new(true, 1, 0, -2).unwrap_err();
        assert_eq!(
            max.to_string(),
            "compaction.maxSummaryTokens must be an integer 0 or more, got -2"
        );
    }

    #[test]
    fn compact_when_enabled_and_estimated_plus_reserve_reaches_the_limit() {
        let settings = Settings::default();
        assert!(should_compact(183_616, 200_000, &settings));
        assert!(!should_compact(183_615, 200_000, &settings));
        assert!(should_compact(200_000, 200_000, &settings));
        assert!(!should_compact(u64::MAX, 0, &settings));

        let mut off = settings;
        off.enabled = false;
        assert!(!should_compact(u64::MAX, 1, &off));
        assert!(!should_compact(0, 0, &off));

        let zero_reserve = Settings::try_new(true, 20_000, 0, 13_107).unwrap();
        assert!(should_compact(15, 15, &zero_reserve));
        assert!(!should_compact(14, 15, &zero_reserve));
        assert!(should_compact(u64::MAX, u64::MAX, &settings));
    }

    #[test]
    fn summary_instruction_copies_the_summarizer_prompts() {
        let text = summary_instruction();
        assert!(text.starts_with("You are a context summarization assistant."));
        assert!(text.contains(
            "Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary."
        ));
        assert!(text.contains(
            "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work."
        ));
        assert!(text.contains("## Goal"));
        assert!(text.contains("## Constraints & Preferences"));
        assert!(text.contains("## Key Decisions"));
        assert!(text.contains("## Next Steps"));
        assert!(text.contains("## Critical Context"));
        assert!(text.ends_with(
            "Keep each section concise. Preserve exact file paths, function names, and error messages."
        ));
    }

    #[test]
    fn summary_entry_wraps_the_text_and_rejects_an_empty_summary() {
        let entry = summary_entry("Kept the goal.", 42).unwrap();
        assert_eq!(entry["kind"], json!("pi.compaction"));
        assert_eq!(entry["head"], json!(42));
        assert_eq!(entry["model"][0]["role"], json!("user"));
        assert_eq!(
            entry["model"][0]["content"][0]["text"],
            json!(
                "The conversation history before this point was compacted into the following summary:\n\n<summary>\nKept the goal.\n</summary>"
            )
        );
        assert!(entry.get("data").is_none());
        assert_eq!(
            summary_entry("", 1).unwrap_err().to_string(),
            "compaction summary is empty"
        );
        assert!(summary_entry(" \n\t", 1).is_err());
        assert!(summary_entry("  x", 7).is_ok());
    }
}
