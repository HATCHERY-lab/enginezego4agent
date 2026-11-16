// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/compaction.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/compaction, MIT, see LICENSES/pi-MIT.txt.

//! One streamed turn that asks the compressor for a summary. No network client lives here.

use ez4a_llm::{ChatMessage, Llm, LlmEvent, TurnRequest};
use serde_json::Value;

use crate::compaction::serialize_conversation;
use crate::compaction_job::summary_entry;
use crate::compressor_agent::compressor_agent;
use crate::context_load::Line;

/// Ask the compressor model for a summary and return [`summary_entry`].
///
/// Line texts are joined the same way [`crate::compact_with`] joins them.
/// The compressor instruction is the system text and the transcript is the
/// user text. [`compressor_agent`] offers no tools, so the turn offers none.
/// A tool call from the model is an error. An empty summary is
/// [`summary_entry`]'s error.
pub async fn compact_model<L: Llm>(
    llm: &L,
    lines: &[Line],
    covered_until: i64,
) -> Result<Value, String> {
    let texts: Vec<String> = lines.iter().map(|line| line.text.clone()).collect();
    let transcript = serialize_conversation(&texts);
    let role = compressor_agent();
    let request = TurnRequest {
        messages: vec![
            chat_message("system", role.instructions),
            chat_message("user", &transcript),
        ],
        tools: Vec::new(),
    };
    let summary = read_summary(llm, &request).await?;
    summary_entry(&summary, covered_until).map_err(|error| error.to_string())
}

/// Fields a generation turn already stores on [`ChatMessage`].
fn chat_message(role: &str, text: &str) -> ChatMessage {
    ChatMessage {
        role: role.to_owned(),
        text: text.to_owned(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        is_error: false,
    }
}

async fn read_summary<L: Llm>(llm: &L, request: &TurnRequest) -> Result<String, String> {
    let mut stream = llm
        .stream(request)
        .await
        .map_err(|error| error.to_string())?;
    let mut summary = String::new();
    while let Some(event) = stream.next().await {
        match event {
            LlmEvent::Text(chunk) => summary.push_str(&chunk),
            LlmEvent::ToolCall(_) => {
                return Err("compaction model returned a tool call".to_owned());
            }
            LlmEvent::Usage(_) => {}
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::compact_model;
    use crate::compaction_job::summary_entry;
    use crate::compressor_agent::compressor_agent;
    use crate::context_load::Line;
    use ez4a_llm::{ChatMessage, FauxLlm, ToolCall, TurnRequest};
    use serde_json::json;

    fn line(entry_id: i64, text: &str) -> Line {
        Line {
            entry_id,
            text: text.to_owned(),
        }
    }

    #[tokio::test]
    async fn faux_text_is_the_summary_entry() {
        let llm = FauxLlm::text("Kept the goal.");
        let lines = vec![line(1, "alpha"), line(2, "beta")];
        let entry = compact_model(&llm, &lines, 12).await.unwrap();
        assert!(entry.to_string().contains("Kept the goal."));
        assert_eq!(entry, summary_entry("Kept the goal.", 12).unwrap());
        assert_eq!(llm.calls(), 1);
        assert_eq!(
            llm.requests(),
            vec![TurnRequest {
                messages: vec![
                    ChatMessage {
                        role: "system".into(),
                        text: compressor_agent().instructions.into(),
                        tool_calls: Vec::new(),
                        tool_call_id: None,
                        is_error: false,
                    },
                    ChatMessage {
                        role: "user".into(),
                        text: "alpha\n\nbeta".into(),
                        tool_calls: Vec::new(),
                        tool_call_id: None,
                        is_error: false,
                    },
                ],
                tools: Vec::new(),
            }]
        );
    }

    #[tokio::test]
    async fn a_tool_call_is_an_error() {
        let llm = FauxLlm::text("nope").with_tool_call(ToolCall {
            id: "call-1".into(),
            name: "read".into(),
            arguments: json!({}),
        });
        let err = compact_model(&llm, &[line(1, "alpha")], 3)
            .await
            .unwrap_err();
        assert_eq!(err, "compaction model returned a tool call");
    }

    #[tokio::test]
    async fn an_empty_summary_is_the_summary_entry_error() {
        let llm = FauxLlm::text("");
        let err = compact_model(&llm, &[line(1, "alpha")], 3)
            .await
            .unwrap_err();
        assert_eq!(err, summary_entry("", 3).unwrap_err().to_string());
        assert_eq!(err, "compaction summary is empty");
    }
}
