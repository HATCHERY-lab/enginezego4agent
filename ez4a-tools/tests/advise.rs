//! The advisor branch on a scripted model: notes land in the main
//! conversation, the cursor moves, and a second review with no delta is
//! silent. No network, no worker.

use std::fs;

use ez4a_harness::{Generation, TurnInput, TurnKind};
use ez4a_llm::{FauxLlm, ToolCall, ToolSchema};
use ez4a_session::{Session, ROOT_CONVERSATION_ID};
use ez4a_tools::{review, Workspace};
use serde_json::json;

fn scratch() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ez4a-advise-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn chat_input() -> TurnInput {
    TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: "say hi".into(),
        tools: vec![ToolSchema {
            name: "none".into(),
            description: "no tools".into(),
            parameters: json!({"type": "object"}),
        }],
    }
}

fn advisor_call(note: &str, severity: &str) -> FauxLlm {
    FauxLlm::text("").with_tool_call(ToolCall {
        id: "advise-1".into(),
        name: "advise".into(),
        arguments: json!({ "note": note, "severity": severity }),
    })
}

#[tokio::test]
async fn notes_land_in_the_main_conversation_and_the_cursor_moves() {
    let root = scratch();
    let session = Session::sqlite(&root.join("session.sqlite")).await.unwrap();
    let workspace = Workspace::new(&root).unwrap();

    let worker = FauxLlm::text("the work is done");
    let outcome = Generation::turn(&session, &worker, &chat_input())
        .await
        .unwrap();
    assert!(matches!(outcome.kind, TurnKind::Answered { .. }));

    let notes = review(
        &session,
        &advisor_call("the loop never ends", "blocker"),
        &workspace,
        ROOT_CONVERSATION_ID,
    )
    .await
    .unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].severity, "blocker");

    let view = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    let binding = view.borrow();
    let advisory = binding
        .entries
        .iter()
        .find(|entry| entry.kind == "pi.advisory")
        .expect("the steer record is missing");
    let text = advisory.model.as_ref().unwrap()[0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.contains("severity=\"blocker\""), "{text}");
    assert!(text.contains("the loop never ends"), "{text}");
    drop(binding);
    drop(view);

    // No new worker delta: the advisor reviews nothing and stays silent.
    let again = review(&session, &advisor_call("repeat", "nit"), &workspace, ROOT_CONVERSATION_ID)
        .await
        .unwrap();
    assert!(again.is_empty(), "a dry delta produced {again:?}");
    session.close().await.unwrap();
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn praise_is_not_a_note() {
    let root = scratch();
    let session = Session::sqlite(&root.join("session.sqlite")).await.unwrap();
    let workspace = Workspace::new(&root).unwrap();
    let worker = FauxLlm::text("done");
    Generation::turn(&session, &worker, &chat_input())
        .await
        .unwrap();

    let notes = review(
        &session,
        &advisor_call("LGTM", "nit"),
        &workspace,
        ROOT_CONVERSATION_ID,
    )
    .await
    .unwrap();
    assert!(notes.is_empty());
    let view = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    assert!(
        !view.borrow().entries.iter().any(|entry| entry.kind == "pi.advisory"),
        "an empty review wrote a steer record"
    );
    session.close().await.unwrap();
    let _ = fs::remove_dir_all(&root);
}
