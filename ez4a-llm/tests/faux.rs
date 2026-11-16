use ez4a_llm::{
    ChatMessage, FauxLlm, Llm, LlmEvent, OpenRouterClient, ToolCall, ToolSchema, TurnRequest, Usage,
};
use serde_json::json;

fn request() -> TurnRequest {
    TurnRequest {
        messages: vec![ChatMessage {
            role: "user".into(),
            text: "hi".into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            is_error: false,
        }],
        tools: vec![ToolSchema {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
        }],
    }
}

async fn collect(llm: &impl Llm, request: &TurnRequest) -> Result<Vec<LlmEvent>, ez4a_llm::Error> {
    let mut stream = llm.stream(request).await?;
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    Ok(events)
}

#[tokio::test]
async fn faux_replays_text_tool_call_and_usage() {
    let usage = Usage {
        input_tokens: 11,
        output_tokens: 7,
        provider: "faux".into(),
        model: "scripted".into(),
        cost_usd: 0.0,
    };
    let llm = FauxLlm::text("hello")
        .with_tool_call(ToolCall {
            id: "call-1".into(),
            name: "read".into(),
            arguments: json!({ "path": "note.txt" }),
        })
        .with_usage(usage.clone());
    let events = collect(&llm, &request()).await.unwrap();
    assert_eq!(
        events,
        vec![
            LlmEvent::Text("hello".into()),
            LlmEvent::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                arguments: json!({ "path": "note.txt" }),
            }),
            LlmEvent::Usage(usage),
        ]
    );
    assert_eq!(llm.calls(), 1);
}

#[tokio::test]
async fn faux_replays_scripts_in_order_then_repeats_the_last() {
    let single = FauxLlm::text("only");
    let first = collect(&single, &request()).await.unwrap();
    let second = collect(&single, &request()).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(single.calls(), 2);

    let llm = FauxLlm::text("one").then(FauxLlm::text("two"));
    let one = collect(&llm, &request()).await.unwrap();
    let two = collect(&llm, &request()).await.unwrap();
    let again = collect(&llm, &request()).await.unwrap();
    assert_eq!(one[0], LlmEvent::Text("one".into()));
    assert_eq!(two[0], LlmEvent::Text("two".into()));
    assert_eq!(again[0], LlmEvent::Text("two".into()));
    assert_eq!(llm.calls(), 3);
    assert_eq!(llm.requests().len(), 3);
}

#[test]
fn chat_message_without_tool_fields_parses() {
    let message: ChatMessage =
        serde_json::from_str(r#"{"role":"user","text":"hi"}"#).unwrap();
    assert_eq!(message.role, "user");
    assert_eq!(message.text, "hi");
    assert!(message.tool_calls.is_empty());
    assert_eq!(message.tool_call_id, None);
    assert!(!message.is_error);
}

#[tokio::test]
async fn openrouter_is_not_wired() {
    let client = OpenRouterClient::new("openrouter::test/model");
    assert_eq!(client.model(), "openrouter::test/model");
    // A configured door would dial. This test never opens a socket.
    if std::env::var_os("EZ4A_SOCKS").is_some_and(|value| !value.is_empty()) {
        return;
    }
    let error = client.stream(&request()).await.unwrap_err();
    assert_eq!(error.to_string(), "EZ4A_SOCKS is unset");
}
