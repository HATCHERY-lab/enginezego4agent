//! A waiting powershell is finished from the sqlite journal. No second model call
//! runs the command.

use std::fs;

use e4a_harness::{Generation, TurnInput, TurnKind};
use e4a_llm::{FauxLlm, ToolCall, ToolSchema, Usage};
use e4a_session::{Session, TaskState, ROOT_CONVERSATION_ID};
use e4a_tools::{resume_waiting, Workspace};
use e4a_types::TaskStatus;
use serde_json::json;

fn scratch() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "e4a-resume-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn powershell_input() -> TurnInput {
    TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: "run it".into(),
        tools: vec![ToolSchema {
            name: "powershell".into(),
            description: "shell".into(),
            parameters: json!({"type": "object"}),
        }],
    }
}

#[tokio::test]
async fn resume_runs_the_waiting_powershell_and_the_next_request_sees_it() {
    let root = scratch();
    let db = root.join("session.sqlite");
    let session = Session::sqlite(&db).await.unwrap();
    let workspace = Workspace::new(&root).unwrap();
    let llm = FauxLlm::text("").with_usage(Usage {
        input_tokens: 1,
        output_tokens: 1,
        provider: "faux".into(),
        model: "scripted".into(),
    }).with_tool_call(ToolCall {
        id: "c1".into(),
        name: "powershell".into(),
        arguments: json!({"command": "Write-Output e4a-resume"}),
    });
    let parked = Generation::turn(&session, &llm, &powershell_input())
        .await
        .unwrap();
    assert!(matches!(parked.kind, TurnKind::WaitingTool { .. }));
    session.close().await.unwrap();

    let session = Session::sqlite(&db).await.unwrap();
    let finished = resume_waiting(&session, &workspace).await.unwrap();
    assert_eq!(finished.len(), 1);
    assert!(!finished[0].is_error);
    assert!(finished[0].content.contains("e4a-resume"), "{}", finished[0].content);
    let stored = session.entry(finished[0].entry_id).await.unwrap().unwrap();
    assert_eq!(stored.kind, "pi.toolResult");
    let generation = session.task(parked.task_id).await.unwrap().unwrap();
    assert!(
        !matches!(generation.state, TaskState::Waiting { .. }),
        "generation still waiting"
    );
    assert_eq!(generation.state.status(), TaskStatus::Running);

    let next = FauxLlm::text("saw");
    let outcome = Generation::bind(parked.task_id, ROOT_CONVERSATION_ID)
        .resume(&session, &next)
        .await
        .unwrap();
    assert!(matches!(outcome.kind, TurnKind::Answered { .. }));
    let blob = format!("{:?}", next.requests()[0]);
    assert!(blob.contains("e4a-resume"), "{blob}");
    assert!(blob.contains("tool"), "{blob}");
    session.close().await.unwrap();
    let _ = fs::remove_dir_all(&root);
}
