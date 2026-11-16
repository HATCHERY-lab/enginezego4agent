//! One model turn, two parallel tool calls: both run, both results reach the
//! next request in call order, and the generation waits on the whole batch.

use std::fs;

use e4a_harness::{TurnInput, TurnKind};
use e4a_llm::{FauxLlm, ToolCall, ToolSchema};
use e4a_session::{Session, ROOT_CONVERSATION_ID};
use e4a_tools::{run_until_answer, Workspace};
use serde_json::json;

fn scratch() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "e4a-parallel-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn write_tool() -> ToolSchema {
    ToolSchema {
        name: "write".into(),
        description: "Write a UTF-8 file under the workspace root.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        }),
    }
}

#[tokio::test]
async fn two_calls_in_one_turn_run_before_the_next_request() {
    let root = scratch();
    let session = Session::sqlite(&root.join("session.sqlite")).await.unwrap();
    let workspace = Workspace::new(&root).unwrap();
    let llm = FauxLlm::text("")
        .with_tool_calls(vec![
            ToolCall {
                id: "call-a".into(),
                name: "write".into(),
                arguments: json!({ "path": "a.txt", "content": "first" }),
            },
            ToolCall {
                id: "call-b".into(),
                name: "write".into(),
                arguments: json!({ "path": "b.txt", "content": "second" }),
            },
        ])
        .then(FauxLlm::text("both written"));
    let input = TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: "write both files".into(),
        tools: vec![write_tool()],
    };
    let outcome = run_until_answer(&session, &llm, &workspace, &input)
        .await
        .unwrap();
    assert!(
        matches!(outcome.kind, TurnKind::Answered { .. }),
        "the turn did not answer: {:?}",
        outcome.kind
    );

    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "first",
        "the first call of the batch did not run"
    );
    assert_eq!(
        fs::read_to_string(root.join("b.txt")).unwrap(),
        "second",
        "the second call of the batch did not run"
    );

    // The model saw both results in its second request, in call order.
    let requests = llm.requests();
    assert_eq!(requests.len(), 2);
    let blob = serde_json::to_string(&requests[1].messages).unwrap();
    let first = blob.find("first").expect("first result missing");
    let second = blob.find("second").expect("second result missing");
    assert!(first < second, "tool results are out of call order: {blob}");

    let view = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    let results = view
        .borrow()
        .entries
        .iter()
        .filter(|entry| entry.kind == "pi.toolResult")
        .count();
    assert_eq!(results, 2, "one tool result per call");
    drop(view);
    session.close().await.unwrap();
    let _ = fs::remove_dir_all(&root);
}
