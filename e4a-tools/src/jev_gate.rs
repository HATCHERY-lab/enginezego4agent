//! Jev's own loop. At most two model calls. Tools are read, stop, compact, steer.
//! The developer tool set is not offered.

use e4a_harness::{jev_tool_schemas, parse_line, JevDecision, JevSnap};
use e4a_llm::{ChatMessage, Llm, LlmEvent, TurnRequest};
use e4a_types::Json;

use crate::{Error, Workspace};

const ROUNDS: usize = 2;
const READ_CAP: usize = 500;

/// The worker asked `jev` a question. Same two-call cap and the same tools.
pub async fn run_jev_ask<L: Llm>(
    llm: &L,
    workspace: &Workspace,
    question: &str,
) -> Result<JevDecision, Error> {
    if question.trim().is_empty() {
        return Err(Error::message("jev question is empty"));
    }
    let arguments = serde_json::json!({"question": question});
    run_jev_gate(llm, workspace, "jev", &arguments, 0).await
}

pub fn render(decision: &JevDecision) -> String {
    match decision {
        JevDecision::Continue => "continue".into(),
        JevDecision::Stop { .. } => "stop".into(),
        JevDecision::Compact => "compact".into(),
        JevDecision::Steer { text } => format!("steer: {text}"),
    }
}

/// Ask the fast model what to do with one pending worker tool.
///
/// A destructive command stops before any model call. `read` is the only
/// workspace tool. `stop`, `compact`, and `steer` end the gate.
pub async fn run_jev_gate<L: Llm>(
    llm: &L,
    workspace: &Workspace,
    tool: &str,
    arguments: &Json,
    transcript_chars: usize,
) -> Result<JevDecision, Error> {
    if let JevDecision::Stop { reason } = e4a_harness::decide(JevSnap {
        tool,
        arguments,
        transcript_chars,
    }) {
        return Ok(JevDecision::Stop { reason });
    }
    let snap = JevSnap {
        tool,
        arguments,
        transcript_chars,
    };
    let mut messages = vec![ChatMessage {
        role: "system".into(),
        text: "You are a fast gate, not a coding agent. Call stop, compact, or steer. You may read one file first.".into(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        is_error: false,
    }, ChatMessage {
        role: "user".into(),
        text: e4a_harness::prompt(&snap),
        tool_calls: Vec::new(),
        tool_call_id: None,
        is_error: false,
    }];
    let tools = jev_tool_schemas();
    for _ in 0..ROUNDS {
        let request = TurnRequest {
            messages: messages.clone(),
            tools: tools.clone(),
        };
        let mut stream = llm
            .stream(&request)
            .await
            .map_err(|error| Error::message(error.to_string()))?;
        let mut text = String::new();
        let mut call = None;
        while let Some(event) = stream.next().await {
            match event {
                LlmEvent::Text(chunk) => text.push_str(&chunk),
                LlmEvent::ToolCall(next) => call = Some(next),
                LlmEvent::Usage(_) => {}
            }
        }
        let Some(call) = call else {
            return Ok(parse_line(&text));
        };
        match call.name.as_str() {
            "stop" => {
                return Ok(JevDecision::Stop {
                    reason: "advisor stopped the step",
                })
            }
            "compact" => return Ok(JevDecision::Compact),
            "steer" => {
                let text = call
                    .arguments
                    .get("text")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if text.is_empty() {
                    return Ok(JevDecision::Continue);
                }
                return Ok(JevDecision::Steer { text });
            }
            "read" => {
                let path = call
                    .arguments
                    .get("path")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let body = match workspace.read(path) {
                    Ok(text) => clip(&text),
                    Err(error) => error.to_string(),
                };
                messages.push(ChatMessage {
                    role: "assistant".into(),
                    text: String::new(),
                    tool_calls: vec![call.clone()],
                    tool_call_id: None,
                    is_error: false,
                });
                messages.push(ChatMessage {
                    role: "tool".into(),
                    text: body,
                    tool_calls: Vec::new(),
                    tool_call_id: Some(call.id),
                    is_error: false,
                });
            }
            _ => return Ok(parse_line(&text)),
        }
    }
    Ok(JevDecision::Continue)
}

fn clip(text: &str) -> String {
    let mut end = READ_CAP.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}
