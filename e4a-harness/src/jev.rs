// engine4agent Jev decision. One shot, no tools, not the worker loop.
//
// JevAgentHarness (arXiv 2610.21412) puts a fast model in a proxy. It answers
// a closed set: continue, stop, compact, steer. It does not write or run code.
// Pi has steer, abort, and a compaction task, and no such proxy.

use serde_json::{json, Value};

use e4a_llm::ToolSchema;

use crate::overlay::{after_tool, Overlay};

/// Closed answer of the fast decider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JevDecision {
    Continue,
    Stop { reason: &'static str },
    Compact,
    /// Replace the worker's next step with this text. The worker does not run the tool.
    Steer { text: String },
}

/// What the decider is allowed to see. Not the worker transcript.
pub struct JevSnap<'a> {
    pub tool: &'a str,
    pub arguments: &'a Value,
    pub transcript_chars: usize,
}

/// Local gate. A model answer, when present, is parsed separately and does not
/// override a destructive stop.
pub fn decide(snap: JevSnap<'_>) -> JevDecision {
    match after_tool(snap.tool, snap.arguments, snap.transcript_chars) {
        Overlay::Stop { reason } => JevDecision::Stop { reason },
        Overlay::Compact => JevDecision::Compact,
        Overlay::Continue => JevDecision::Continue,
    }
}

/// Prompt for one completion. No tool list. The model must answer with one word.
pub fn prompt(snap: &JevSnap<'_>) -> String {
    format!(
        "You are a fast gate, not a coding agent. Reply with one word: continue, stop, or compact.\n\
tool={}\nargs={}\nchars={}",
        snap.tool,
        snap.arguments,
        snap.transcript_chars
    )
}

/// Parse a one-word gate answer. Unknown text is continue.
pub fn parse(answer: &str) -> JevDecision {
    let word = answer.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    match word.as_str() {
        "stop" | "abort" => JevDecision::Stop {
            reason: "advisor stopped the step",
        },
        "compact" => JevDecision::Compact,
        _ => JevDecision::Continue,
    }
}

/// Tools the gate may call. Not the developer set.
pub fn jev_tool_schemas() -> Vec<ToolSchema> {
    vec![
        tool(
            "read",
            "Read one UTF-8 file before you decide. One path only.",
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        ),
        tool(
            "stop",
            "Stop the worker. The tool will not run.",
            json!({"type":"object","properties":{"reason":{"type":"string"}},"required":["reason"]}),
        ),
        tool(
            "compact",
            "Ask for a summary before the next worker step.",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "steer",
            "Replace the worker step with a short instruction. The tool will not run.",
            json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
        ),
    ]
}

fn tool(name: &str, description: &str, parameters: Value) -> ToolSchema {
    ToolSchema {
        name: name.into(),
        description: description.into(),
        parameters,
    }
}

/// Steer is a phrase, not a word. `steer: ...` wins over parse.
pub fn parse_line(answer: &str) -> JevDecision {
    let trimmed = answer.trim();
    if let Some(rest) = trimmed.strip_prefix("steer:") {
        let text = rest.trim();
        if !text.is_empty() {
            return JevDecision::Steer { text: text.to_string() };
        }
    }
    parse(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn destructive_stop_beats_a_continue_word() {
        let args = json!({"command": "Remove-Item a.txt"});
        let snap = JevSnap { tool: "powershell", arguments: &args, transcript_chars: 10 };
        assert!(matches!(decide(snap), JevDecision::Stop { .. }));
        assert_eq!(parse("continue"), JevDecision::Continue);
        assert!(matches!(parse_line("steer: use edit"), JevDecision::Steer { .. }));
    }
}
