// engine zero for agents advisor turn. No disk and no process.

//! One read-only question. A `read` call is answered from its arguments.

use ez4a_llm::{ChatMessage, Llm, LlmEvent, ToolCall, ToolSchema, TurnRequest};
use serde_json::{json, Value};

use crate::advisor_agent::advisor_agent;

/// Ask the advisor. An empty question is refused before the model is called.
///
/// The turn offers one `read` tool. Its result is `read:{path}` and does not
/// touch the disk. Any other tool name is `advisor refused a tool`.
pub async fn advise<L: Llm>(llm: &L, question: &str) -> Result<String, String> {
    if question.is_empty() {
        return Err("advisor question must be non-empty".to_owned());
    }
    let role = advisor_agent();
    let request = TurnRequest {
        messages: vec![
            chat_message("system", role.instructions),
            chat_message("user", question),
        ],
        tools: vec![read_tool()],
    };
    let mut stream = llm
        .stream(&request)
        .await
        .map_err(|error| error.to_string())?;
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        match event {
            LlmEvent::Text(chunk) => text.push_str(&chunk),
            LlmEvent::ToolCall(call) => return read_result(&call),
            LlmEvent::Usage(_) => {}
        }
    }
    Ok(text)
}

fn chat_message(role: &str, text: &str) -> ChatMessage {
    ChatMessage {
        role: role.to_owned(),
        text: text.to_owned(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        is_error: false,
    }
}

fn read_tool() -> ToolSchema {
    ToolSchema {
        name: "read".to_owned(),
        description: "Read one UTF-8 file. One path only.".to_owned(),
        parameters: json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
    }
}

fn read_result(call: &ToolCall) -> Result<String, String> {
    if call.name != "read" {
        return Err("advisor refused a tool".to_owned());
    }
    let path = call
        .arguments
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "advisor read needs a path".to_owned())?;
    Ok(format!("read:{path}"))
}

#[cfg(test)]
mod tests {
    use super::advise;
    use crate::advisor_agent::advisor_agent;
    use ez4a_llm::{FauxLlm, ToolCall};
    use serde_json::json;

    #[tokio::test]
    async fn faux_text_is_the_advice() {
        let llm = FauxLlm::text("looks fine");
        let text = advise(&llm, "is this ok").await.unwrap();
        assert_eq!(text, "looks fine");
        assert_eq!(llm.calls(), 1);
        let request = llm.requests().remove(0);
        assert_eq!(request.messages[0].role, "system");
        assert_eq!(request.messages[0].text, advisor_agent().instructions);
        assert_eq!(request.messages[1].role, "user");
        assert_eq!(request.messages[1].text, "is this ok");
        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "read");
        assert_eq!(request.tools[0].parameters["required"], json!(["path"]));
        assert_eq!(
            request.tools[0].parameters["properties"]["path"]["type"],
            json!("string")
        );
    }

    #[tokio::test]
    async fn a_read_tool_call_returns_the_path_and_does_not_read() {
        let path = "E:/no-such-turnside-dir/missing.txt";
        let llm = FauxLlm::text("").with_tool_call(ToolCall {
            id: "call-1".into(),
            name: "read".into(),
            arguments: json!({ "path": path }),
        });
        let text = advise(&llm, "read the note").await.unwrap();
        assert_eq!(text, format!("read:{path}"));
        assert_eq!(llm.calls(), 1);
    }

    #[tokio::test]
    async fn an_empty_question_does_not_call_the_model() {
        let llm = FauxLlm::text("nope");
        let err = advise(&llm, "").await.unwrap_err();
        assert_eq!(err, "advisor question must be non-empty");
        assert_eq!(llm.calls(), 0);
    }

    #[tokio::test]
    async fn a_write_tool_is_refused() {
        let llm = FauxLlm::text("no").with_tool_call(ToolCall {
            id: "call-2".into(),
            name: "write".into(),
            arguments: json!({ "path": "note.txt" }),
        });
        let err = advise(&llm, "write it").await.unwrap_err();
        assert_eq!(err, "advisor refused a tool");
    }
}
