// engine4agent context compressor. Separate session, no tools.

//! Context compressor role.
//!
//! A separate session from Pi compaction, on a cheaper model.
//! Thinking text is not part of the summary. This role has no workspace tools.

use crate::dev_agent::AgentRole;

/// Cheap fast fallback. Not the free window.
const COMPRESSOR_MODEL: &str = "minimax/minimax-m3.1-flash";

const COMPRESSOR_INSTRUCTIONS: &str = "\
Summarize the transcript into plain text. Omit system messages and thinking. \
Keep tool names and their results. Do not call tools. Do not continue the conversation.";

/// Compressor role: empty tool list, flash model, summary-only instructions.
pub fn compressor_agent() -> AgentRole {
    AgentRole {
        id: "compressor",
        model: COMPRESSOR_MODEL,
        tools: &[],
        instructions: COMPRESSOR_INSTRUCTIONS,
    }
}

/// Token cap for the plain-text summary.
pub fn summary_max_tokens() -> u32 {
    4096
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_empty_and_model_is_flash() {
        let role = compressor_agent();
        assert!(role.tools.is_empty());
        assert_eq!(role.model, "minimax/minimax-m3.1-flash");
    }
}
