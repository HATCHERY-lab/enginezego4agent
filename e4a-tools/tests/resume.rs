//! A waiting powershell is finished from the sqlite journal. No second model call
//! runs the command.

use std::fs;

use e4a_harness::{Generation, TurnInput, TurnKind};
use e4a_llm::{FauxLlm, OpenRouterClient, ToolCall, ToolSchema, Usage};
use e4a_session::{Session, TaskState, ROOT_CONVERSATION_ID};
use e4a_tools::{resume_turns, resume_waiting, Workspace};
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
        cost_usd: 0.0,
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
    let view = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    let users = view
        .borrow()
        .entries
        .iter()
        .filter(|entry| entry.kind == "pi.user")
        .count();
    assert_eq!(users, 1, "resume must not add a user turn");
    let answered = view.borrow().entries.iter().any(|entry| {
        entry.kind == "pi.assistant"
            && entry
                .model
                .as_ref()
                .and_then(|model| model.to_string().contains("saw").then_some(()))
                .is_some()
    });
    assert!(answered, "assistant answer from the resumed generation is missing");
    session.close().await.unwrap();
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn resume_refuses_legacy_jev_and_advise_tools() {
    let root = scratch();
    let db = root.join("session.sqlite");
    let session = Session::sqlite(&db).await.unwrap();
    let workspace = Workspace::new(&root).unwrap();
    park_tool(
        &session,
        "advise",
        json!({"question": "what is 2 plus 2?"}),
    )
    .await;
    session.close().await.unwrap();

    let session = Session::sqlite(&db).await.unwrap();
    let finished = resume_waiting(&session, &workspace).await.unwrap();
    assert_eq!(finished.len(), 1);
    assert!(finished[0].is_error, "advise is not a worker tool");
    session.close().await.unwrap();

    let root = scratch();
    let db = root.join("session.sqlite");
    let session = Session::sqlite(&db).await.unwrap();
    let workspace = Workspace::new(&root).unwrap();
    park_tool(
        &session,
        "jev",
        json!({"question": "should we write a file?"}),
    )
    .await;
    session.close().await.unwrap();
    let session = Session::sqlite(&db).await.unwrap();
    let finished = resume_waiting(&session, &workspace).await.unwrap();
    assert_eq!(finished.len(), 1);
    assert!(finished[0].is_error, "jev is not a worker tool");
    session.close().await.unwrap();
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn resume_turns_runs_the_next_tool_then_answers() {
    let root = scratch();
    let db = root.join("session.sqlite");
    let session = Session::sqlite(&db).await.unwrap();
    let workspace = Workspace::new(&root).unwrap();
    let llm = FauxLlm::text("").with_tool_call(ToolCall {
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
    let llm = FauxLlm::text("")
        .with_tool_call(ToolCall {
            id: "c2".into(),
            name: "powershell".into(),
            arguments: json!({"command": "Write-Output e4a-again"}),
        })
        .then(FauxLlm::text("done"));
    let turns = resume_turns(&session, &workspace, &llm).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert!(matches!(turns[0].kind, TurnKind::Answered { .. }));
    let generation = session.task(parked.task_id).await.unwrap().unwrap();
    assert_eq!(generation.state.status(), TaskStatus::Terminal);
    let view = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    let blob = format!("{:?}", view.borrow().entries);
    let results = view
        .borrow()
        .entries
        .iter()
        .filter(|entry| entry.kind == "pi.toolResult")
        .count();
    assert_eq!(results, 2, "{blob}");
    assert!(blob.contains("e4a-resume"), "{blob}");
    assert!(blob.contains("e4a-again"), "{blob}");
    assert!(blob.contains("done"), "{blob}");
    let users = view
        .borrow()
        .entries
        .iter()
        .filter(|entry| entry.kind == "pi.user")
        .count();
    assert_eq!(users, 1, "{blob}");
    session.close().await.unwrap();
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn resume_turns_on_the_free_model_answers() {
    let key = std::env::var("E4A_OPENROUTER_KEY").unwrap_or_default();
    let socks = std::env::var("E4A_SOCKS").unwrap_or_default();
    if key.is_empty() || socks.is_empty() {
        eprintln!("skip live resume: socks or key unset");
        return;
    }
    let root = scratch();
    let db = root.join("session.sqlite");
    let session = Session::sqlite(&db).await.unwrap();
    let workspace = Workspace::new(&root).unwrap();
    let llm = FauxLlm::text("").with_tool_call(ToolCall {
        id: "c1".into(),
        name: "powershell".into(),
        arguments: json!({"command": "Write-Output e4a-live"}),
    });
    let parked = Generation::turn(&session, &llm, &powershell_input())
        .await
        .unwrap();
    session.close().await.unwrap();

    let session = Session::sqlite(&db).await.unwrap();
    let llm = OpenRouterClient::new("cohere/north-mini-code:free");
    let turns = resume_turns(&session, &workspace, &llm).await.unwrap();
    assert_eq!(turns.len(), 1, "free model did not finish the turn");
    assert!(
        matches!(turns[0].kind, TurnKind::Answered { .. }),
        "free model left the turn {:?}",
        turns[0].kind
    );
    let view = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    let blob = format!("{:?}", view.borrow().entries);
    assert!(blob.contains("e4a-live"), "{blob}");
    let _ = parked.task_id;
    session.close().await.unwrap();
    let _ = fs::remove_dir_all(&root);
}

async fn park_tool(session: &Session, name: &str, arguments: serde_json::Value) {
    let llm = FauxLlm::text("").with_tool_call(ToolCall {
        id: "c1".into(),
        name: name.into(),
        arguments,
    });
    let input = TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: "ask".into(),
        tools: vec![ToolSchema {
            name: name.into(),
            description: "ask".into(),
            parameters: json!({"type": "object"}),
        }],
    };
    let parked = Generation::turn(session, &llm, &input).await.unwrap();
    assert!(matches!(parked.kind, TurnKind::WaitingTool { .. }));
}
