use e4a_llm::{
    ChatMessage, FauxLlm, Llm, LlmEvent, OpenRouterClient, ToolCall, ToolSchema, TurnRequest, Usage,
};
use serde_json::json;

fn request() -> TurnRequest {
    TurnRequest {
        messages: vec![ChatMessage {
            role: "user".into(),
            text: "hi".into(),
            tool_calls: Vec::new(),
        }],
        tools: vec![ToolSchema {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
        }],
    }
}

async fn collect(llm: &impl Llm, request: &TurnRequest) -> Result<Vec<LlmEvent>, e4a_llm::Error> {
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
async fn openrouter_is_not_wired() {
    let client = OpenRouterClient::new("openrouter::test/model");
    let error = client.stream(&request()).await.unwrap_err();
    assert_eq!(error.to_string(), "live client not wired");
    assert_eq!(client.model(), "openrouter::test/model");
}
