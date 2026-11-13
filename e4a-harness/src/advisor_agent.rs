// engine4agent advisor. Second session, read-only.

//! Advisor role for a second session.
//!
//! The reviewer only reads. It is not extra tools on the developer.

/// Identity, model, tools, and instructions for one agent session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentRole {
    pub id: &'static str,
    pub model: &'static str,
    pub tools: &'static [&'static str],
    pub instructions: &'static str,
}

const ADVISOR_INSTRUCTIONS: &str = "\
You review the workspace. You do not write, edit, or run commands. \
Quote the file path and the line you read. If you did not read it, say you do not know. \
Reasoning text is not stored as a durable entry.";

/// Read-only reviewer. The tool list is `read` only.
pub fn advisor_agent() -> AgentRole {
    AgentRole {
        id: "advisor",
        model: "cohere/north-mini-code:free",
        tools: &["read"],
        instructions: ADVISOR_INSTRUCTIONS,
    }
}

/// Model used when the advisor model is unavailable.
pub fn fallback_model() -> &'static str {
    "minimax/minimax-m3.1-flash"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_exactly_read() {
        assert_eq!(advisor_agent().tools, &["read"]);
    }
}
